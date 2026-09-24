//! H.264 Annex-B packetize → RTP → depacketize round-trip. Confirms an access
//! unit split across multiple RTP packets is reassembled to its original NALs.

use rtc::media::{SeqCounter, VideoReceiver, VideoSender};
use rtc::{CapturedFrame, MediaConfig};
use rtp::Packet;

fn annexb_access_unit() -> Vec<u8> {
    let mut data = Vec::new();
    // SPS
    data.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f]);
    // PPS
    data.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80]);
    // IDR slice with a chunky payload so it spans multiple RTP packets.
    data.extend_from_slice(&[0, 0, 0, 1, 0x65]);
    data.extend(std::iter::repeat_n(0xAB, 4000));
    data
}

#[test]
fn h264_access_unit_round_trip() {
    let cfg = MediaConfig::h264_constrained_baseline();
    let ssrc = 0x9999;
    let mut sender = VideoSender::new(&cfg, ssrc, "1".to_string(), Some(1), Some(3));
    let mut receiver = VideoReceiver::new(ssrc);
    let mut twcc = SeqCounter::new();

    let frame = CapturedFrame {
        width: 1280,
        height: 720,
        rtp_timestamp: 9000,
        keyframe: true,
        data: annexb_access_unit(),
    };

    let packets = sender.encode(&frame, &mut twcc).expect("packetize");
    assert!(
        packets.len() > 1,
        "large access unit should span multiple RTP packets"
    );
    assert!(
        packets.last().unwrap().header().marker(),
        "final packet sets the marker bit"
    );

    let mut units = Vec::new();
    for pkt in packets {
        let seq = pkt.header().sequence_number();
        receiver.push(seq, pkt.into_bytes());
        units.extend(receiver.drain());
    }
    // End of stream: release whatever is still held below the jitter target.
    units.extend(receiver.flush());

    assert_eq!(units.len(), 1, "one access unit reassembled");
    let au = &units[0];
    assert_eq!(au.ssrc, ssrc);
    assert_eq!(au.rtp_timestamp, 9000);
    // The reassembled Annex-B should carry the SPS, PPS and IDR NAL types.
    assert!(au.annex_b.windows(2).any(|w| w[1] == 0x67), "has SPS");
    assert!(au.annex_b.windows(2).any(|w| w[1] == 0x68), "has PPS");
    assert!(au.annex_b.windows(2).any(|w| w[1] == 0x65), "has IDR");
}
