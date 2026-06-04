//! This stack carries only media. An offer requesting an *active* data channel
//! is refused outright; an offer whose data section is already rejected
//! (port 0) is tolerated and answered.

use rtc::{MediaConfig, Offer, PeerConnection, RemoteDescription, RtcConfig};

const HEAD: &str = "v=0\r\n\
o=- 2795505170280940885 1 IN IP4 0.0.0.0\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0 2\r\n\
a=ice-options:trickle\r\n\
m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
c=IN IP4 0.0.0.0\r\n\
a=mid:0\r\n\
a=sendrecv\r\n\
a=rtcp-mux\r\n\
a=ice-ufrag:m3HA4k84\r\n\
a=ice-pwd:Q1ZyiXynuTrLxnsUd16Hfvbt\r\n\
a=fingerprint:sha-256 23:3B:8A:6B:DF:BD:0B:D3:D0:13:2B:6A:F5:C5:5D:46:F7:A3:61:C0:A0:41:C6:A7:A8:2B:7D:75:12:BA:95:49\r\n\
a=setup:actpass\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=ssrc:2067992544 cname:peer\r\n";

fn data_section(port: u16) -> String {
    format!(
        "m=application {port} UDP/DTLS/SCTP webrtc-datachannel\r\n\
c=IN IP4 0.0.0.0\r\n\
a=mid:2\r\n\
a=ice-ufrag:m3HA4k84\r\n\
a=ice-pwd:Q1ZyiXynuTrLxnsUd16Hfvbt\r\n\
a=fingerprint:sha-256 23:3B:8A:6B:DF:BD:0B:D3:D0:13:2B:6A:F5:C5:5D:46:F7:A3:61:C0:A0:41:C6:A7:A8:2B:7D:75:12:BA:95:49\r\n\
a=setup:actpass\r\n\
a=sctp-port:5000\r\n"
    )
}

fn peer() -> PeerConnection {
    let cfg = RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .loopback_only(true)
        .build();
    PeerConnection::new(cfg)
}

#[test]
fn active_data_channel_is_refused() {
    let sdp = format!("{HEAD}{}", data_section(9));
    let offer = Offer::parse(&sdp).expect("parse offer");
    let mut b = peer();
    let err = b.set_remote_description(RemoteDescription::Offer(offer));
    assert!(err.is_err(), "active data channel must be refused");
}

#[test]
fn rejected_data_channel_is_tolerated() {
    let sdp = format!("{HEAD}{}", data_section(0));
    let offer = Offer::parse(&sdp).expect("parse offer");
    let mut b = peer();
    b.set_remote_description(RemoteDescription::Offer(offer))
        .expect("rejected data channel is tolerated");
    let answer = b.create_answer().expect("answer");
    assert!(
        answer.to_string().contains("m=audio"),
        "audio still answered"
    );
}
