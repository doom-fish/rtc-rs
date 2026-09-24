//! SDP negotiation glue: building local offers/answers and applying remote
//! descriptions onto transport parameters.
//!
//! All WebRTC profile knowledge (BUNDLE, rtcp-mux, ICE, DTLS-SRTP
//! fingerprints, H.264/Opus fmtp) lives in the sibling `sdp` crate; this module
//! only orchestrates it.

mod apply;
mod answer;
mod offer;

pub use apply::{apply_remote, RemoteParams};
pub use answer::build_answer;
pub use offer::{build_offer, OfferSsrcs};

use crate::config::{Codec, MediaConfig};
use crate::error::{Result, RtcError};
use crate::state::DtlsRole;
use sdp::{Attribute, Direction, MediaKind, Sdp, SetupRole};
use zeroize::Zeroizing;

/// Local DTLS-SRTP / ICE identity used on every media section.
#[derive(Clone)]
pub struct LocalSdpIdentity {
    /// ICE username fragment.
    pub ice_ufrag: String,
    /// ICE password.
    pub ice_pwd: Zeroizing<String>,
    /// Local DTLS certificate fingerprint (SHA-256, 32 bytes).
    pub fingerprint_sha256: [u8; 32],
}

/// One-byte RTP header-extension ids we negotiate (RFC 8285).
pub const MID_EXT_ID: u8 = 1;
/// TWCC transport-wide sequence-number header-extension id.
pub const TWCC_EXT_ID: u8 = 3;

/// Decide our local DTLS role from the remote `a=setup` value (RFC 5763 §5).
///
/// * remote `active`  → we are passive → DTLS **server**
/// * remote `passive` → we are active  → DTLS **client**
/// * remote `actpass` → we choose `preference`
pub fn dtls_role_for_offer(remote: Option<SetupRole>, preference: DtlsRole) -> Result<DtlsRole> {
    match remote {
        None | Some(SetupRole::ActPass) => Ok(preference),
        Some(SetupRole::Active) => Ok(DtlsRole::Server),
        Some(SetupRole::Passive) => Ok(DtlsRole::Client),
        Some(SetupRole::HoldConn) => Err(RtcError::Negotiation(
            "the remote offer holds the connection (a=setup:holdconn)".into(),
        )),
    }
}

pub fn dtls_role_for_answer(remote: Option<SetupRole>) -> Result<DtlsRole> {
    match remote {
        Some(SetupRole::Active) => Ok(DtlsRole::Server),
        Some(SetupRole::Passive) => Ok(DtlsRole::Client),
        Some(SetupRole::ActPass) => Err(RtcError::Negotiation(
            "the remote answer uses a=setup:actpass, which only an offer may use (RFC 5763 §5)"
                .into(),
        )),
        Some(SetupRole::HoldConn) => Err(RtcError::Negotiation(
            "the remote answer holds the connection (a=setup:holdconn)".into(),
        )),
        None => Err(RtcError::Negotiation("the remote answer has no a=setup attribute".into())),
    }
}

