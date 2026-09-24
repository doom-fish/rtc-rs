//! Build a local SDP offer from [`RtcConfig`].

use super::{LocalSdpIdentity, MID_EXT_ID, TWCC_EXT_ID};
use crate::config::{Codec, MediaConfig, RtcConfig};
use sdp::{
    Attribute, Direction, Extmap, Fingerprint, Fmtp, Group, MediaDescription, MediaKind, Offer,
    Origin, RtcpFb, RtpMap, SetupRole, Sdp, Ssrc, Timing,
};

/// Build an `Offer` advertising audio and/or video per `cfg`, all under one
/// BUNDLE group with rtcp-mux, announcing the offerer's outbound `ssrcs`.
#[must_use]
pub fn build_offer(
    cfg: &RtcConfig,
    identity: &LocalSdpIdentity,
    ssrcs: OfferSsrcs,
    session_id: u64,
) -> Offer {
    let mut media = Vec::new();
    let mut mids = Vec::new();

    if let (Some(audio), Some(ssrc)) = (&cfg.audio, ssrcs.audio) {
        media.push(media_section("0", audio, identity, ssrc, SetupRole::ActPass));
        mids.push("0".to_string());
    }
    if let (Some(video), Some(ssrc)) = (&cfg.video, ssrcs.video) {
        media.push(media_section("1", video, identity, ssrc, SetupRole::ActPass));
        mids.push("1".to_string());
    }

    let session_id = format!("{}", session_id >> 1);
    let sdp = Sdp {
        version: 0,
        origin: Origin {
            username: "-".to_string(),
            session_id,
            session_version: "1".to_string(),
            net_type: "IN".to_string(),
            addr_type: "IP4".to_string(),
            unicast_address: "0.0.0.0".to_string(),
        },
        session_name: "-".to_string(),
        information: None,
        uri: None,
        emails: Vec::new(),
        phones: Vec::new(),
        connection: None,
        bandwidths: Vec::new(),
        timing: vec![Timing::permanent()],
        time_zones: None,
        key: None,
        attributes: vec![
            Attribute::Group(Group::bundle(mids)),
            Attribute::IceOptions("trickle".to_string()),
        ],
        media,
    };

    Offer(sdp)
}

/// SSRCs assigned to the offerer's outbound streams.
#[derive(Debug, Clone, Copy, Default)]
pub struct OfferSsrcs {
    /// Outbound audio SSRC.
    pub audio: Option<u32>,
    /// Outbound video SSRC.
    pub video: Option<u32>,
}

fn media_section(
    mid: &str,
    mc: &MediaConfig,
    identity: &LocalSdpIdentity,
    ssrc: u32,
    setup: SetupRole,
) -> MediaDescription {
    let kind = match mc.codec {
        Codec::Opus => MediaKind::Audio,
        Codec::H264 => MediaKind::Video,
    };
    let mut m = MediaDescription::new(kind, 9, "UDP/TLS/RTP/SAVPF");
    m.formats = vec![mc.payload_type.to_string()];
    m.connection = Some(sdp::Connection {
        net_type: "IN".to_string(),
        addr_type: "IP4".to_string(),
        address: "0.0.0.0".to_string(),
    });

    m.push_attribute(Attribute::Mid(mid.to_string()));
    m.push_attribute(Attribute::Direction(Direction::SendRecv));
    m.push_attribute(Attribute::RtcpMux);
    m.push_attribute(Attribute::IceUfrag(identity.ice_ufrag.clone()));
    m.push_attribute(Attribute::IcePwd(identity.ice_pwd.to_string()));
    m.push_attribute(Attribute::IceOptions("trickle".to_string()));
    m.push_attribute(Attribute::Fingerprint(Fingerprint::new(
        "sha-256",
        identity.fingerprint_sha256.to_vec(),
    )));
    m.push_attribute(Attribute::Setup(setup));
    m.push_attribute(Attribute::Extmap(Extmap::new(
        u16::from(MID_EXT_ID),
        sdp::extmap::URI_MID,
    )));
    m.push_attribute(Attribute::Extmap(Extmap::new(
        u16::from(TWCC_EXT_ID),
        sdp::extmap::URI_TWCC,
    )));

    let channels = if mc.channels > 0 { Some(u32::from(mc.channels)) } else { None };
    m.push_attribute(Attribute::RtpMap(RtpMap {
        payload: mc.payload_type,
        encoding: mc.encoding_name().to_string(),
        clock_rate: mc.clock_rate,
        channels,
    }));
    m.push_attribute(Attribute::Fmtp(Fmtp {
        format: mc.payload_type.to_string(),
        params: mc.fmtp.clone(),
    }));
    for feedback in supported_feedback(mc.codec) {
        m.push_attribute(Attribute::RtcpFb(RtcpFb {
            format: mc.payload_type.to_string(),
            feedback: (*feedback).to_string(),
        }));
    }

    m.push_attribute(Attribute::Ssrc(Ssrc {
        id: ssrc,
        attribute: "cname".to_string(),
        value: Some("rtc-rs".to_string()),
    }));
    m
}

pub(crate) fn supported_feedback(codec: Codec) -> &'static [&'static str] {
    match codec {
        Codec::Opus => &["transport-cc", "nack"],
        Codec::H264 => &["transport-cc", "nack", "nack pli", "ccm fir"],
    }
}
