//! Transport-wide congestion-control (TWCC) feedback: record arrivals of
//! transport-wide sequence numbers and emit RFC 8888-style feedback packets.

use rtp::rtcp::twcc::{Delta, Twcc};
use rtp::rtcp::RtcpPacket;

/// Records arrival of transport-wide sequence numbers and builds TWCC feedback.
#[derive(Debug, Default)]
pub struct TwccRecorder {
    media_ssrc: u32,
    sender_ssrc: u32,
    base_seq: Option<u16>,
    received: Vec<(u16, i64)>,
    fb_count: u8,
}

impl TwccRecorder {
    /// Create a recorder that will attribute feedback to `sender_ssrc` for
    /// media source `media_ssrc`.
    #[must_use]
    pub fn new(sender_ssrc: u32, media_ssrc: u32) -> Self {
        Self {
            media_ssrc,
            sender_ssrc,
            base_seq: None,
            received: Vec::new(),
            fb_count: 0,
        }
    }

    /// Record arrival of `seq` at `arrival_us` microseconds (monotonic).
    pub fn record(&mut self, seq: u16, arrival_us: i64) {
        if self.base_seq.is_none() {
            self.base_seq = Some(seq);
        }
        self.received.push((seq, arrival_us));
    }

    /// Whether any arrivals are pending feedback.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.received.is_empty()
    }

    /// Build a feedback packet for the accumulated arrivals and reset the
    /// window. Returns `None` if nothing was recorded.
    pub fn build(&mut self) -> Option<RtcpPacket> {
        if self.received.is_empty() {
            return None;
        }
        self.received.sort_by_key(|&(seq, _)| seq);
        let base = self.received.first().map(|&(s, _)| s)?;
        let last = self.received.last().map(|&(s, _)| s)?;
        let ref_us = self.received.first().map(|&(_, a)| a).unwrap_or(0);

        let span = last.wrapping_sub(base) as usize + 1;
        let mut statuses = vec![Delta::NotReceived; span];
        let mut prev_us = ref_us;
        for &(seq, arrival) in &self.received {
            let idx = seq.wrapping_sub(base) as usize;
            if idx < statuses.len() {
                let delta_ticks = (arrival - prev_us) / 250;
                statuses[idx] = if (0..=255).contains(&delta_ticks) {
                    Delta::Small(delta_ticks as u8)
                } else {
                    Delta::Large(delta_ticks.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16)
                };
                prev_us = arrival;
            }
        }

        let twcc = Twcc {
            sender_ssrc: self.sender_ssrc,
            media_ssrc: self.media_ssrc,
            base_seq: base,
            reference_time: (ref_us / 64_000) as i32,
            fb_pkt_count: self.fb_count,
            statuses,
        };
        self.fb_count = self.fb_count.wrapping_add(1);
        self.received.clear();
        self.base_seq = None;
        Some(RtcpPacket::Twcc(twcc))
    }
}

/// Parse a TWCC feedback packet from RTCP bytes.
#[must_use]
#[allow(dead_code)]
pub fn parse(bytes: &[u8]) -> Option<Twcc> {
    match RtcpPacket::parse(bytes).ok()? {
        RtcpPacket::Twcc(t) => Some(t),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_builds() {
        let mut rec = TwccRecorder::new(0xAAAA, 0xBBBB);
        rec.record(0, 0);
        rec.record(1, 250);
        rec.record(2, 500);
        let pkt = rec.build().expect("feedback");
        let bytes = pkt.encode();
        let parsed = parse(&bytes).expect("round-trip");
        assert_eq!(parsed.base_seq, 0);
        assert_eq!(parsed.statuses.len(), 3);
    }
}
