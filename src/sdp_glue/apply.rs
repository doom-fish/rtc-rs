//! Apply a remote SDP description: validate WebRTC requirements and extract
//! the transport parameters (ICE creds, fingerprint, setup, candidates).

use crate::error::{Result, RtcError};
use sdp::{Attribute, Fingerprint, MediaKind, Sdp, SetupRole};
use zeroize::Zeroizing;

/// Everything `rtc` needs from a remote description to drive the transport.
#[derive(Clone)]
pub struct RemoteParams {
    /// Remote ICE username fragment.
    pub ice_ufrag: String,
    /// Remote ICE password.
    pub ice_pwd: Zeroizing<String>,
    /// Remote DTLS fingerprint (SHA-256).
    pub fingerprint_sha256: [u8; 32],
    /// Remote `a=setup` role.
    pub setup: Option<SetupRole>,
    /// In-line ICE candidates parsed from the SDP, if any.
    pub candidates: Vec<String>,
    pub end_of_candidates: bool,
}

struct Transport {
    ufrag: String,
    pwd: Zeroizing<String>,
    fingerprint: [u8; 32],
    setup: Option<SetupRole>,
}

impl Transport {
    fn same_as(&self, other: &Self) -> bool {
        self.ufrag == other.ufrag
            && self.pwd.as_str() == other.pwd.as_str()
            && self.fingerprint == other.fingerprint
            && self.setup == other.setup
    }
}

/// Validate the WebRTC profile requirements and pull out [`RemoteParams`].
///
/// Requirements enforced (RFC 8843 / RFC 5763 / RFC 5761):
/// * a BUNDLE group is present,
/// * every media section that carries its own transport has rtcp-mux,
/// * ICE ufrag/pwd and exactly one SHA-256 fingerprint are present, and all
///   bundled sections agree on them,
/// * `m=application` data channels are tolerated only when rejected (port 0).
pub fn apply_remote(sdp: &Sdp) -> Result<RemoteParams> {
    if sdp.bundle_group().is_none() {
        return Err(RtcError::Negotiation(
            "remote SDP has no BUNDLE group".into(),
        ));
    }

    let mut transport: Option<Transport> = None;
    let mut candidates = Vec::new();
    let mut end_of_candidates = sdp
        .attributes
        .iter()
        .any(|a| matches!(a, Attribute::EndOfCandidates));

    for m in &sdp.media {
        // A data channel is acceptable only when it is rejected (port 0).
        if m.kind == MediaKind::Application {
            if !m.is_rejected() {
                return Err(RtcError::Negotiation(
                    "remote offered an active data channel (m=application); only rejected \
                     data channels are supported"
                        .into(),
                ));
            }
            continue;
        }

        if m.port == 0 {
            continue;
        }

        if !m.has_rtcp_mux() {
            return Err(RtcError::Negotiation(format!(
                "media section mid={:?} is missing a=rtcp-mux",
                m.mid()
            )));
        }

        // Session- or media-level ICE/fingerprint/setup (media-level wins).
        let section = Transport {
            ufrag: sdp
                .ice_ufrag_for(m)
                .ok_or_else(|| RtcError::Negotiation("remote SDP missing ice-ufrag".into()))?
                .to_string(),
            pwd: Zeroizing::new(
                sdp.ice_pwd_for(m)
                    .ok_or_else(|| RtcError::Negotiation("remote SDP missing ice-pwd".into()))?
                    .to_string(),
            ),
            fingerprint: select_fingerprint(&sdp.fingerprints_for(m))?,
            setup: sdp.setup_for(m),
        };
        match &transport {
            Some(first) if !first.same_as(&section) => {
                return Err(RtcError::Negotiation(
                    "bundled media sections disagree on their ICE or DTLS parameters".into(),
                ));
            }
            Some(_) => {}
            None => transport = Some(section),
        }

        candidates.extend(m.candidates().into_iter().map(|c| format!("candidate:{c}")));
        end_of_candidates |= m
            .attributes
            .iter()
            .any(|a| matches!(a, Attribute::EndOfCandidates));
    }

    let transport = transport
        .ok_or_else(|| RtcError::Negotiation("remote SDP has no active media section".into()))?;
    Ok(RemoteParams {
        ice_ufrag: transport.ufrag,
        ice_pwd: transport.pwd,
        fingerprint_sha256: transport.fingerprint,
        setup: transport.setup,
        candidates,
        end_of_candidates,
    })
}

