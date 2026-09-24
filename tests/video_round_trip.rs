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

#[test]
fn oversized_access_units_are_dropped_not_buffered() {
    use rtc::media::video_recv::MAX_ACCESS_UNIT_BYTES;
    let ssrc = 0x4242;
    let mut receiver = VideoReceiver::new(ssrc);
    let nal_len = 1024 * 1024;
    let packet = |seq: u16, ts: u32, marker: bool, len: usize| {
        let mut payload = vec![0xAB; len];
        payload[0] = 0x41;
        rtp::RtpPacket::builder()
            .payload_type(96)
            .sequence_number(seq)
            .timestamp(ts)
            .ssrc(ssrc)
            .marker(marker)
            .payload(payload)
            .build()
            .expect("rtp")
    };
    let mut units = Vec::new();
    let oversized = MAX_ACCESS_UNIT_BYTES / nal_len + 2;
    let mut seq = 1u16;
    for _ in 0..oversized {
        let pkt = packet(seq, 1000, false, nal_len);
        receiver.push(seq, pkt.into_bytes());
        units.extend(receiver.drain());
        seq += 1;
    }
    let pkt = packet(seq, 1000, true, nal_len);
    receiver.push(seq, pkt.into_bytes());
    seq += 1;
    let next = packet(seq, 4000, true, 64);
    receiver.push(seq, next.into_bytes());
    units.extend(receiver.drain());
    units.extend(receiver.flush());

    assert_eq!(units.len(), 1, "the oversized access unit is discarded");
    assert_eq!(units[0].rtp_timestamp, 4000);
    assert!(units[0].annex_b.len() < 128);
}
