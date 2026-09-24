mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{audio_config, av_config, ScriptedOfferer, ToneSource, SCRIPTED_MID, SCRIPTED_PT};
use rtc::{
    write_sdp, ConnectionState, DtlsRole, Event, PeerConnection, RemoteDescription, RtcConfig,
    RtcError, SignalingState,
};
use sdp::{Attribute, SetupRole};

fn answer_scripted_offer(setup: &str, config: RtcConfig) {
    let mut scripted = ScriptedOfferer::new(dtls::Role::Client);
    let mut pc = PeerConnection::new(config).expect("peer");
    pc.add_audio_track(Arc::new(ToneSource::new()))
        .expect("audio track");
    pc.set_remote_description(RemoteDescription::Offer(scripted.offer(setup)))
        .expect("apply offer");
    assert_eq!(pc.dtls_role(), Some(DtlsRole::Server));
    let answer = pc.create_answer().expect("answer");
    let text = write_sdp(answer.sdp()).expect("answer text");
    assert!(text.contains("a=setup:passive"), "{text}");
    assert!(text.contains(&format!("a=mid:{SCRIPTED_MID}")), "{text}");
    assert!(
        text.contains(&format!("a=rtpmap:{SCRIPTED_PT} opus/48000/2")),
        "{text}"
    );
    scripted.apply_answer(&answer);

    let (mut connected, mut frames) = (false, 0usize);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        while let Some(event) = pc.poll_event() {
            match event {
                Event::LocalCandidate(line) => scripted.add_remote_candidate(&line),
                Event::Connected => connected = true,
                Event::AudioFrame(..) => frames += 1,
                Event::Failed(error) => panic!("rtc peer failed: {error}"),
                _ => {}
            }
        }
        scripted.trickle_to(&pc);
        scripted.pump(true);
        if connected && frames >= 10 && scripted.received.len() >= 10 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(
        connected,
        "rtc answerer completed the handshake as DTLS server"
    );
    assert!(
        frames >= 10,
        "rtc decoded the scripted peer's audio ({frames} frames)"
    );
    assert!(
        scripted.received.len() >= 10,
        "scripted peer decrypted rtc's audio"
    );
    for rtp in &scripted.received {
        assert_eq!(
            rtp.payload_type, SCRIPTED_PT,
            "rtc sends with the negotiated payload type"
        );
        assert_eq!(
            rtp.mid.as_deref(),
            Some(SCRIPTED_MID.as_bytes()),
            "negotiated MID and id"
        );
        assert!(
            rtp.twcc,
            "rtc stamps TWCC under the negotiated extension id"
        );
    }
    assert_eq!(pc.connection_state(), ConnectionState::Connected);
    pc.close();
}

#[test]
fn answerer_is_dtls_server_when_it_prefers_passive() {
    let config = RtcConfig {
        answer_dtls_role: DtlsRole::Server,
        ..audio_config()
    };
    answer_scripted_offer("actpass", config);
}

#[test]
fn answerer_is_dtls_server_when_the_offer_is_active() {
    answer_scripted_offer("active", audio_config());
}

fn rewrite_setup(answer: &mut rtc::Answer, setup: SetupRole) {
    for m in &mut answer.0.media {
        for attr in &mut m.attributes {
            if let Attribute::Setup(role) = attr {
                *role = setup;
            }
        }
    }
}

#[test]
fn offerer_takes_the_client_role_for_a_passive_answer() {
    let mut a = PeerConnection::new(av_config()).expect("peer a");
    let config = RtcConfig {
        answer_dtls_role: DtlsRole::Server,
        ..av_config()
    };
    let mut b = PeerConnection::new(config).expect("peer b");
    let offer = a.create_offer().expect("offer");
    assert!(write_sdp(offer.sdp()).expect("offer text").contains("a=setup:actpass"));
    assert_eq!(a.dtls_role(), None, "the offerer's role waits for the answer");
    b.set_remote_description(RemoteDescription::Offer(offer))
        .expect("apply offer");
    let answer = b.create_answer().expect("answer");
    assert!(write_sdp(answer.sdp()).expect("answer text").contains("a=setup:passive"));
    a.set_remote_description(RemoteDescription::Answer(answer))
        .expect("a passive answer is accepted");
    assert_eq!(a.signaling_state(), SignalingState::Stable);
    assert_eq!(a.dtls_role(), Some(DtlsRole::Client));
    assert_eq!(b.dtls_role(), Some(DtlsRole::Server));
}

#[test]
fn offerer_rejects_actpass_and_holdconn_answers() {
    for setup in [SetupRole::ActPass, SetupRole::HoldConn] {
        let mut a = PeerConnection::new(av_config()).expect("peer a");
        let mut b = PeerConnection::new(av_config()).expect("peer b");
        b.set_remote_description(RemoteDescription::Offer(a.create_offer().expect("offer")))
            .expect("apply offer");
        let mut answer = b.create_answer().expect("answer");
        rewrite_setup(&mut answer, setup);
        let result = a.set_remote_description(RemoteDescription::Answer(answer));
        assert!(
            matches!(result, Err(RtcError::Negotiation(_))),
            "{setup:?}: {result:?}"
        );
    }
}

#[test]
fn answerer_rejects_a_holdconn_offer() {
    let scripted = ScriptedOfferer::new(dtls::Role::Client);
    let mut pc = PeerConnection::new(audio_config()).expect("peer");
    let result = pc.set_remote_description(RemoteDescription::Offer(scripted.offer("holdconn")));
    assert!(
        matches!(result, Err(RtcError::Negotiation(_))),
        "{result:?}"
    );
}
