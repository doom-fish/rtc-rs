//! RTCP sender/receiver report construction.

use rtp::rtcp::rr::ReportBlock;
use rtp::rtcp::RtcpPacket;

/// Build a Sender Report (RFC 3550 §6.4.1).
#[must_use]
pub fn sender_report(
    ssrc: u32,
    ntp_timestamp: u64,
    rtp_timestamp: u32,
    packet_count: u32,
    octet_count: u32,
    blocks: Vec<ReportBlock>,
) -> RtcpPacket {
    let mut b = RtcpPacket::sender_report(ssrc)
        .ntp_timestamp(ntp_timestamp)
        .rtp_timestamp(rtp_timestamp)
        .counts(packet_count, octet_count);
    for block in blocks {
        b = b.add_block(block);
    }
    b.build()
}

/// Build a Receiver Report (RFC 3550 §6.4.2).
#[must_use]
pub fn receiver_report(ssrc: u32, blocks: Vec<ReportBlock>) -> RtcpPacket {
    let mut b = RtcpPacket::receiver_report(ssrc);
    for block in blocks {
        b = b.add_block(block);
    }
    b.build()
}

/// NTP timestamp (64-bit fixed point, seconds since 1900) for a Unix-epoch
/// duration.
#[must_use]
pub fn ntp_from_unix(secs: u64, nanos: u32) -> u64 {
    const UNIX_TO_NTP: u64 = 2_208_988_800;
    let ntp_secs = secs + UNIX_TO_NTP;
    let frac = ((u64::from(nanos)) << 32) / 1_000_000_000;
    (ntp_secs << 32) | (frac & 0xFFFF_FFFF)
}
