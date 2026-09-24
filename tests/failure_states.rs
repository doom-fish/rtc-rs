mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{audio_config, av_config, OffsetClock, ScriptedOfferer, ToneSource};
use rtc::{
    Answer, ConnectionState, DtlsState, Event, IceConnectionState, Offer, PeerConnection,
    RemoteDescription, RtcConfig, RtcError,
};
use sdp::{Attribute, Sdp};

#[derive(Default, Debug)]
struct Outcome {
    connected: bool,
    dtls_failed: bool,
    ice_failed: bool,
    state_failed: bool,
    failure: Option<RtcError>,
    audio_frames: usize,
}

impl Outcome {
    fn absorb(&mut self, event: Event, peer: Option<&PeerConnection>) {
        match event {
            Event::LocalCandidate(line) => {
                if let Some(peer) = peer {
                    peer.add_remote_candidate_immediate(line).ok();
                }
            }
            Event::Connected => self.connected = true,
            Event::DtlsStateChange(DtlsState::Failed) => self.dtls_failed = true,
            Event::IceConnectionStateChange(IceConnectionState::Failed) => self.ice_failed = true,
            Event::ConnectionStateChange(ConnectionState::Failed) => self.state_failed = true,
            Event::Failed(error) => self.failure = Some(error),
            Event::AudioFrame(..) => self.audio_frames += 1,
            _ => {}
        }
    }
}

fn tamper_fingerprint(sdp: &mut Sdp) {
    for m in &mut sdp.media {
        for attr in &mut m.attributes {
            if let Attribute::Fingerprint(fp) = attr {
                fp.digest[0] ^= 0xFF;
            }
        }
    }
}

