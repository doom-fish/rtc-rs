//! GATING TEST: two in-process `PeerConnection`s complete the full WebRTC
//! handshake over loopback (SDP → trickle ICE → DTLS → SRTP) and exchange live
//! Opus audio and H.264 video.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rtc::{
    CapturedFrame, Event, MediaConfig, PeerConnection, RemoteDescription, Result, RtcConfig,
    VideoSource,
};

/// A continuous 440 Hz tone — non-blocking, the worker paces it.
struct ToneSource {
    phase: std::sync::Mutex<f32>,
}

impl ToneSource {
    fn new() -> Self {
        Self {
            phase: std::sync::Mutex::new(0.0),
        }
    }
}

impl rtc::AudioSource for ToneSource {
    fn next_block(&self, out: &mut [i16; 960]) -> Result<()> {
        let mut phase = self.phase.lock().unwrap();
        for s in out.iter_mut() {
            *s = (phase.sin() * 8000.0) as i16;
            *phase += 2.0 * std::f32::consts::PI * 440.0 / 48000.0;
        }
        Ok(())
    }
}

/// A synthetic Annex-B H.264 source emitting an IDR access unit at ~30 fps. It
/// paces itself off a wall clock and returns `None` when no frame is due yet,
/// matching the contract of a real capture source (the worker pulls with a zero
/// timeout and expects `None` when nothing is ready).
struct SyntheticVideoSource {
    ts: AtomicU32,
    next_due: std::sync::Mutex<Instant>,
}

impl SyntheticVideoSource {
    fn new() -> Self {
        Self {
            ts: AtomicU32::new(0),
            next_due: std::sync::Mutex::new(Instant::now()),
        }
    }
}

impl VideoSource for SyntheticVideoSource {
    fn next_frame(&self, _timeout: Duration) -> Option<CapturedFrame> {
        {
            let mut due = self.next_due.lock().unwrap();
            if Instant::now() < *due {
                return None;
            }
            *due += Duration::from_millis(33);
        }
        let ts = self.ts.fetch_add(3000, Ordering::Relaxed);
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80]);
        data.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        data.extend(std::iter::repeat(0xAB).take(800));
        Some(CapturedFrame {
            pixel_buffer: 0,
            width: 640,
            height: 480,
            rtp_timestamp: ts,
            keyframe: true,
            data,
        })
    }
}

fn av_config() -> RtcConfig {
    RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .video(MediaConfig::h264_constrained_baseline())
        .loopback_only(true)
        .build()
}

#[derive(Default)]
struct Seen {
    connected: bool,
    audio: bool,
    video: bool,
}

impl Seen {
    fn done(&self) -> bool {
        self.connected && self.audio && self.video
    }
}

fn pump(pc: &PeerConnection, peer: &PeerConnection, seen: &mut Seen) {
    while let Some(ev) = pc.poll_event() {
        match ev {
            Event::LocalCandidate(line) => {
                let _ = peer.add_remote_candidate_immediate(line);
            }
            Event::Connected => seen.connected = true,
            Event::AudioFrame(..) => seen.audio = true,
            Event::VideoAccessUnit(..) => seen.video = true,
            _ => {}
        }
    }
}

#[test]
fn loopback_full_stack() {
    let mut a = PeerConnection::new(av_config());
    let mut b = PeerConnection::new(av_config());

    a.add_audio_track(Arc::new(ToneSource::new()));
    a.add_video_track(Arc::new(SyntheticVideoSource::new()));
    b.add_audio_track(Arc::new(ToneSource::new()));
    b.add_video_track(Arc::new(SyntheticVideoSource::new()));

    let offer = a.create_offer().expect("offer");
    b.set_remote_description(RemoteDescription::Offer(offer))
        .expect("apply offer");
    let answer = b.create_answer().expect("answer");
    a.set_remote_description(RemoteDescription::Answer(answer))
        .expect("apply answer");

    let mut sa = Seen::default();
    let mut sb = Seen::default();

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        pump(&a, &b, &mut sa);
        pump(&b, &a, &mut sb);
        if sa.done() && sb.done() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(sa.connected && sb.connected, "both peers connected");
    assert!(sa.audio && sb.audio, "both peers received audio");
    assert!(sa.video && sb.video, "both peers received video");

    a.close();
    b.close();
}
