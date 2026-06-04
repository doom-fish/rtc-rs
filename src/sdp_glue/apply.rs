//! Apply a remote SDP description: validate WebRTC requirements and extract
//! the transport parameters (ICE creds, fingerprint, setup, codecs, SSRCs).

use crate::error::{Result, RtcError};
use sdp::{Attribute, MediaDescription, MediaKind, Sdp, SetupRole};

/// A negotiated media section.
/// The negotiated media parameters parsed from a remote SDP m-section. Retained
/// in full for completeness; the symmetric default-config path drives the
/// transport from [`crate::config::MediaConfig`] directly.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct NegotiatedMedia {
    /// The `a=mid` value.
    pub mid: String,
    /// The remote payload type for this kind.
    pub payload_type: u8,
    /// The remote SSRC, when signaled.
    pub ssrc: Option<u32>,
    /// Negotiated MID header-extension id, if present.
    pub mid_ext_id: Option<u8>,
    /// Negotiated TWCC header-extension id, if present.
    pub twcc_ext_id: Option<u8>,
}

/// Everything `rtc` needs from a remote description to drive the transport.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RemoteParams {
    /// Remote ICE username fragment.
    pub ice_ufrag: String,
    /// Remote ICE password.
    pub ice_pwd: String,
    /// Remote DTLS fingerprint (SHA-256).
    pub fingerprint_sha256: [u8; 32],
    /// Remote `a=setup` role.
    pub setup: SetupRole,
    /// Negotiated audio section, if any.
    pub audio: Option<NegotiatedMedia>,
    /// Negotiated video section, if any.
    pub video: Option<NegotiatedMedia>,
    /// In-line ICE candidates parsed from the SDP, if any.
    pub candidates: Vec<sdp::Candidate>,
}

/// Validate the WebRTC profile requirements and pull out [`RemoteParams`].
///
/// Requirements enforced (RFC 8843 / RFC 5763 / RFC 5761):
/// * a BUNDLE group is present,
/// * every non-rejected media section has rtcp-mux,
/// * ICE ufrag/pwd and a SHA-256 fingerprint are present,
/// * `m=application` data channels are tolerated only when rejected (port 0).
pub fn apply_remote(sdp: &Sdp) -> Result<RemoteParams> {
    if sdp.bundle_group().is_none() {
        return Err(RtcError::Negotiation("remote SDP has no BUNDLE group".into()));
    }

    let mut audio = None;
    let mut video = None;
    let mut ice_ufrag = None;
    let mut ice_pwd = None;
    let mut fingerprint = None;
    let mut setup = None;
    let mut candidates = Vec::new();

    for m in &sdp.media {
        // A data channel is acceptable only when it is rejected (port 0).
        if m.kind == MediaKind::Application {
            if !m.is_rejected() && m.port != 0 {
                return Err(RtcError::Negotiation(
                    "remote offered an active data channel (m=application); only rejected \
                     data channels are supported"
                        .into(),
                ));
            }
            continue;
        }

        if m.is_rejected() || m.port == 0 {
            continue;
        }

        if !m.has_rtcp_mux() {
            return Err(RtcError::Negotiation(format!(
                "media section mid={:?} is missing a=rtcp-mux",
                m.mid()
            )));
        }

        // Session- or media-level ICE/fingerprint/setup (media-level wins).
        if let Some(u) = media_ufrag(m) {
            ice_ufrag = Some(u);
        }
        if let Some(p) = media_pwd(m) {
            ice_pwd = Some(p);
        }
        if let Some(fp) = m.fingerprint() {
            fingerprint = Some(parse_fp(&fp.algorithm, &fp.digest)?);
        }
        if let Some(s) = m.setup() {
            setup = Some(s);
        }
        for attr in &m.attributes {
            if let Attribute::Candidate(c) = attr {
                candidates.push(c.clone());
            }
        }

        let neg = negotiate_section(m);
        match m.kind {
            MediaKind::Audio => audio = Some(neg),
            MediaKind::Video => video = Some(neg),
            _ => {}
        }
    }

    // Fall back to session-level attributes.
    for attr in &sdp.attributes {
        match attr {
            Attribute::IceUfrag(u) if ice_ufrag.is_none() => ice_ufrag = Some(u.clone()),
            Attribute::IcePwd(p) if ice_pwd.is_none() => ice_pwd = Some(p.clone()),
            Attribute::Fingerprint(fp) if fingerprint.is_none() => {
                fingerprint = Some(parse_fp(&fp.algorithm, &fp.digest)?);
            }
            Attribute::Setup(s) if setup.is_none() => setup = Some(*s),
            _ => {}
        }
    }

    Ok(RemoteParams {
        ice_ufrag: ice_ufrag
            .ok_or_else(|| RtcError::Negotiation("remote SDP missing ice-ufrag".into()))?,
        ice_pwd: ice_pwd
            .ok_or_else(|| RtcError::Negotiation("remote SDP missing ice-pwd".into()))?,
        fingerprint_sha256: fingerprint
            .ok_or_else(|| RtcError::Negotiation("remote SDP missing sha-256 fingerprint".into()))?,
        setup: setup.unwrap_or(SetupRole::ActPass),
        audio,
        video,
        candidates,
    })
}

fn negotiate_section(m: &MediaDescription) -> NegotiatedMedia {
    let payload_type = m
        .rtpmaps()
        .first()
        .map_or_else(
            || m.formats.first().and_then(|f| f.parse().ok()).unwrap_or(0),
            |r| r.payload,
        );

    let mut ssrc = None;
    let mut mid_ext_id = None;
    let mut twcc_ext_id = None;
    for attr in &m.attributes {
        match attr {
            Attribute::Ssrc(s) if ssrc.is_none() => ssrc = Some(s.id),
            Attribute::Extmap(e) => {
                if e.uri == sdp::extmap::URI_MID {
                    mid_ext_id = u8::try_from(e.id).ok();
                } else if e.uri == sdp::extmap::URI_TWCC {
                    twcc_ext_id = u8::try_from(e.id).ok();
                }
            }
            _ => {}
        }
    }

    NegotiatedMedia {
        mid: m.mid().unwrap_or_default().to_string(),
        payload_type,
        ssrc,
        mid_ext_id,
        twcc_ext_id,
    }
}

fn media_ufrag(m: &MediaDescription) -> Option<String> {
    m.attributes.iter().find_map(|a| match a {
        Attribute::IceUfrag(u) => Some(u.clone()),
        _ => None,
    })
}

fn media_pwd(m: &MediaDescription) -> Option<String> {
    m.attributes.iter().find_map(|a| match a {
        Attribute::IcePwd(p) => Some(p.clone()),
        _ => None,
    })
}

fn parse_fp(algorithm: &str, digest: &[u8]) -> Result<[u8; 32]> {
    if !algorithm.eq_ignore_ascii_case("sha-256") {
        return Err(RtcError::Negotiation(format!(
            "unsupported fingerprint algorithm {algorithm:?}; only sha-256 is supported"
        )));
    }
    let arr: [u8; 32] = digest
        .try_into()
        .map_err(|_| RtcError::Negotiation("sha-256 fingerprint is not 32 bytes".into()))?;
    Ok(arr)
}
