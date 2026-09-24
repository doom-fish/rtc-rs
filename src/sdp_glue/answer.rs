//! Build a local SDP answer for a received offer, rejecting any data channel.

use super::offer::supported_feedback;
use super::{codec_matches, LocalSdpIdentity};
use crate::config::{Codec, MediaConfig, RtcConfig};
use crate::error::{Result, RtcError};
use sdp::{
    Answer, Attribute, Fingerprint, Fmtp, H264Fmtp, LocalIdentity, MediaDescription, MediaKind,
    Offer, RtcpFb, RtpMap, SetupRole,
};

/// Build an [`Answer`] for `offer`, accepting the audio/video codecs in `cfg`
/// and cleanly rejecting any `m=application` (SCTP data channel) section with
/// port 0 — required because browsers always offer one.
pub fn build_answer(
    offer: &Offer,
    cfg: &RtcConfig,
    identity: &LocalSdpIdentity,
    setup: SetupRole,
) -> Result<Answer> {
    let local = LocalIdentity::new(
        identity.ice_ufrag.clone(),
        identity.ice_pwd.to_string(),
        Fingerprint::new("sha-256", identity.fingerprint_sha256.to_vec()),
    );

    let mut builder = Offer::answer().identity(local).reject_data();

    // Match the offered payload type for each kind when present, so the
    // codec we answer with lines up with what the peer offered.
    if let Some(audio) = &cfg.audio {
        if let Some(section) = offered_section(offer, MediaKind::Audio) {
            if let Some(pt) = offered_pt(section, audio.codec) {
                builder = builder.accept_audio(codec_attrs(audio, section, pt));
            }
        }
    }
    if let Some(video) = &cfg.video {
        if let Some(section) = offered_section(offer, MediaKind::Video) {
            if let Some(pt) = offered_pt(section, video.codec) {
                builder = builder.accept_video(codec_attrs(video, section, pt));
            }
        }
    }

    let mut answer = builder.build(offer).map_err(RtcError::from)?;
    for m in answer.0.media.iter_mut().filter(|m| m.port != 0) {
        for attr in &mut m.attributes {
            if let Attribute::Setup(role) = attr {
                *role = setup;
            }
        }
    }
    Ok(answer)
}

fn offered_section(offer: &Offer, kind: MediaKind) -> Option<&MediaDescription> {
    offer
        .sdp()
        .media
        .iter()
        .find(|m| m.kind == kind && (m.port != 0 || m.is_bundle_only()))
}

/// Find the payload type the offer used for an encoding name (case-insensitive).
fn offered_pt(section: &MediaDescription, codec: Codec) -> Option<u8> {
    let mut candidates = section
        .rtpmaps()
        .into_iter()
        .filter(|r| codec_matches(codec, &r.encoding))
        .map(|r| r.payload);
    match codec {
        Codec::Opus => candidates.next(),
        Codec::H264 => {
            let usable: Vec<(u8, Option<u32>)> = candidates
                .filter_map(|pt| {
                    let fmtp =
                        H264Fmtp::parse(section.fmtp_for(pt).map_or("", |f| f.params.as_str()));
                    (fmtp.packetization_mode() == Some(1)).then_some((pt, fmtp.profile_level_id()))
                })
                .collect();
            usable
                .iter()
                .find(|(_, profile)| profile.is_some_and(|p| p >> 16 == 0x42))
                .or_else(|| usable.first())
                .map(|&(pt, _)| pt)
        }
    }
}

fn codec_attrs(mc: &MediaConfig, section: &MediaDescription, pt: u8) -> Vec<Attribute> {
    let channels = if mc.channels > 0 {
        Some(u32::from(mc.channels))
    } else {
        None
    };
    let params = match mc.codec {
        Codec::Opus => mc.fmtp.clone(),
        Codec::H264 => section
            .fmtp_for(pt)
            .map_or_else(|| mc.fmtp.clone(), |f| f.params.clone()),
    };
    let mut attrs = vec![
        Attribute::RtpMap(RtpMap {
            payload: pt,
            encoding: mc.encoding_name().to_string(),
            clock_rate: mc.clock_rate,
            channels,
        }),
        Attribute::Fmtp(Fmtp {
            format: pt.to_string(),
            params,
        }),
    ];
    let pt_text = pt.to_string();
    for supported in supported_feedback(mc.codec) {
        let offered = section.attributes.iter().any(|a| {
            matches!(a, Attribute::RtcpFb(fb)
                if (fb.format == pt_text || fb.format == "*") && fb.feedback == *supported)
        });
        if offered {
            attrs.push(Attribute::RtcpFb(RtcpFb {
                format: pt_text.clone(),
                feedback: (*supported).to_string(),
            }));
        }
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_section(lines: &str) -> MediaDescription {
        let text = format!(
            "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96 102 106\r\n{lines}"
        );
        sdp::Sdp::parse(&text).unwrap().media.remove(0)
    }

    #[test]
    fn h264_prefers_packetization_mode_1_baseline() {
        let section = video_section(
            "a=rtpmap:96 VP8/90000\r\n\
             a=rtpmap:102 H264/90000\r\n\
             a=fmtp:102 packetization-mode=0;profile-level-id=42e01f\r\n\
             a=rtpmap:106 H264/90000\r\n\
             a=fmtp:106 packetization-mode=1;profile-level-id=42e01f\r\n",
        );
        assert_eq!(offered_pt(&section, Codec::H264), Some(106));
    }

    #[test]
    fn h264_without_packetization_mode_1_is_not_accepted() {
        let section = video_section(
            "a=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=0;profile-level-id=42e01f\r\n",
        );
        assert_eq!(offered_pt(&section, Codec::H264), None);
    }

    #[test]
    fn only_offered_feedback_is_echoed() {
        let section = video_section(
            "a=rtpmap:106 H264/90000\r\n\
             a=fmtp:106 packetization-mode=1;profile-level-id=42e01f\r\n\
             a=rtcp-fb:106 nack pli\r\n\
             a=rtcp-fb:* transport-cc\r\n\
             a=rtcp-fb:106 goog-remb\r\n",
        );
        let attrs = codec_attrs(&MediaConfig::h264_constrained_baseline(), &section, 106);
        let feedback: Vec<&str> = attrs
            .iter()
            .filter_map(|a| match a {
                Attribute::RtcpFb(fb) => Some(fb.feedback.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(feedback, vec!["transport-cc", "nack pli"]);
    }
}
