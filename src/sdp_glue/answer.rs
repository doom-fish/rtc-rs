//! Build a local SDP answer for a received offer, rejecting any data channel.

use super::LocalSdpIdentity;
use crate::config::{Codec, MediaConfig, RtcConfig};
use crate::error::{Result, RtcError};
use sdp::{Answer, Attribute, Fingerprint, Fmtp, LocalIdentity, Offer, RtpMap};

/// Build an [`Answer`] for `offer`, accepting the audio/video codecs in `cfg`
/// and cleanly rejecting any `m=application` (SCTP data channel) section with
/// port 0 — required because browsers always offer one.
pub fn build_answer(
    offer: &Offer,
    cfg: &RtcConfig,
    identity: &LocalSdpIdentity,
) -> Result<Answer> {
    let local = LocalIdentity::new(
        identity.ice_ufrag.clone(),
        identity.ice_pwd.clone(),
        Fingerprint::new("sha-256", identity.fingerprint_sha256.to_vec()),
    );

    let mut builder = Offer::answer().identity(local).reject_data();

    // Match the offered payload type for each kind when present, so the
    // codec we answer with lines up with what the peer offered.
    if let Some(audio) = &cfg.audio {
        let pt = offered_pt(offer, "opus").unwrap_or(audio.payload_type);
        builder = builder.accept_audio(codec_attrs(audio, pt));
    }
    if let Some(video) = &cfg.video {
        let pt = offered_pt(offer, "h264").unwrap_or(video.payload_type);
        builder = builder.accept_video(codec_attrs(video, pt));
    }

    builder.build(offer).map_err(RtcError::from)
}

fn codec_attrs(mc: &MediaConfig, pt: u8) -> Vec<Attribute> {
    let channels = if mc.channels > 0 { Some(u32::from(mc.channels)) } else { None };
    vec![
        Attribute::RtpMap(RtpMap {
            payload: pt,
            encoding: mc.encoding_name().to_string(),
            clock_rate: mc.clock_rate,
            channels,
        }),
        Attribute::Fmtp(Fmtp {
            format: pt.to_string(),
            params: mc.fmtp.clone(),
        }),
    ]
}

/// Find the payload type the offer used for an encoding name (case-insensitive).
fn offered_pt(offer: &Offer, encoding: &str) -> Option<u8> {
    let want = match encoding {
        "opus" => Codec::Opus,
        _ => Codec::H264,
    };
    let target = match want {
        Codec::Opus => "opus",
        Codec::H264 => "h264",
    };
    for m in &offer.sdp().media {
        for rtpmap in m.rtpmaps() {
            if rtpmap.encoding.eq_ignore_ascii_case(target) {
                return Some(rtpmap.payload);
            }
        }
    }
    None
}
