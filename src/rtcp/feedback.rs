//! RTCP feedback construction (NACK / PLI / FIR) and loss detection.

use rtp::rtcp::RtcpPacket;
use rtp::seq::seq_greater_than;
use std::collections::BTreeSet;

/// Build a generic NACK (RFC 4585) for `media_ssrc` listing `lost` sequence
/// numbers.
#[must_use]
pub fn nack(media_ssrc: u32, lost: &[u16]) -> RtcpPacket {
    RtcpPacket::Nack(RtcpPacket::nack(media_ssrc, lost))
}

/// Build a Picture Loss Indication (RFC 4585 §6.3.1) for `media_ssrc`.
#[must_use]
pub fn pli(media_ssrc: u32) -> RtcpPacket {
    RtcpPacket::Pli(RtcpPacket::pli(media_ssrc))
}

/// Build a Full Intra Request (RFC 5104 §4.3.1) for `media_ssrc`.
#[must_use]
#[allow(dead_code)]
pub fn fir(media_ssrc: u32, seq: u8) -> RtcpPacket {
    RtcpPacket::Fir(RtcpPacket::fir(media_ssrc, seq))
}

/// Tracks per-SSRC sequence-number gaps to drive NACK generation.
#[derive(Debug, Default)]
pub struct LossTracker {
    highest: Option<u16>,
    missing: BTreeSet<u16>,
}

impl LossTracker {
    /// Create an empty tracker.
    #[must_use]
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe a received sequence number. Returns sequence numbers that became
    /// newly missing (a forward gap), which the caller should NACK.
    pub fn observe(&mut self, seq: u16) -> Vec<u16> {
        let Some(highest) = self.highest else {
            self.highest = Some(seq);
            return Vec::new();
        };

        if self.missing.remove(&seq) {
            // A previously missing packet was recovered (late arrival).
            return Vec::new();
        }

        if seq_greater_than(seq, highest) {
            let mut newly = Vec::new();
            let mut s = highest.wrapping_add(1);
            while s != seq {
                self.missing.insert(s);
                newly.push(s);
                s = s.wrapping_add(1);
            }
            self.highest = Some(seq);
            newly
        } else {
            Vec::new()
        }
    }

    /// The set of currently outstanding (missing) sequence numbers.
    #[must_use]
    #[allow(dead_code)]
    pub fn outstanding(&self) -> Vec<u16> {
        self.missing.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_single_gap() {
        let mut t = LossTracker::new();
        assert!(t.observe(1).is_empty());
        assert!(t.observe(2).is_empty());
        assert_eq!(t.observe(4), vec![3]);
        assert_eq!(t.outstanding(), vec![3]);
    }

    #[test]
    fn recovers_late_packet() {
        let mut t = LossTracker::new();
        t.observe(1);
        t.observe(4);
        assert_eq!(t.outstanding(), vec![2, 3]);
        assert!(t.observe(2).is_empty());
        assert_eq!(t.outstanding(), vec![3]);
    }

    #[test]
    fn nack_packet_lists_lost() {
        let pkt = nack(0x1234, &[3, 7]);
        let bytes = pkt.encode();
        assert!(!bytes.is_empty());
    }
}
