//! Transport-wide congestion-control (TWCC) feedback: record arrivals of
//! transport-wide sequence numbers and emit RFC 8888-style feedback packets.

use rtp::rtcp::twcc::{Delta, Twcc, MAX_STATUSES};
use rtp::rtcp::RtcpPacket;
use rtp::seq::seq_diff;
use std::collections::BTreeMap;

const TICK_US: i64 = 250;
const REFERENCE_TICKS: i64 = 256;
const MAX_PENDING: usize = MAX_STATUSES;

/// Records arrival of transport-wide sequence numbers and builds TWCC feedback.
#[derive(Debug, Default)]
pub struct TwccRecorder {
    media_ssrc: u32,
    sender_ssrc: u32,
    highest: Option<i64>,
    received: BTreeMap<i64, i64>,
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
            ..Self::default()
        }
    }

    fn unwrap(&mut self, seq: u16) -> i64 {
        let Some(highest) = self.highest else {
            self.highest = Some(i64::from(seq));
            return i64::from(seq);
        };
        let reference = u16::try_from(highest.rem_euclid(0x1_0000)).unwrap_or_default();
        let extended = highest + i64::from(seq_diff(reference, seq));
        self.highest = Some(highest.max(extended));
        extended
    }

    /// Record arrival of `seq` at `arrival_us` microseconds (monotonic).
    pub fn record(&mut self, seq: u16, arrival_us: i64) {
        let extended = self.unwrap(seq);
        if self.received.len() >= MAX_PENDING && !self.received.contains_key(&extended) {
            self.received.pop_first();
        }
        self.received.entry(extended).or_insert(arrival_us);
    }

    /// Whether any arrivals are pending feedback.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.received.is_empty()
    }

    /// Build feedback packets for the accumulated arrivals and reset the
    /// window.
    pub fn build(&mut self) -> Vec<RtcpPacket> {
        let received = std::mem::take(&mut self.received);
        let mut packets = Vec::new();
        let mut current: Option<(Twcc, i64, i64)> = None;
        for (extended, arrival_us) in received {
            let ticks = arrival_us.div_euclid(TICK_US);
            if let Some((twcc, base, prev_ticks)) = current.as_mut() {
                let span = usize::try_from(extended - *base).unwrap_or(usize::MAX);
                match i16::try_from(ticks - *prev_ticks) {
                    Ok(delta) if span < MAX_STATUSES => {
                        twcc.statuses.resize(span, Delta::NotReceived);
                        twcc.statuses.push(encode_delta(delta));
                        *prev_ticks = ticks;
                        continue;
                    }
                    _ => {}
                }
            }
            if let Some((twcc, _, _)) = current.take() {
                packets.push(RtcpPacket::Twcc(twcc));
            }
            let reference = ticks.div_euclid(REFERENCE_TICKS);
            let delta = ticks - reference * REFERENCE_TICKS;
            let twcc = Twcc {
                sender_ssrc: self.sender_ssrc,
                media_ssrc: self.media_ssrc,
                base_seq: u16::try_from(extended.rem_euclid(0x1_0000)).unwrap_or_default(),
                reference_time: i32::try_from(reference).unwrap_or_default(),
                fb_pkt_count: self.fb_count,
                statuses: vec![encode_delta(i16::try_from(delta).unwrap_or_default())],
            };
            self.fb_count = self.fb_count.wrapping_add(1);
            current = Some((twcc, extended, ticks));
        }
        if let Some((twcc, _, _)) = current {
            packets.push(RtcpPacket::Twcc(twcc));
        }
        packets
    }
}

