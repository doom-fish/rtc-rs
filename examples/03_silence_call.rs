//! A loopback call driven by [`SilenceAudioSource`] — the simplest possible
//! audio-only call, useful for smoke-testing the pipeline without a microphone.
//!
//! Run with: `cargo run --example 03_silence_call`

use std::sync::Arc;
use std::time::{Duration, Instant};

use rtc::{
    Event, MediaConfig, PeerConnection, RemoteDescription, RtcConfig, SilenceAudioSource,
};

fn config() -> RtcConfig {
    RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .loopback_only(true)
        .build()
}

fn main() {
    let mut a = PeerConnection::new(config());
    let mut b = PeerConnection::new(config());

    a.add_audio_track(Arc::new(SilenceAudioSource));
    b.add_audio_track(Arc::new(SilenceAudioSource));

    let offer = a.create_offer().expect("offer");
    b.set_remote_description(RemoteDescription::Offer(offer))
        .expect("apply offer");
    let answer = b.create_answer().expect("answer");
    a.set_remote_description(RemoteDescription::Answer(answer))
        .expect("apply answer");

    let (mut a_frames, mut b_frames) = (0u32, 0u32);
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        while let Some(ev) = a.poll_event() {
            match ev {
                Event::LocalCandidate(l) => {
                    let _ = b.add_remote_candidate_immediate(l);
                }
                Event::AudioFrame(..) => a_frames += 1,
                _ => {}
            }
        }
        while let Some(ev) = b.poll_event() {
            match ev {
                Event::LocalCandidate(l) => {
                    let _ = a.add_remote_candidate_immediate(l);
                }
                Event::AudioFrame(..) => b_frames += 1,
                _ => {}
            }
        }
        if a_frames > 10 && b_frames > 10 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    println!("Silence call exchanged: A={a_frames} frames, B={b_frames} frames");
    a.close();
    b.close();
}
