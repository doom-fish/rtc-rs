//! Opus encode → RTP → jitter-buffered decode round-trip. Confirms the audio
//! send/receive pipeline produces PCM at the negotiated clock rate.

use rtc::media::{AudioReceiver, AudioSender, SeqCounter, SAMPLES_PER_FRAME};
use rtc::MediaConfig;
use rtp::Packet;

fn tone(phase: &mut f32) -> [i16; SAMPLES_PER_FRAME] {
    let mut buf = [0i16; SAMPLES_PER_FRAME];
    for s in buf.iter_mut() {
        *s = (phase.sin() * 8000.0) as i16;
        *phase += 2.0 * std::f32::consts::PI * 440.0 / 48000.0;
    }
    buf
}

#[test]
fn opus_round_trip_yields_pcm() {
    let cfg = MediaConfig::opus_default();
    let ssrc = 0x1234_5678;
    let mut sender = AudioSender::new(&cfg, ssrc, "0".to_string(), Some(1), Some(3)).expect("sender");
    let mut receiver = AudioReceiver::new(&cfg, ssrc).expect("receiver");
    let mut twcc = SeqCounter::new();

    let mut phase = 0.0f32;
    let mut decoded = Vec::new();
    for _ in 0..10 {
        let frame = tone(&mut phase);
        let pkt = sender.encode(&frame, &mut twcc).expect("encode");
        let seq = pkt.header().sequence_number();
        receiver.push(seq, pkt.into_bytes());
        decoded.extend(receiver.drain());
    }

    assert!(!decoded.is_empty(), "decoder produced audio");
    let first = &decoded[0];
    assert_eq!(first.ssrc, ssrc);
    assert_eq!(first.sample_rate, 48_000);
    assert!(!first.pcm.is_empty(), "decoded PCM is non-empty");
}