fn encode_delta(ticks: i16) -> Delta {
    match u8::try_from(ticks) {
        Ok(small) => Delta::Small(small),
        Err(_) => Delta::Large(ticks),
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

    fn round_trip(pkt: &RtcpPacket) -> Twcc {
        parse(&pkt.encode().expect("encode")).expect("round-trip")
    }

    fn arrivals(twcc: &Twcc) -> Vec<(u16, i64)> {
        let mut ticks = i64::from(twcc.reference_time) * REFERENCE_TICKS;
        let mut out = Vec::new();
        for (i, status) in twcc.statuses.iter().enumerate() {
            let delta = match status {
                Delta::NotReceived => continue,
                Delta::Small(d) => i64::from(*d),
                Delta::Large(d) => i64::from(*d),
            };
            ticks += delta;
            let seq = twcc.base_seq.wrapping_add(u16::try_from(i).unwrap());
            out.push((seq, ticks * TICK_US));
        }
        out
    }

    #[test]
    fn records_and_builds() {
        let mut rec = TwccRecorder::new(0xAAAA, 0xBBBB);
        rec.record(0, 0);
        rec.record(1, 250);
        rec.record(2, 500);
        let packets = rec.build();
        assert_eq!(packets.len(), 1);
        let parsed = round_trip(&packets[0]);
        assert_eq!(parsed.base_seq, 0);
        assert_eq!(parsed.statuses.len(), 3);
        assert!(!rec.has_pending());
    }

    #[test]
    fn sequence_numbers_wrap_into_one_short_report() {
        let mut rec = TwccRecorder::new(1, 2);
        let seqs: Vec<u16> = (65_530..=65_535).chain(0..=5).collect();
        for (i, &seq) in seqs.iter().enumerate() {
            rec.record(seq, 1_000_000 + i64::try_from(i).unwrap() * 1_000);
        }
        let packets = rec.build();
        assert_eq!(packets.len(), 1);
        let parsed = round_trip(&packets[0]);
        assert_eq!(parsed.base_seq, 65_530);
        assert_eq!(parsed.statuses.len(), 12);
        assert!(parsed.statuses.iter().all(|s| *s != Delta::NotReceived));
    }

    #[test]
    fn reordering_and_loss_across_the_wrap() {
        let mut rec = TwccRecorder::new(1, 2);
        rec.record(1, 3_000);
        rec.record(65_534, 1_000);
        rec.record(0, 2_500);
        rec.record(65_534, 9_999);
        let parsed = round_trip(&rec.build()[0]);
        assert_eq!(parsed.base_seq, 65_534);
        assert_eq!(
            parsed
                .statuses
                .iter()
                .map(|s| *s != Delta::NotReceived)
                .collect::<Vec<_>>(),
            vec![true, false, true, true]
        );
        assert_eq!(
            arrivals(&parsed),
            vec![(65_534, 1_000), (0, 2_500), (1, 3_000)]
        );
    }

    #[test]
    fn deltas_reconstruct_arrival_times_exactly() {
        let mut rec = TwccRecorder::new(1, 2);
        let times = [
            64_123_250_i64,
            64_124_000,
            64_123_500,
            64_190_000,
            64_190_250,
        ];
        for (i, t) in times.iter().enumerate() {
            rec.record(u16::try_from(100 + i).unwrap(), *t);
        }
        let parsed = round_trip(&rec.build()[0]);
        let expected: Vec<(u16, i64)> = times
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    u16::try_from(100 + i).unwrap(),
                    t.div_euclid(TICK_US) * TICK_US,
                )
            })
            .collect();
        assert_eq!(arrivals(&parsed), expected);
    }

    #[test]
    fn long_gaps_and_huge_deltas_split_the_feedback() {
        let mut rec = TwccRecorder::new(1, 2);
        rec.record(10, 0);
        rec.record(11, 10_000_000);
        rec.record(11 + 20_000, 10_000_250);
        let packets = rec.build();
        assert_eq!(packets.len(), 2);
        let first = round_trip(&packets[0]);
        let second = round_trip(&packets[1]);
        assert_eq!(first.statuses.len(), 1);
        assert_eq!(second.base_seq, 11);
        assert_eq!(second.statuses.len(), 20_001);
        assert_ne!(first.fb_pkt_count, second.fb_pkt_count);
    }
}
