//! Demultiplex SRTP from SRTCP on a single rtcp-muxed 5-tuple (RFC 5761 §4).
//!
//! ICE's [`ice::classify`] already tells us a datagram is RTP-family (first
//! byte `128..=191`). Within that family, RTP and RTCP are told apart by the
//! payload-type byte: RTCP packet types fall in the `64..=95` masked range,
//! which the RFC reserves so the two never collide.

/// `true` when a muxed RTP-family datagram is SRTCP rather than SRTP.
#[must_use]
pub fn is_rtcp(packet: &[u8]) -> bool {
    if packet.len() < 2 {
        return false;
    }
    let pt = packet[1] & 0x7f;
    (64..=95).contains(&pt)
}

/// The muxed media kind of an RTP-family datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuxedKind {
    /// Media (SRTP).
    Rtp,
    /// Control (SRTCP).
    Rtcp,
}

/// Classify a muxed RTP-family datagram.
#[must_use]
pub fn muxed_kind(packet: &[u8]) -> MuxedKind {
    if is_rtcp(packet) {
        MuxedKind::Rtcp
    } else {
        MuxedKind::Rtp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtp_pt96_is_media() {
        // V=2, PT=96 (0x60) — a typical dynamic media payload type.
        let pkt = [0x80, 0x60, 0, 0, 0, 0, 0, 0];
        assert_eq!(muxed_kind(&pkt), MuxedKind::Rtp);
    }

    #[test]
    fn rtcp_sr_is_control() {
        // V=2, PT=200 (0xC8) — Sender Report.
        let pkt = [0x80, 0xC8, 0, 6];
        assert_eq!(muxed_kind(&pkt), MuxedKind::Rtcp);
    }

    #[test]
    fn rtcp_psfb_is_control() {
        // PT=206 (0xCE) — Payload-Specific Feedback (PLI/REMB).
        let pkt = [0x81, 0xCE, 0, 3];
        assert!(is_rtcp(&pkt));
    }
}
