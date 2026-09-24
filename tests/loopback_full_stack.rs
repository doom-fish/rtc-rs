//! GATING TEST: two in-process `PeerConnection`s complete the full WebRTC
//! handshake over loopback (SDP → trickle ICE → DTLS → SRTP) and exchange live
//! Opus audio and H.264 video.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{av_config, SyntheticVideoSource, ToneSource};
use rtc::{ConnectionState, DtlsRole, Event, PeerConnection, RemoteDescription, RtcConfig, Stats};

#[derive(Default)]
struct Seen {
    connected: bool,
    audio: bool,
    video: bool,
    stats: Option<Stats>,
}

impl Seen {
    fn done(&self) -> bool {
        self.connected
            && self.audio
            && self.video
            && self
                .stats
                .is_some_and(|s| s.rtt_ms > 0 && s.audio_recv_bitrate > 0)
    }
}

fn pump(pc: &PeerConnection, peer: &PeerConnection, seen: &mut Seen) {
    while let Some(ev) = pc.poll_event() {
        match ev {
            Event::LocalCandidate(line) => {
                peer.add_remote_candidate_immediate(line).expect("trickle");
            }
            Event::Connected => seen.connected = true,
            Event::AudioFrame(..) => seen.audio = true,
            Event::VideoAccessUnit(..) => seen.video = true,
            Event::Stats(stats) if seen.audio && seen.video => seen.stats = Some(stats),
            Event::Failed(error) => panic!("connection failed: {error}"),
            _ => {}
        }
    }
}

fn call(offerer: &mut PeerConnection, answerer: &mut PeerConnection, offerer_role: DtlsRole) {
    for pc in [&mut *offerer, &mut *answerer] {
        pc.add_audio_track(Arc::new(ToneSource::new()))
            .expect("audio track");
        pc.add_video_track(Arc::new(SyntheticVideoSource::new()))
            .expect("video track");
    }

    let offer = offerer.create_offer().expect("offer");
    answerer
        .set_remote_description(RemoteDescription::Offer(offer))
        .expect("apply offer");
    let answer = answerer.create_answer().expect("answer");
    offerer
        .set_remote_description(RemoteDescription::Answer(answer))
        .expect("apply answer");
    let answerer_role = match offerer_role {
        DtlsRole::Client => DtlsRole::Server,
        DtlsRole::Server => DtlsRole::Client,
    };
    assert_eq!(offerer.dtls_role(), Some(offerer_role));
    assert_eq!(answerer.dtls_role(), Some(answerer_role));

    let mut so = Seen::default();
    let mut sa = Seen::default();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        pump(offerer, answerer, &mut so);
        pump(answerer, offerer, &mut sa);
        if so.done() && sa.done() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(so.connected && sa.connected, "both peers connected");
    assert!(so.audio && sa.audio, "both peers received audio");
    assert!(so.video && sa.video, "both peers received video");
    for stats in [so.stats, sa.stats] {
        let stats = stats.expect("statistics after media");
        assert!(
            stats.rtt_ms > 0,
            "RTT from the peer's receiver reports: {stats:?}"
        );
        assert!(
            stats.audio_recv_bitrate > 0 && stats.video_send_bitrate > 0,
            "{stats:?}"
        );
        assert_eq!(stats.srtp_rejected, 0, "{stats:?}");
        assert_eq!(stats.dropped_unexpected_source, 0, "{stats:?}");
    }
    assert_eq!(offerer.connection_state(), ConnectionState::Connected);
    assert_eq!(answerer.connection_state(), ConnectionState::Connected);

    offerer.close();
    answerer.close();
    assert_eq!(offerer.connection_state(), ConnectionState::Closed);
}

#[test]
fn loopback_full_stack() {
    let mut a = PeerConnection::new(av_config()).expect("peer a");
    let mut b = PeerConnection::new(av_config()).expect("peer b");
    call(&mut a, &mut b, DtlsRole::Server);
}

#[test]
fn loopback_full_stack_with_roles_swapped() {
    let mut a = PeerConnection::new(av_config()).expect("peer a");
    let mut b = PeerConnection::new(av_config()).expect("peer b");
    call(&mut b, &mut a, DtlsRole::Server);
}

#[test]
fn loopback_full_stack_with_a_passive_answer() {
    let mut a = PeerConnection::new(av_config()).expect("peer a");
    let passive = RtcConfig {
        answer_dtls_role: DtlsRole::Server,
        ..av_config()
    };
    let mut b = PeerConnection::new(passive).expect("peer b");
    call(&mut a, &mut b, DtlsRole::Client);
}
