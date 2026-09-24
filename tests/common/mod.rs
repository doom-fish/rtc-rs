#![allow(dead_code)]

pub mod fake_turn;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rtc::{Answer, CapturedFrame, MediaConfig, Offer, Result, RtcConfig, VideoSource};

pub fn av_config() -> RtcConfig {
    RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .video(MediaConfig::h264_constrained_baseline())
        .loopback_only(true)
        .build()
}

pub fn audio_config() -> RtcConfig {
    RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .loopback_only(true)
        .build()
}

/// A continuous 440 Hz tone — non-blocking, the worker paces it.
pub struct ToneSource {
    phase: Mutex<f32>,
    pub blocks: AtomicU64,
}

impl ToneSource {
    pub fn new() -> Self {
        Self {
            phase: Mutex::new(0.0),
            blocks: AtomicU64::new(0),
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
        self.blocks.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// A synthetic Annex-B H.264 source emitting an IDR access unit at ~30 fps. It
/// paces itself off a wall clock and returns `None` when no frame is due yet,
/// matching the contract of a real capture source (the worker pulls with a zero
/// timeout and expects `None` when nothing is ready).
pub struct SyntheticVideoSource {
    ts: AtomicU32,
    next_due: Mutex<Instant>,
    pub keyframe_requests: AtomicU64,
}

impl SyntheticVideoSource {
    pub fn new() -> Self {
        Self {
            ts: AtomicU32::new(0),
            next_due: Mutex::new(Instant::now()),
            keyframe_requests: AtomicU64::new(0),
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
        data.extend(std::iter::repeat_n(0xAB, 800));
        Some(CapturedFrame {
            width: 640,
            height: 480,
            rtp_timestamp: ts,
            keyframe: true,
            data,
        })
    }

    fn request_keyframe(&self) {
        self.keyframe_requests.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct OffsetClock {
    offset: Mutex<Duration>,
}

impl OffsetClock {
    pub fn advance(&self, by: Duration) {
        *self.offset.lock().unwrap() += by;
    }
}

impl rtc::Clock for OffsetClock {
    fn now(&self) -> Instant {
        Instant::now() + *self.offset.lock().unwrap()
    }
}

pub fn fingerprint_hex(fp: &[u8; 32]) -> String {
    fp.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub const SCRIPTED_PT: u8 = 109;
pub const SCRIPTED_MID: &str = "a0";
pub const SCRIPTED_MID_EXT: u8 = 4;
pub const SCRIPTED_TWCC_EXT: u8 = 5;

struct ChannelSink(Sender<(ice::PacketKind, Vec<u8>, SocketAddr)>);

impl ice::PacketSink for ChannelSink {
    fn on_packet(&self, kind: ice::PacketKind, bytes: &[u8], from: SocketAddr) {
        let _ = self.0.send((kind, bytes.to_vec(), from));
    }
}

pub struct ReceivedRtp {
    pub payload_type: u8,
    pub mid: Option<Vec<u8>>,
    pub twcc: bool,
}

pub struct ScriptedOfferer {
    role: dtls::Role,
    ufrag: String,
    pwd: String,
    pub dtls: dtls::Connection,
    ssrc: u32,
    agent: Option<ice::IceAgent>,
    rx: Receiver<(ice::PacketKind, Vec<u8>, SocketAddr)>,
    tx: Sender<(ice::PacketKind, Vec<u8>, SocketAddr)>,
    start: Instant,
    emitted: usize,
    send: Option<srtp::SrtpSession>,
    recv: Option<srtp::SrtpSession>,
    encoder: opus::Encoder,
    seq: u16,
    ts: u32,
    twcc: u16,
    next_audio: Instant,
    pub received: Vec<ReceivedRtp>,
}

impl ScriptedOfferer {
    pub fn new(role: dtls::Role) -> Self {
        let config = match role {
            dtls::Role::Client => dtls::Config::client(),
            dtls::Role::Server => dtls::Config::server(),
        };
        let (tx, rx) = channel();
        Self {
            role,
            ufrag: "scriptedUfragScripted0".into(),
            pwd: "scriptedPasswordScriptedPassword".into(),
            dtls: dtls::Connection::new(config).expect("dtls"),
            ssrc: 0x5C21_7ED0,
            agent: None,
            rx,
            tx,
            start: Instant::now(),
            emitted: 0,
            send: None,
            recv: None,
            encoder: opus::Encoder::new(
                opus::SampleRate::Hz48000,
                opus::Channels::Mono,
                opus::Application::Voip,
            )
            .expect("encoder"),
            seq: 1,
            ts: 0,
            twcc: 0,
            next_audio: Instant::now(),
            received: Vec::new(),
        }
    }

    pub fn offer(&self, setup: &str) -> Offer {
        let text = format!(
            "v=0\r\n\
             o=- 4611731400430051336 2 IN IP4 127.0.0.1\r\n\
             s=-\r\n\
             t=0 0\r\n\
             a=group:BUNDLE {mid}\r\n\
             a=ice-options:trickle\r\n\
             m=audio 9 UDP/TLS/RTP/SAVPF {pt}\r\n\
             c=IN IP4 0.0.0.0\r\n\
             a=mid:{mid}\r\n\
             a=sendrecv\r\n\
             a=rtcp-mux\r\n\
             a=ice-ufrag:{ufrag}\r\n\
             a=ice-pwd:{pwd}\r\n\
             a=fingerprint:sha-256 {fp}\r\n\
             a=setup:{setup}\r\n\
             a=extmap:{mid_ext} {mid_uri}\r\n\
             a=extmap:{twcc_ext} {twcc_uri}\r\n\
             a=rtpmap:{pt} opus/48000/2\r\n\
             a=rtcp-fb:{pt} transport-cc\r\n\
             a=ssrc:{ssrc} cname:scripted\r\n",
            mid = SCRIPTED_MID,
            pt = SCRIPTED_PT,
            ufrag = self.ufrag,
            pwd = self.pwd,
            fp = fingerprint_hex(&self.dtls.local_fingerprint()),
            mid_ext = SCRIPTED_MID_EXT,
            twcc_ext = SCRIPTED_TWCC_EXT,
            mid_uri = sdp::extmap::URI_MID,
            twcc_uri = sdp::extmap::URI_TWCC,
            ssrc = self.ssrc,
        );
        Offer::parse(&text).expect("scripted offer parses")
    }

    pub fn apply_answer(&mut self, answer: &Answer) {
        let sdp = answer.sdp();
        let m = sdp
            .media
            .iter()
            .find(|m| m.port != 0)
            .expect("accepted section");
        let ufrag = sdp.ice_ufrag_for(m).expect("ufrag").to_string();
        let pwd = sdp.ice_pwd_for(m).expect("pwd").to_string();
        let digest: [u8; 32] = sdp.fingerprints_for(m)[0]
            .digest
            .as_slice()
            .try_into()
            .expect("sha-256");
        self.dtls
            .set_expected_peer_fingerprint(digest)
            .expect("pin");
        let mut config = ice::IceConfig::new(
            ice::IceRole::Controlling,
            &self.ufrag,
            &self.pwd,
            ufrag,
            pwd,
        );
        config.bind_addr = Some(SocketAddr::from(([127, 0, 0, 1], 0)));
        let agent = ice::IceAgent::new(config).expect("agent");
        agent.set_packet_sink(Arc::new(ChannelSink(self.tx.clone())));
        agent.start_gathering().expect("gather");
        self.agent = Some(agent);
    }

    pub fn add_remote_candidate(&self, line: &str) {
        let candidate = ice::sdp::parse(line, &ice::MdnsPolicy::Reject).expect("candidate");
        self.agent
            .as_ref()
            .expect("agent")
            .add_remote_candidate(candidate)
            .expect("add candidate");
    }

    pub fn trickle_to(&mut self, pc: &rtc::PeerConnection) {
        let Some(agent) = &self.agent else { return };
        let locals = agent.local_candidates();
        for c in &locals[self.emitted..] {
            pc.add_remote_candidate_immediate(ice::sdp::to_sdp(c))
                .expect("trickle");
        }
        self.emitted = locals.len();
    }

    pub fn selected(&self) -> bool {
        self.agent
            .as_ref()
            .is_some_and(|a| a.selected_pair().is_some())
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap()
    }

    pub fn pump(&mut self, speak_dtls: bool) {
        let Some(agent) = self.agent.as_ref() else {
            return;
        };
        while let Ok((kind, mut bytes, from)) = self.rx.try_recv() {
            match kind {
                ice::PacketKind::Dtls if speak_dtls => {
                    let now = self.now_ms();
                    self.dtls.feed_recv(&bytes, from, now).expect("dtls");
                }
                ice::PacketKind::Srtp
                    if bytes.len() > 1 && !(64..=95).contains(&(bytes[1] & 0x7f)) =>
                {
                    if let Some(recv) = self.recv.as_mut() {
                        if let Ok(pkt) = recv.unprotect(&mut bytes) {
                            use rtp::Packet;
                            let exts = pkt.extensions();
                            self.received.push(ReceivedRtp {
                                payload_type: pkt.header().payload_type(),
                                mid: exts
                                    .iter()
                                    .find(|e| e.id() == SCRIPTED_MID_EXT)
                                    .map(rtp::Extension::data),
                                twcc: exts.iter().any(|e| e.id() == SCRIPTED_TWCC_EXT),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        if !speak_dtls || agent.selected_pair().is_none() {
            return;
        }
        let now = self.now_ms();
        if self.dtls.timeout(now) == Some(0) {
            self.dtls.handle_timeout(now).expect("dtls timeout");
        }
        while let Some(record) = self.dtls.poll_send() {
            let _ = agent.send_on_selected(&record);
        }
        if self.send.is_none() && self.dtls.is_handshake_done() {
            let block = self
                .dtls
                .export_keying_material(b"EXTRACTOR-dtls_srtp", None, 56)
                .expect("exporter");
            let (client_key, server_key) = (&block[..16], &block[16..32]);
            let (client_salt, server_salt) = (&block[32..44], &block[44..56]);
            let (send, recv) = if self.role == dtls::Role::Client {
                ((client_key, client_salt), (server_key, server_salt))
            } else {
                ((server_key, server_salt), (client_key, client_salt))
            };
            let profile = srtp::Profile::AeadAes128Gcm;
            self.send = Some(srtp::SrtpSession::new_sender(
                srtp::SrtpKey::from_master(send.0, send.1, profile).expect("send key"),
            ));
            self.recv = Some(srtp::SrtpSession::new_receiver(
                srtp::SrtpKey::from_master(recv.0, recv.1, profile).expect("recv key"),
            ));
        }
        if self.send.is_some() {
            while Instant::now() >= self.next_audio {
                self.next_audio += Duration::from_millis(20);
                self.send_audio();
            }
        }
    }

    fn send_audio(&mut self) {
        let pcm: Vec<i16> = (0..960)
            .map(|i| ((i as f32 * 0.06).sin() * 6000.0) as i16)
            .collect();
        let mut encoded = vec![0u8; 1500];
        let n = self.encoder.encode(&pcm, &mut encoded).expect("opus");
        let pkt = rtp::RtpPacket::builder()
            .payload_type(SCRIPTED_PT)
            .sequence_number(self.seq)
            .timestamp(self.ts)
            .ssrc(self.ssrc)
            .extension(rtp::Extension::mid(SCRIPTED_MID.as_bytes()).with_id(SCRIPTED_MID_EXT))
            .extension(rtp::Extension::twcc(self.twcc).with_id(SCRIPTED_TWCC_EXT))
            .payload(&encoded[..n])
            .build()
            .expect("rtp");
        self.seq = self.seq.wrapping_add(1);
        self.ts = self.ts.wrapping_add(960);
        self.twcc = self.twcc.wrapping_add(1);
        let mut out = vec![0u8; 1600];
        let session = self.send.as_mut().expect("srtp");
        let n = session.protect(&pkt, &mut out).expect("protect");
        let _ = self
            .agent
            .as_ref()
            .expect("agent")
            .send_on_selected(&out[..n]);
    }
}
