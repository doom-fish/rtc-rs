//! Every outbound RTP packet carries a transport-wide congestion-control
//! sequence number, drawn from a single shared counter across audio and video
//! so the values increase by exactly one per packet on the wire.

use rtc::media::{AudioSender, SeqCounter, VideoSender, SAMPLES_PER_FRAME};
use rtc::{CapturedFrame, MediaConfig};

fn twcc_of(pkt: &rtp::RtpPacketOwned) -> u16 {
    pkt.extensions()
        .iter()
        .find_map(|e| e.as_twcc())
        .expect("packet carries a twcc extension")
}

#[test]
fn shared_counter_stamps_consecutive_seqs() {
    let audio_cfg = MediaConfig::opus_default();
    let video_cfg = MediaConfig::h264_constrained_baseline();
    let mut audio = AudioSender::new(&audio_cfg, 1, "0".to_string(), 1, 3).expect("audio");
    let mut video = VideoSender::new(&video_cfg, 2, "1".to_string(), 1, 3);
    let mut twcc = SeqCounter::new();

    let silence = [0i16; SAMPLES_PER_FRAME];
    let mut seqs = Vec::new();

    let a0 = audio.encode(&silence, &mut twcc).expect("a0");
    seqs.push(twcc_of(&a0));

    let frame = CapturedFrame {
        pixel_buffer: 0,
        width: 320,
        height: 240,
        rtp_timestamp: 3000,
        keyframe: true,
        data: vec![0, 0, 0, 1, 0x65, 1, 2, 3, 4, 5],
    };
    for pkt in video.encode(&frame, &mut twcc) {
        seqs.push(twcc_of(&pkt));
    }

    let a1 = audio.encode(&silence, &mut twcc).expect("a1");
    seqs.push(twcc_of(&a1));

    assert!(seqs.len() >= 3, "collected several packets");
    for win in seqs.windows(2) {
        assert_eq!(
            win[1],
            win[0].wrapping_add(1),
            "twcc sequence increments by one across all media: {seqs:?}"
        );
    }
}
