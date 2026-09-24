//! Create a local offer and the matching answer, printing both SDP blobs.
//!
//! Run with: `cargo run --example 01_offer_answer_local`

use rtc::{write_sdp, MediaConfig, PeerConnection, RemoteDescription, RtcConfig};

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

    let offer = a.create_offer().expect("offer");
    println!("--- OFFER ---\n{}", write_sdp(offer.sdp()).expect("offer text"));

    b.set_remote_description(RemoteDescription::Offer(offer))
        .expect("apply offer");
    let answer = b.create_answer().expect("answer");
    println!("--- ANSWER ---\n{}", write_sdp(answer.sdp()).expect("answer text"));

    a.set_remote_description(RemoteDescription::Answer(answer))
        .expect("apply answer");
    println!("Signaling complete: {:?}", a.signaling_state());
}
