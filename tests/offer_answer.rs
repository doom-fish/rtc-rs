//! Offer/answer negotiation drives the signaling state machine and produces a
//! BUNDLE-grouped, rtcp-mux SDP with audio and video.

use rtc::{
    write_sdp, DtlsRole, MediaConfig, PeerConnection, RemoteDescription, RtcConfig, RtcError,
    SignalingState,
};

fn av_config() -> RtcConfig {
    RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .video(MediaConfig::h264_constrained_baseline())
        .loopback_only(true)
        .build()
}

#[test]
fn offer_answer_completes_signaling() {
    let mut a = PeerConnection::new(av_config()).expect("peer a");
    let mut b = PeerConnection::new(av_config()).expect("peer b");

    assert_eq!(a.signaling_state(), SignalingState::Stable);

    let offer = a.create_offer().expect("offer");
    assert_eq!(a.signaling_state(), SignalingState::HaveLocalOffer);

    let offer_text = write_sdp(offer.sdp()).expect("offer text");
    assert!(offer_text.contains("m=audio"), "offer has audio");
    assert!(offer_text.contains("m=video"), "offer has video");
    assert!(offer_text.contains("a=group:BUNDLE"), "offer bundles");
    assert!(offer_text.contains("a=rtcp-mux"), "offer rtcp-mux");
    assert!(
        offer_text.contains("a=fingerprint:sha-256"),
        "offer fingerprint"
    );
    assert!(
        offer_text.contains("a=setup:actpass"),
        "offerer lets the answerer pick"
    );
    assert!(
        offer_text.contains("a=rtcp-fb:96 nack pli"),
        "offer asks for video feedback"
    );

    b.set_remote_description(RemoteDescription::Offer(offer))
        .expect("apply offer");
    assert_eq!(b.signaling_state(), SignalingState::HaveRemoteOffer);
    assert_eq!(b.dtls_role(), Some(DtlsRole::Client));

    let answer = b.create_answer().expect("answer");
    assert_eq!(b.signaling_state(), SignalingState::Stable);

    let answer_text = write_sdp(answer.sdp()).expect("answer text");
    assert!(answer_text.contains("m=audio"), "answer has audio");
    assert!(answer_text.contains("m=video"), "answer has video");
    assert!(answer_text.contains("a=setup:active"), "answerer is active");
    assert!(!answer_text.contains("a=setup:passive"));

    a.set_remote_description(RemoteDescription::Answer(answer))
        .expect("apply answer");
    assert_eq!(a.signaling_state(), SignalingState::Stable);
    assert_eq!(a.dtls_role(), Some(DtlsRole::Server));
}

#[test]
fn cannot_answer_own_offer() {
    let mut a = PeerConnection::new(av_config()).expect("peer");
    let _ = a.create_offer().expect("offer");
    assert!(a.create_answer().is_err(), "offerer must not answer itself");
}

#[test]
fn answerer_can_prefer_the_server_role() {
    let mut a = PeerConnection::new(av_config()).expect("peer a");
    let config = RtcConfig {
        answer_dtls_role: DtlsRole::Server,
        ..av_config()
    };
    let mut b = PeerConnection::new(config).expect("peer b");
    b.set_remote_description(RemoteDescription::Offer(a.create_offer().expect("offer")))
        .expect("apply offer");
    assert_eq!(b.dtls_role(), Some(DtlsRole::Server));
    let answer = b.create_answer().expect("answer");
    let text = write_sdp(answer.sdp()).expect("answer text");
    assert!(text.contains("a=setup:passive"));
    assert!(!text.contains("a=setup:active"));
}

#[test]
fn ice_credentials_and_ssrcs_are_random() {
    let mut a = PeerConnection::new(av_config()).expect("peer a");
    let mut b = PeerConnection::new(av_config()).expect("peer b");
    let first = write_sdp(a.create_offer().expect("offer").sdp()).expect("text");
    let second = write_sdp(b.create_offer().expect("offer").sdp()).expect("text");
    let value = |text: &str, key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(str::to_string)
            .expect("attribute present")
    };
    let ufrag = value(&first, "a=ice-ufrag:");
    let pwd = value(&first, "a=ice-pwd:");
    assert!(ufrag.len() >= 22, "ufrag carries at least 128 bits");
    assert!(pwd.len() >= 22, "password carries at least 128 bits");
    assert_ne!(ufrag, value(&second, "a=ice-ufrag:"));
    assert_ne!(pwd, value(&second, "a=ice-pwd:"));
    assert_ne!(value(&first, "a=ssrc:"), value(&second, "a=ssrc:"));
}

#[test]
fn invalid_ice_servers_are_rejected_up_front() {
    let with = |server| RtcConfig::builder().ice_server(server).build();
    assert!(matches!(
        PeerConnection::new(with(rtc::IceServer::stun("https://example.invalid"))),
        Err(RtcError::Config(_))
    ));
    assert!(matches!(
        PeerConnection::new(with(rtc::IceServer::turn("turns:127.0.0.1:5349", "u", "p"))),
        Err(RtcError::Unsupported(_))
    ));
    assert!(matches!(
        PeerConnection::new(with(rtc::IceServer::turn(
            "turn:127.0.0.1:3478?transport=tcp",
            "u",
            "p"
        ))),
        Err(RtcError::Unsupported(_))
    ));
    assert!(matches!(
        PeerConnection::new(with(rtc::IceServer::stun("turn:127.0.0.1:3478"))),
        Err(RtcError::Config(_))
    ));
    assert!(PeerConnection::new(with(rtc::IceServer::stun("stun:127.0.0.1:3478"))).is_ok());
    assert!(PeerConnection::new(with(rtc::IceServer::turn("turn:127.0.0.1", "u", "p"))).is_ok());
}

#[test]
fn turn_credentials_are_redacted() {
    let server = rtc::IceServer::turn("turn:127.0.0.1:3478", "alice", "hunter2-secret");
    let text = format!(
        "{server:?} {:?}",
        RtcConfig::builder().ice_server(server.clone()).build()
    );
    assert!(text.contains("alice"));
    assert!(!text.contains("hunter2-secret"));
}
