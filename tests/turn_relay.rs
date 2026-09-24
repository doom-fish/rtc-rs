mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::fake_turn::{FakeTurn, TURN_PASSWORD, TURN_USER};
use common::{audio_config, ToneSource};
use rtc::{Event, IceServer, PeerConnection, RemoteDescription, RtcConfig, RtcError};

#[test]
fn relay_only_call_runs_through_the_turn_server() {
    let turn = FakeTurn::spawn();
    let relayed = RtcConfig::builder()
        .audio(rtc::MediaConfig::opus_default())
        .loopback_only(true)
        .relay_only(true)
        .ice_server(IceServer::turn(turn.url(), TURN_USER, TURN_PASSWORD))
        .build();
    let mut a = PeerConnection::new(relayed).expect("peer a");
    let mut b = PeerConnection::new(audio_config()).expect("peer b");
    a.add_audio_track(Arc::new(ToneSource::new()))
        .expect("audio");
    b.add_audio_track(Arc::new(ToneSource::new()))
        .expect("audio");
    b.set_remote_description(RemoteDescription::Offer(a.create_offer().expect("offer")))
        .expect("apply offer");
    a.set_remote_description(RemoteDescription::Answer(
        b.create_answer().expect("answer"),
    ))
    .expect("apply answer");

    let relay_port = format!(" {} typ relay", turn.relay_addr.port());
    let (mut a_candidates, mut a_frames, mut b_frames) = (Vec::new(), 0, 0);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && (a_frames < 10 || b_frames < 10) {
        while let Some(event) = a.poll_event() {
            match event {
                Event::LocalCandidate(line) => {
                    b.add_remote_candidate_immediate(line.clone())
                        .expect("trickle");
                    a_candidates.push(line);
                }
                Event::AudioFrame(..) => a_frames += 1,
                Event::Failed(error) => panic!("a failed: {error}"),
                _ => {}
            }
        }
        while let Some(event) = b.poll_event() {
            match event {
                Event::LocalCandidate(line) => {
                    a.add_remote_candidate_immediate(line).expect("trickle")
                }
                Event::AudioFrame(..) => b_frames += 1,
                Event::Failed(error) => panic!("b failed: {error}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        a_frames >= 10 && b_frames >= 10,
        "audio both ways ({a_frames}, {b_frames})"
    );
    assert!(!a_candidates.is_empty());
    assert!(
        a_candidates.iter().all(|c| c.contains(&relay_port)),
        "{a_candidates:?}"
    );
    assert!(turn.allocations.load(Ordering::Relaxed) >= 1);
    assert!(
        turn.relayed.load(Ordering::Relaxed) >= 20,
        "media went through the relay"
    );
    a.close();
    b.close();
}

#[test]
fn relay_only_without_a_turn_server_is_refused() {
    let config = RtcConfig {
        relay_only: true,
        ..audio_config()
    };
    assert!(matches!(
        PeerConnection::new(config),
        Err(RtcError::Config(_))
    ));
}
