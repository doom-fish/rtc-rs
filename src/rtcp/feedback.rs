//! RTCP feedback construction (NACK / PLI / FIR) and loss detection.

use rtp::rtcp::nack::Nack;
use rtp::rtcp::RtcpPacket;
use rtp::seq::{seq_diff, seq_greater_than};
use std::collections::BTreeSet;

pub const MAX_NACK_GAP: u16 = 64;
const MAX_TRACKED_AGE: i32 = 512;

/// Build a generic NACK (RFC 4585) for `media_ssrc` listing `lost` sequence
/// numbers.
#[must_use]
pub fn nack(sender_ssrc: u32, media_ssrc: u32, lost: &[u16]) -> RtcpPacket {
    RtcpPacket::Nack(Nack::from_lost(sender_ssrc, media_ssrc, lost))
}

/// Build a Picture Loss Indication (RFC 4585 §6.3.1) for `media_ssrc`.
#[must_use]
pub fn pli(sender_ssrc: u32, media_ssrc: u32) -> RtcpPacket {
    RtcpPacket::Pli(RtcpPacket::pli(sender_ssrc, media_ssrc))
}

/// Build a Full Intra Request (RFC 5104 §4.3.1) for `media_ssrc`.
#[must_use]
#[allow(dead_code)]
pub fn fir(sender_ssrc: u32, media_ssrc: u32, seq: u8) -> RtcpPacket {
    RtcpPacket::Fir(RtcpPacket::fir(sender_ssrc, media_ssrc, seq))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    InOrder,
    Lost(Vec<u16>),
    Discontinuity,
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

    /// Observe a received sequence number. Reports sequence numbers that
    /// became newly missing (a forward gap), which the caller should NACK.
    pub fn observe(&mut self, seq: u16) -> Observation {
        let Some(highest) = self.highest else {
            self.highest = Some(seq);
            return Observation::InOrder;
        };

        if self.missing.remove(&seq) {
            // A previously missing packet was recovered (late arrival).
            return Observation::InOrder;
        }
        if !seq_greater_than(seq, highest) {
            return Observation::InOrder;
        }

        self.highest = Some(seq);
        let gap = seq.wrapping_sub(highest).wrapping_sub(1);
        if gap > MAX_NACK_GAP {
            self.missing.clear();
            return Observation::Discontinuity;
        }
        let mut newly = Vec::with_capacity(usize::from(gap));
        let mut s = highest.wrapping_add(1);
        while s != seq {
            self.missing.insert(s);
            newly.push(s);
            s = s.wrapping_add(1);
        }
        self.missing.retain(|&m| seq_diff(m, seq) < MAX_TRACKED_AGE);
        if newly.is_empty() {
            Observation::InOrder
        } else {
            Observation::Lost(newly)
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
        assert_eq!(t.observe(1), Observation::InOrder);
        assert_eq!(t.observe(2), Observation::InOrder);
        assert_eq!(t.observe(4), Observation::Lost(vec![3]));
        assert_eq!(t.outstanding(), vec![3]);
    }

    #[test]
    fn recovers_late_packet() {
        let mut t = LossTracker::new();
        t.observe(1);
        t.observe(4);
        assert_eq!(t.outstanding(), vec![2, 3]);
        assert_eq!(t.observe(2), Observation::InOrder);
        assert_eq!(t.outstanding(), vec![3]);
    }

    #[test]
    fn gaps_across_the_wrap_are_reported() {
        let mut t = LossTracker::new();
        t.observe(65_534);
        assert_eq!(t.observe(1), Observation::Lost(vec![65_535, 0]));
    }

    #[test]
    fn large_jumps_are_a_discontinuity_not_a_nack_storm() {
        let mut t = LossTracker::new();
        t.observe(10);
        t.observe(12);
        assert_eq!(t.observe(10_012), Observation::Discontinuity);
        assert!(t.outstanding().is_empty());
        assert_eq!(t.observe(10_013), Observation::InOrder);
    }

    #[test]
    fn old_losses_are_forgotten() {
        let mut t = LossTracker::new();
        t.observe(0);
        t.observe(2);
        let mut seq = 2u16;
        for _ in 0..600 {
            seq = seq.wrapping_add(1);
            t.observe(seq);
        }
        assert!(t.outstanding().is_empty());
    }

    #[test]
    fn nack_packet_lists_lost() {
        let pkt = nack(0x1111, 0x1234, &[3, 7]);
        let bytes = pkt.encode().unwrap();
        match RtcpPacket::parse(&bytes).unwrap() {
            RtcpPacket::Nack(n) => {
                assert_eq!(n.sender_ssrc, 0x1111);
                assert_eq!(n.media_ssrc, 0x1234);
                assert_eq!(n.lost_sequence_numbers(), vec![3, 7]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
