//! A full in-process loopback call: two `PeerConnection`s negotiate, connect
//! over loopback ICE/DTLS/SRTP, and exchange synthetic audio + video until each
//! has received media, then shut down.
//!
//! Run with: `cargo run --example 02_loopback`

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rtc::{
    AudioSource, CapturedFrame, Event, MediaConfig, PeerConnection, RemoteDescription, Result,
    RtcConfig, VideoSource,
};

struct Tone {
    phase: std::sync::Mutex<f32>,
}

impl AudioSource for Tone {
    fn next_block(&self, out: &mut [i16; 960]) -> Result<()> {
        let mut p = self.phase.lock().unwrap();
        for s in out.iter_mut() {
            *s = (p.sin() * 8000.0) as i16;
            *p += 2.0 * std::f32::consts::PI * 440.0 / 48000.0;
        }
        Ok(())
    }
}

struct Frames {
    ts: AtomicU32,
    next_due: std::sync::Mutex<Instant>,
}

impl VideoSource for Frames {
    fn next_frame(&self, _t: Duration) -> Option<CapturedFrame> {
        {
            let mut due = self.next_due.lock().unwrap();
            if Instant::now() < *due {
                return None;
            }
            *due += Duration::from_millis(33);
        }
        let ts = self.ts.fetch_add(3000, Ordering::Relaxed);
        let mut data = vec![0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80];
        data.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        data.extend(std::iter::repeat_n(0xAB, 800));
        Some(CapturedFrame {
            width: 640,
            height: 480,
            rtp_timestamp: ts,
            keyframe: true,
            data,
        })
    }
}

fn config() -> RtcConfig {
    RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .video(MediaConfig::h264_constrained_baseline())
        .loopback_only(true)
        .build()
}

fn main() {
    let mut a = PeerConnection::new(config()).expect("peer a");
    let mut b = PeerConnection::new(config()).expect("peer b");

    for pc in [&mut a, &mut b] {
        pc.add_audio_track(Arc::new(Tone {
            phase: std::sync::Mutex::new(0.0),
        }))
        .expect("audio track");
        pc.add_video_track(Arc::new(Frames {
            ts: AtomicU32::new(0),
            next_due: std::sync::Mutex::new(Instant::now()),
        }))
        .expect("video track");
    }

    let offer = a.create_offer().expect("offer");
    b.set_remote_description(RemoteDescription::Offer(offer))
        .expect("apply offer");
    let answer = b.create_answer().expect("answer");
    a.set_remote_description(RemoteDescription::Answer(answer))
        .expect("apply answer");

    let (mut a_audio, mut a_video, mut b_audio, mut b_video) = (0u32, 0u32, 0u32, 0u32);
    let (mut a_conn, mut b_conn) = (false, false);

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        while let Some(ev) = a.poll_event() {
            match ev {
                Event::LocalCandidate(l) => {
                    let _ = b.add_remote_candidate_immediate(l);
                }
                Event::Connected => a_conn = true,
                Event::AudioFrame(..) => a_audio += 1,
                Event::VideoAccessUnit(..) => a_video += 1,
                _ => {}
            }
        }
        while let Some(ev) = b.poll_event() {
            match ev {
                Event::LocalCandidate(l) => {
                    let _ = a.add_remote_candidate_immediate(l);
                }
                Event::Connected => b_conn = true,
                Event::AudioFrame(..) => b_audio += 1,
                Event::VideoAccessUnit(..) => b_video += 1,
                _ => {}
            }
        }
        if a_audio > 5 && a_video > 2 && b_audio > 5 && b_video > 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    println!("A: connected={a_conn} audio_frames={a_audio} video_units={a_video}");
    println!("B: connected={b_conn} audio_frames={b_audio} video_units={b_video}");

    a.close();
    b.close();
}
