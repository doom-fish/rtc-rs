use rtp::rtcp::rr::ReportBlock;
use rtp::rtcp::twcc::Delta;
use rtp::rtcp::RtcpPacket;

const MAX_RTT_UNITS: u32 = 60 * 65_536;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalSsrcs {
    pub audio: Option<u32>,
    pub video: Option<u32>,
}

impl LocalSsrcs {
    fn contains(self, ssrc: u32) -> bool {
        self.audio == Some(ssrc) || self.video == Some(ssrc)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feedback {
    pub sender_reports: Vec<(u32, u32)>,
    pub audio_fraction_lost: Option<u8>,
    pub video_fraction_lost: Option<u8>,
    pub rtt_ms: Option<u32>,
    pub nacks: Vec<(u32, Vec<u16>)>,
    pub keyframe_requested: bool,
    pub remb_bps: Option<u64>,
    pub twcc_received: u64,
    pub twcc_lost: u64,
}

impl Feedback {
    fn read_blocks(&mut self, blocks: &[ReportBlock], local: LocalSsrcs, ntp_middle_now: u32) {
        for block in blocks {
            let slot = if Some(block.ssrc) == local.audio {
                &mut self.audio_fraction_lost
            } else if Some(block.ssrc) == local.video {
                &mut self.video_fraction_lost
            } else {
                continue;
            };
            *slot = Some(block.fraction_lost);
            if block.last_sr != 0 {
                let rtt = ntp_middle_now
                    .wrapping_sub(block.last_sr)
                    .wrapping_sub(block.delay_since_last_sr);
                if rtt < MAX_RTT_UNITS {
                    self.rtt_ms = u32::try_from(u64::from(rtt) * 1_000 / 65_536).ok();
                }
            }
        }
    }
}

pub fn read_feedback(
    packets: &[RtcpPacket],
    local: LocalSsrcs,
    ntp_middle_now: u32,
    last_fir_seq: &mut Option<u8>,
) -> Feedback {
    let mut feedback = Feedback::default();
    for packet in packets {
        match packet {
            RtcpPacket::SenderReport(sr) => {
                let middle =
                    u32::try_from((sr.ntp_timestamp >> 16) & 0xFFFF_FFFF).unwrap_or_default();
                feedback.sender_reports.push((sr.ssrc, middle));
                feedback.read_blocks(&sr.reports, local, ntp_middle_now);
            }
            RtcpPacket::ReceiverReport(rr) => {
                feedback.read_blocks(&rr.reports, local, ntp_middle_now)
            }
            RtcpPacket::Nack(nack) if local.contains(nack.media_ssrc) => {
                feedback
                    .nacks
                    .push((nack.media_ssrc, nack.lost_sequence_numbers()));
            }
            RtcpPacket::Pli(pli) if local.video == Some(pli.media_ssrc) => {
                feedback.keyframe_requested = true;
            }
            RtcpPacket::Fir(fir) => {
                for entry in fir.entries.iter().filter(|e| local.video == Some(e.ssrc)) {
                    if *last_fir_seq != Some(entry.seq_nr) {
                        *last_fir_seq = Some(entry.seq_nr);
                        feedback.keyframe_requested = true;
                    }
                }
            }
            RtcpPacket::Remb(remb) if remb.ssrcs.iter().any(|&s| local.contains(s)) => {
                feedback.remb_bps = Some(
                    feedback
                        .remb_bps
                        .map_or(remb.bitrate_bps, |bps| bps.min(remb.bitrate_bps)),
                );
            }
            RtcpPacket::Twcc(twcc) => {
                for status in &twcc.statuses {
                    if *status == Delta::NotReceived {
                        feedback.twcc_lost += 1;
                    } else {
                        feedback.twcc_received += 1;
                    }
                }
            }
            _ => {}
        }
    }
    feedback
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtp::rtcp::fir::{Fir, FirEntry};
    use rtp::rtcp::nack::Nack;
    use rtp::rtcp::remb::Remb;
    use rtp::rtcp::twcc::Twcc;

    const LOCAL: LocalSsrcs = LocalSsrcs {
        audio: Some(0xA),
        video: Some(0xB),
    };

    fn block(ssrc: u32, fraction_lost: u8, last_sr: u32, delay: u32) -> ReportBlock {
        ReportBlock {
            ssrc,
            fraction_lost,
            cumulative_lost: 0,
            extended_highest_seq: 0,
            jitter: 0,
            last_sr,
            delay_since_last_sr: delay,
        }
    }

    #[test]
    fn report_blocks_about_our_streams_give_loss_and_rtt() {
        let rr = RtcpPacket::receiver_report(0x99)
            .add_block(block(0xA, 26, 1_000_000, 65_536))
            .add_block(block(0xB, 64, 0, 0))
            .add_block(block(0xC, 200, 0, 0))
            .build();
        let now = 1_000_000 + 65_536 + 6_554;
        let fb = read_feedback(&[rr], LOCAL, now, &mut None);
        assert_eq!(fb.audio_fraction_lost, Some(26));
        assert_eq!(fb.video_fraction_lost, Some(64));
        assert_eq!(fb.rtt_ms, Some(100));
    }

    #[test]
    fn implausible_rtt_is_ignored() {
        let rr = RtcpPacket::receiver_report(0x99)
            .add_block(block(0xA, 0, 5_000, 0))
            .build();
        assert_eq!(read_feedback(&[rr], LOCAL, 1_000, &mut None).rtt_ms, None);
    }

    #[test]
    fn nacks_only_for_our_ssrcs() {
        let ours = RtcpPacket::Nack(Nack::from_lost(0x99, 0xB, &[5, 6, 9]));
        let theirs = RtcpPacket::Nack(Nack::from_lost(0x99, 0xC, &[1]));
        let fb = read_feedback(&[ours, theirs], LOCAL, 0, &mut None);
        assert_eq!(fb.nacks, vec![(0xB, vec![5, 6, 9])]);
    }

    #[test]
    fn pli_and_new_fir_request_a_keyframe() {
        let pli = RtcpPacket::Pli(RtcpPacket::pli(0x99, 0xB));
        assert!(read_feedback(&[pli], LOCAL, 0, &mut None).keyframe_requested);
        let audio_pli = RtcpPacket::Pli(RtcpPacket::pli(0x99, 0xA));
        assert!(!read_feedback(&[audio_pli], LOCAL, 0, &mut None).keyframe_requested);

        let fir = |seq_nr| {
            RtcpPacket::Fir(Fir {
                sender_ssrc: 0x99,
                media_ssrc: 0,
                entries: vec![FirEntry { ssrc: 0xB, seq_nr }],
            })
        };
        let mut last = None;
        assert!(read_feedback(&[fir(3)], LOCAL, 0, &mut last).keyframe_requested);
        assert!(!read_feedback(&[fir(3)], LOCAL, 0, &mut last).keyframe_requested);
        assert!(read_feedback(&[fir(4)], LOCAL, 0, &mut last).keyframe_requested);
    }

    #[test]
    fn remb_and_twcc_are_read() {
        let remb = RtcpPacket::Remb(Remb {
            sender_ssrc: 0x99,
            media_ssrc: 0,
            bitrate_bps: 750_000,
            ssrcs: vec![0xB],
        });
        let twcc = RtcpPacket::Twcc(Twcc {
            sender_ssrc: 0x99,
            media_ssrc: 0xB,
            base_seq: 10,
            reference_time: 0,
            fb_pkt_count: 0,
            statuses: vec![
                Delta::Small(1),
                Delta::NotReceived,
                Delta::Large(-3),
                Delta::NotReceived,
            ],
        });
        let fb = read_feedback(&[remb, twcc], LOCAL, 0, &mut None);
        assert_eq!(fb.remb_bps, Some(750_000));
        assert_eq!((fb.twcc_received, fb.twcc_lost), (2, 2));
    }

    #[test]
    fn sender_reports_are_collected_for_rtt_on_the_other_side() {
        let sr = rtp::rtcp::RtcpPacket::sender_report(0x77)
            .ntp_timestamp(0x1234_5678_9ABC_DEF0)
            .build();
        let fb = read_feedback(&[sr], LOCAL, 0, &mut None);
        assert_eq!(fb.sender_reports, vec![(0x77, 0x5678_9ABC)]);
    }
}