pub fn setup_for_role(role: DtlsRole) -> SetupRole {
    match role {
        DtlsRole::Client => SetupRole::Active,
        DtlsRole::Server => SetupRole::Passive,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedMedia {
    pub mid: String,
    pub payload_type: u8,
    pub mid_ext_id: Option<u8>,
    pub twcc_ext_id: Option<u8>,
    pub send: bool,
    pub recv: bool,
    pub nack: bool,
    pub pli: bool,
    pub transport_cc: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Negotiated {
    pub audio: Option<NegotiatedMedia>,
    pub video: Option<NegotiatedMedia>,
}

impl Negotiated {
    pub fn from_answer(
        answer: &Sdp,
        local_is_answerer: bool,
        audio: Option<&MediaConfig>,
        video: Option<&MediaConfig>,
    ) -> Self {
        let mut negotiated = Self::default();
        for m in &answer.media {
            if m.port == 0 {
                continue;
            }
            let (slot, cfg) = match m.kind {
                MediaKind::Audio if negotiated.audio.is_none() => (&mut negotiated.audio, audio),
                MediaKind::Video if negotiated.video.is_none() => (&mut negotiated.video, video),
                _ => continue,
            };
            let Some(cfg) = cfg else { continue };
            let Some(payload_type) = m
                .rtpmaps()
                .iter()
                .find(|r| codec_matches(cfg.codec, &r.encoding))
                .map(|r| r.payload)
            else {
                continue;
            };
            let mut mid_ext_id = None;
            let mut twcc_ext_id = None;
            for attr in &m.attributes {
                if let Attribute::Extmap(e) = attr {
                    if e.uri == sdp::extmap::URI_MID {
                        mid_ext_id = u8::try_from(e.id).ok().filter(|id| (1..=14).contains(id));
                    } else if e.uri == sdp::extmap::URI_TWCC {
                        twcc_ext_id = u8::try_from(e.id).ok().filter(|id| (1..=14).contains(id));
                    }
                }
            }
            let answered = m.direction().unwrap_or(Direction::SendRecv);
            let ours = if local_is_answerer { answered } else { answered.reverse() };
            let format = payload_type.to_string();
            let feedback = |name: &str| {
                m.attributes.iter().any(|a| {
                    matches!(a, Attribute::RtcpFb(fb)
                        if (fb.format == format || fb.format == "*") && fb.feedback == name)
                })
            };
            *slot = Some(NegotiatedMedia {
                mid: m.mid().unwrap_or_default().to_string(),
                payload_type,
                mid_ext_id,
                twcc_ext_id,
                send: matches!(ours, Direction::SendRecv | Direction::SendOnly),
                recv: matches!(ours, Direction::SendRecv | Direction::RecvOnly),
                nack: feedback("nack"),
                pli: feedback("nack pli"),
                transport_cc: feedback("transport-cc"),
            });
        }
        negotiated
    }

    pub fn twcc_ext_id(&self) -> Option<u8> {
        self.audio
            .as_ref()
            .and_then(|m| m.twcc_ext_id)
            .or_else(|| self.video.as_ref().and_then(|m| m.twcc_ext_id))
    }
}

pub(crate) fn codec_matches(codec: Codec, encoding: &str) -> bool {
    match codec {
        Codec::Opus => encoding.eq_ignore_ascii_case("opus"),
        Codec::H264 => encoding.eq_ignore_ascii_case("h264"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_setup_picks_the_complementary_role() {
        assert_eq!(dtls_role_for_offer(Some(SetupRole::Active), DtlsRole::Client).unwrap(), DtlsRole::Server);
        assert_eq!(dtls_role_for_offer(Some(SetupRole::Passive), DtlsRole::Server).unwrap(), DtlsRole::Client);
        assert_eq!(dtls_role_for_offer(Some(SetupRole::ActPass), DtlsRole::Server).unwrap(), DtlsRole::Server);
        assert_eq!(dtls_role_for_offer(Some(SetupRole::ActPass), DtlsRole::Client).unwrap(), DtlsRole::Client);
        assert_eq!(dtls_role_for_offer(None, DtlsRole::Client).unwrap(), DtlsRole::Client);
        assert!(dtls_role_for_offer(Some(SetupRole::HoldConn), DtlsRole::Client).is_err());
    }

    #[test]
    fn answer_setup_is_validated() {
        assert_eq!(
            dtls_role_for_answer(Some(SetupRole::Active)).unwrap(),
            DtlsRole::Server
        );
        assert_eq!(
            dtls_role_for_answer(Some(SetupRole::Passive)).unwrap(),
            DtlsRole::Client
        );
        assert!(matches!(
            dtls_role_for_answer(Some(SetupRole::ActPass)),
            Err(RtcError::Negotiation(_))
        ));
        assert!(matches!(
            dtls_role_for_answer(Some(SetupRole::HoldConn)),
            Err(RtcError::Negotiation(_))
        ));
        assert!(matches!(dtls_role_for_answer(None), Err(RtcError::Negotiation(_))));
    }

    #[test]
    fn setup_attribute_matches_the_role() {
        assert_eq!(setup_for_role(DtlsRole::Client), SetupRole::Active);
        assert_eq!(setup_for_role(DtlsRole::Server), SetupRole::Passive);
    }

    const ANSWER: &str = "v=0\r\n\
o=- 1 1 IN IP4 0.0.0.0\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE a0 v0\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 109\r\n\
c=IN IP4 0.0.0.0\r\n\
a=mid:a0\r\n\
a=recvonly\r\n\
a=rtcp-mux\r\n\
a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
a=extmap:5 http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01\r\n\
a=rtpmap:109 opus/48000/2\r\n\
a=rtcp-fb:109 nack\r\n\
a=rtcp-fb:* transport-cc\r\n\
m=video 0 UDP/TLS/RTP/SAVPF 102\r\n\
c=IN IP4 0.0.0.0\r\n\
a=mid:v0\r\n\
a=rtpmap:102 H264/90000\r\n";

    #[test]
    fn negotiated_media_follows_the_answer() {
        let sdp = Sdp::parse(ANSWER).unwrap();
        let audio_cfg = MediaConfig::opus_default();
        let video_cfg = MediaConfig::h264_constrained_baseline();
        let offerer = Negotiated::from_answer(&sdp, false, Some(&audio_cfg), Some(&video_cfg));
        let audio = offerer.audio.as_ref().unwrap();
        assert_eq!(audio.mid, "a0");
        assert_eq!(audio.payload_type, 109);
        assert_eq!(audio.mid_ext_id, Some(4));
        assert_eq!(audio.twcc_ext_id, Some(5));
        assert!(audio.send);
        assert!(!audio.recv);
        assert!(audio.nack);
        assert!(!audio.pli);
        assert!(audio.transport_cc);
        assert!(offerer.video.is_none());
        assert_eq!(offerer.twcc_ext_id(), Some(5));

        let answerer = Negotiated::from_answer(&sdp, true, Some(&audio_cfg), None);
        let audio = answerer.audio.unwrap();
        assert!(!audio.send);
        assert!(audio.recv);
    }
}