fn select_fingerprint(fingerprints: &[&Fingerprint]) -> Result<[u8; 32]> {
    let mut chosen: Option<[u8; 32]> = None;
    for fp in fingerprints
        .iter()
        .filter(|fp| fp.algorithm.eq_ignore_ascii_case("sha-256"))
    {
        let digest: [u8; 32] = fp
            .digest
            .as_slice()
            .try_into()
            .map_err(|_| RtcError::Negotiation("sha-256 fingerprint is not 32 bytes".into()))?;
        match chosen {
            None => chosen = Some(digest),
            Some(existing) if existing == digest => {}
            Some(_) => {
                return Err(RtcError::Unsupported(
                    "several sha-256 fingerprints; the DTLS transport pins exactly one certificate",
                ))
            }
        }
    }
    chosen.ok_or_else(|| {
        RtcError::Negotiation(
            "remote SDP has no sha-256 fingerprint; only sha-256 is supported".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP_A: &str = "23:3B:8A:6B:DF:BD:0B:D3:D0:13:2B:6A:F5:C5:5D:46:F7:A3:61:C0:A0:41:C6:A7:A8:2B:7D:75:12:BA:95:49";
    const FP_B: &str = "00:3B:8A:6B:DF:BD:0B:D3:D0:13:2B:6A:F5:C5:5D:46:F7:A3:61:C0:A0:41:C6:A7:A8:2B:7D:75:12:BA:95:49";

    fn sdp(session: &str, audio: &str, video: &str) -> Sdp {
        let text = format!(
            "v=0\r\no=- 1 1 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0 1\r\n{session}\
             m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\na=rtcp-mux\r\n{audio}\
             a=rtpmap:111 opus/48000/2\r\n\
             m=video 9 UDP/TLS/RTP/SAVPF 96\r\nc=IN IP4 0.0.0.0\r\na=mid:1\r\na=rtcp-mux\r\n{video}\
             a=rtpmap:96 H264/90000\r\n"
        );
        Sdp::parse(&text).unwrap()
    }

    fn transport(fp: &str) -> String {
        format!("a=ice-ufrag:uf\r\na=ice-pwd:pwpwpwpwpwpwpwpwpwpwpw\r\na=fingerprint:sha-256 {fp}\r\na=setup:actpass\r\n")
    }

    #[test]
    fn session_level_transport_is_used() {
        let params = apply_remote(&sdp(&transport(FP_A), "", "")).unwrap();
        assert_eq!(params.ice_ufrag, "uf");
        assert_eq!(params.setup, Some(SetupRole::ActPass));
        assert_eq!(params.fingerprint_sha256[0], 0x23);
    }

    #[test]
    fn bundled_sections_must_agree() {
        let err = apply_remote(&sdp("", &transport(FP_A), &transport(FP_B)));
        assert!(matches!(err, Err(RtcError::Negotiation(_))));
        assert!(apply_remote(&sdp("", &transport(FP_A), &transport(FP_A))).is_ok());
    }

    #[test]
    fn several_certificates_are_refused() {
        let both = format!("{}a=fingerprint:sha-256 {FP_B}\r\n", transport(FP_A));
        assert!(matches!(
            apply_remote(&sdp(&both, "", "")),
            Err(RtcError::Unsupported(_))
        ));
    }

    #[test]
    fn other_hash_algorithms_are_skipped() {
        let extra = format!(
            "a=fingerprint:sha-1 00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33\r\n{}",
            transport(FP_A)
        );
        let params = apply_remote(&sdp(&extra, "", "")).unwrap();
        assert_eq!(params.fingerprint_sha256[0], 0x23);
    }

    #[test]
    fn inline_candidates_and_end_of_candidates_are_collected() {
        let audio =
            "a=candidate:1 1 udp 2130706431 127.0.0.1 5000 typ host\r\na=end-of-candidates\r\n";
        let params = apply_remote(&sdp(&transport(FP_A), audio, "")).unwrap();
        assert_eq!(
            params.candidates,
            vec!["candidate:1 1 udp 2130706431 127.0.0.1 5000 typ host"]
        );
        assert!(params.end_of_candidates);
    }
}