fn run_pair(
    a: &PeerConnection,
    b: &PeerConnection,
    until: impl Fn(&Outcome, &Outcome) -> bool,
) -> (Outcome, Outcome) {
    let (mut oa, mut ob) = (Outcome::default(), Outcome::default());
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !until(&oa, &ob) {
        while let Some(event) = a.poll_event() {
            oa.absorb(event, Some(b));
        }
        while let Some(event) = b.poll_event() {
            ob.absorb(event, Some(a));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    (oa, ob)
}

fn short_handshake() -> RtcConfig {
    RtcConfig {
        handshake_timeout: Duration::from_secs(5),
        ..av_config()
    }
}

#[test]
fn server_side_fingerprint_mismatch_fails_the_handshake() {
    let mut a = PeerConnection::new(short_handshake()).expect("peer a");
    let mut b = PeerConnection::new(short_handshake()).expect("peer b");
    b.set_remote_description(RemoteDescription::Offer(a.create_offer().expect("offer")))
        .expect("apply offer");
    let Answer(mut sdp) = b.create_answer().expect("answer");
    tamper_fingerprint(&mut sdp);
    a.set_remote_description(RemoteDescription::Answer(Answer(sdp)))
        .expect("apply tampered answer");

    let (oa, ob) = run_pair(&a, &b, |oa, ob| {
        oa.failure.is_some() && ob.failure.is_some()
    });
    assert_eq!(oa.failure, Some(RtcError::FingerprintMismatch), "{oa:?}");
    assert!(oa.dtls_failed && oa.state_failed && !oa.connected, "{oa:?}");
    assert!(
        ob.failure.is_some() && ob.dtls_failed && !ob.connected,
        "{ob:?}"
    );
    assert_eq!(a.connection_state(), ConnectionState::Failed);
    assert_eq!(b.connection_state(), ConnectionState::Failed);
}

#[test]
fn client_side_fingerprint_mismatch_fails_the_handshake() {
    let mut a = PeerConnection::new(short_handshake()).expect("peer a");
    let mut b = PeerConnection::new(short_handshake()).expect("peer b");
    let Offer(mut sdp) = a.create_offer().expect("offer");
    tamper_fingerprint(&mut sdp);
    b.set_remote_description(RemoteDescription::Offer(Offer(sdp)))
        .expect("apply tampered offer");
    a.set_remote_description(RemoteDescription::Answer(
        b.create_answer().expect("answer"),
    ))
    .expect("apply answer");

    let (oa, ob) = run_pair(&a, &b, |oa, ob| {
        oa.failure.is_some() && ob.failure.is_some()
    });
    assert_eq!(ob.failure, Some(RtcError::FingerprintMismatch), "{ob:?}");
    assert!(ob.dtls_failed && ob.state_failed && !ob.connected, "{ob:?}");
    assert!(
        oa.failure.is_some() && oa.dtls_failed && !oa.connected,
        "{oa:?}"
    );
}

#[test]
fn stalled_dtls_handshake_reaches_the_failed_state() {
    let mut scripted = ScriptedOfferer::new(dtls::Role::Server);
    let config = RtcConfig {
        handshake_timeout: Duration::from_secs(2),
        ..audio_config()
    };
    let mut pc = PeerConnection::new(config).expect("peer");
    pc.set_remote_description(RemoteDescription::Offer(scripted.offer("actpass")))
        .expect("apply offer");
    let answer = pc.create_answer().expect("answer");
    scripted.apply_answer(&answer);

    let started = Instant::now();
    let mut outcome = Outcome::default();
    while started.elapsed() < Duration::from_secs(20) && outcome.failure.is_none() {
        while let Some(event) = pc.poll_event() {
            if let Event::LocalCandidate(line) = &event {
                scripted.add_remote_candidate(line);
            }
            outcome.absorb(event, None);
        }
        scripted.trickle_to(&pc);
        scripted.pump(false);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(scripted.selected(), "ICE connected, only DTLS stalled");
    assert_eq!(
        outcome.failure,
        Some(RtcError::Timeout("DTLS handshake")),
        "{outcome:?}"
    );
    assert!(
        outcome.dtls_failed && outcome.state_failed && !outcome.connected,
        "{outcome:?}"
    );
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(pc.connection_state(), ConnectionState::Failed);
    assert!(
        pc.recv_event_timeout(Duration::from_secs(5)).is_none(),
        "the worker stopped"
    );
}

#[test]
fn consent_expiry_fails_the_connection_and_stops_media() {
    let clock = Arc::new(OffsetClock::default());
    let a_config = RtcConfig::builder()
        .audio(rtc::MediaConfig::opus_default())
        .loopback_only(true)
        .clock(clock.clone())
        .build();
    let mut a = PeerConnection::new(a_config).expect("peer a");
    let mut b = PeerConnection::new(audio_config()).expect("peer b");
    let source = Arc::new(ToneSource::new());
    a.add_audio_track(source.clone()).expect("audio track");
    b.add_audio_track(Arc::new(ToneSource::new()))
        .expect("audio track");
    b.set_remote_description(RemoteDescription::Offer(a.create_offer().expect("offer")))
        .expect("apply offer");
    a.set_remote_description(RemoteDescription::Answer(
        b.create_answer().expect("answer"),
    ))
    .expect("apply answer");

    let (oa, ob) = run_pair(&a, &b, |oa, ob| {
        oa.audio_frames >= 10 && ob.audio_frames >= 10
    });
    assert!(oa.connected && ob.connected, "{oa:?} {ob:?}");
    assert!(
        ob.audio_frames >= 10,
        "media flowed before consent was lost"
    );

    b.close();
    clock.advance(Duration::from_secs(35));

    let mut outcome = Outcome::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && outcome.failure.is_none() {
        if let Some(event) = a.recv_event_timeout(Duration::from_millis(50)) {
            outcome.absorb(event, None);
        }
    }
    assert_eq!(
        outcome.failure,
        Some(RtcError::ConsentExpired),
        "{outcome:?}"
    );
    assert!(outcome.ice_failed && outcome.state_failed, "{outcome:?}");
    assert_eq!(a.connection_state(), ConnectionState::Failed);

    let pulled = source.blocks.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        source.blocks.load(Ordering::Relaxed),
        pulled,
        "no media after consent expired"
    );
}
