//! Offer/answer negotiation drives the signaling state machine and produces a
//! BUNDLE-grouped, rtcp-mux SDP with audio and video.

use rtc::{MediaConfig, PeerConnection, RemoteDescription, RtcConfig, SignalingState};

fn av_config() -> RtcConfig {
    RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .video(MediaConfig::h264_constrained_baseline())
        .loopback_only(true)
        .build()
}

#[test]
fn offer_answer_completes_signaling() {
    let mut a = PeerConnection::new(av_config());
    let mut b = PeerConnection::new(av_config());

    assert_eq!(a.signaling_state(), SignalingState::Stable);

    let offer = a.create_offer().expect("offer");
    assert_eq!(a.signaling_state(), SignalingState::HaveLocalOffer);

    let offer_text = offer.to_string();
    assert!(offer_text.contains("m=audio"), "offer has audio");
    assert!(offer_text.contains("m=video"), "offer has video");
    assert!(offer_text.contains("a=group:BUNDLE"), "offer bundles");
    assert!(offer_text.contains("a=rtcp-mux"), "offer rtcp-mux");
    assert!(offer_text.contains("a=fingerprint:sha-256"), "offer fingerprint");

    b.set_remote_description(RemoteDescription::Offer(offer))
        .expect("apply offer");
    assert_eq!(b.signaling_state(), SignalingState::HaveRemoteOffer);

    let answer = b.create_answer().expect("answer");
    assert_eq!(b.signaling_state(), SignalingState::Stable);

    let answer_text = answer.to_string();
    assert!(answer_text.contains("m=audio"), "answer has audio");
    assert!(answer_text.contains("m=video"), "answer has video");
    assert!(answer_text.contains("a=setup:active"), "answerer is active");

    a.set_remote_description(RemoteDescription::Answer(answer))
        .expect("apply answer");
    assert_eq!(a.signaling_state(), SignalingState::Stable);
}

#[test]
fn cannot_answer_own_offer() {
    let mut a = PeerConnection::new(av_config());
    let _ = a.create_offer().expect("offer");
    assert!(a.create_answer().is_err(), "offerer must not answer itself");
}
