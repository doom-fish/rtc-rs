//! The single background worker thread that owns the ICE agent, the DTLS
//! handshake, the SRTP sessions, and the media send/receive loops.
//!
//! Everything that mutates transport or codec state lives here so the public
//! [`crate::PeerConnection`] handle can stay a thin, `Send` message-passing
//! facade. The worker is driven by:
//!
//! * inbound datagrams pushed by the ICE agent's receive thread (via
//!   [`crate::transport::ice_glue::WorkerSink`]),
//! * control [`Command`]s from the application,
//! * and wall-clock timers (media pacing, RTCP, stats).

use crate::cc::GccLite;
use crate::config::{MediaConfig, RtcConfig};
use crate::error::Result;
use crate::event::{Event, TrackKind};
use crate::media::{
    AudioReceiver, AudioSender, SeqCounter, VideoReceiver, VideoSender, SAMPLES_PER_FRAME,
};
use crate::rtcp::{ntp_from_unix, receiver_report, sender_report, LossTracker, TwccRecorder};
use crate::sdp_glue::{RemoteParams, MID_EXT_ID, TWCC_EXT_ID};
use crate::source::{AudioSource, VideoSource};
use crate::state::{DtlsState, IceConnectionState};
use crate::transport::dtls_glue::{now_ms, verify_peer_fingerprint};
use crate::transport::ice_glue::{install_sink_and_gather, setup_agent, WorkerSink};
use crate::transport::srtp_glue::{derive, SrtpContext};
use crate::transport::Inbound;
use ice::{sdp as ice_sdp, IceAgent};
use rtp::rtcp::compound::CompoundPacket;
use rtp::rtcp::RtcpPacket;
use rtp::stats::ReceptionStats;
use rtp::{Extension, Packet, RtpPacketOwned};
use std::collections::HashMap;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Control messages sent from [`crate::PeerConnection`] to the worker.
pub(crate) enum Command {
    /// Attach an outbound audio source.
    AddAudioTrack(Arc<dyn AudioSource>),
    /// Attach an outbound video source.
    AddVideoTrack(Arc<dyn VideoSource>),
    /// Add a remote ICE candidate (trickle), as an `a=candidate:` line value.
    AddRemoteCandidate(String),
    /// Tear the connection down.
    Stop,
}

/// Everything the worker needs to bootstrap, assembled on the control thread.
pub(crate) struct WorkerInit {
    pub config: RtcConfig,
    pub controlling: bool,
    pub local_ufrag: String,
    pub local_pwd: String,
    pub remote: RemoteParams,
    pub dtls: dtls::Connection,
    pub expected_fingerprint: [u8; 32],
    pub is_dtls_client: bool,
    pub local_audio_ssrc: Option<u32>,
    pub local_video_ssrc: Option<u32>,
    pub audio_cfg: Option<MediaConfig>,
    pub video_cfg: Option<MediaConfig>,
    pub audio_source: Option<Arc<dyn AudioSource>>,
    pub video_source: Option<Arc<dyn VideoSource>>,
}

enum Receiver_ {
    Audio(AudioReceiver),
    Video(VideoReceiver),
}

/// Run the worker loop until [`Command::Stop`] or a fatal error.
pub(crate) fn run(init: WorkerInit, cmd_rx: Receiver<Command>, evt_tx: SyncSender<Event>) {
    if let Err(e) = run_inner(init, cmd_rx, &evt_tx) {
        let _ = evt_tx.try_send(Event::Disconnected(e.to_string()));
    }
}

#[allow(clippy::too_many_lines)]
fn run_inner(
    mut init: WorkerInit,
    cmd_rx: Receiver<Command>,
    evt_tx: &SyncSender<Event>,
) -> Result<()> {
    let emit = |e: Event| {
        let _ = evt_tx.try_send(e);
    };

    // --- ICE bring-up -------------------------------------------------------
    let (in_tx, in_rx) = sync_channel::<Inbound>(4096);
    let sink = Arc::new(WorkerSink::new(in_tx));
    let agent = setup_agent(
        &init.config,
        init.controlling,
        &init.local_ufrag,
        &init.local_pwd,
        &init.remote.ice_ufrag,
        &init.remote.ice_pwd,
    )?;
    install_sink_and_gather(&agent, sink)?;
    emit(Event::IceConnectionStateChange(IceConnectionState::Checking));

    // --- senders ------------------------------------------------------------
    let mut audio_sender = match (&init.audio_cfg, init.local_audio_ssrc) {
        (Some(cfg), Some(ssrc)) => {
            Some(AudioSender::new(cfg, ssrc, "0".into(), MID_EXT_ID, TWCC_EXT_ID)?)
        }
        _ => None,
    };
    let mut video_sender = match (&init.video_cfg, init.local_video_ssrc) {
        (Some(cfg), Some(ssrc)) => {
            Some(VideoSender::new(cfg, ssrc, "1".into(), MID_EXT_ID, TWCC_EXT_ID))
        }
        _ => None,
    };
    let audio_pt = init.audio_cfg.as_ref().map(|c| c.payload_type);
    let video_pt = init.video_cfg.as_ref().map(|c| c.payload_type);
    let local_rtcp_ssrc = init
        .local_audio_ssrc
        .or(init.local_video_ssrc)
        .unwrap_or(0x1234_5678);

    // --- mutable transport state -------------------------------------------
    let mut srtp: Option<SrtpContext> = None;
    let mut receivers: HashMap<u32, Receiver_> = HashMap::new();
    let mut recv_stats: HashMap<u32, ReceptionStats> = HashMap::new();
    let mut loss: HashMap<u32, LossTracker> = HashMap::new();
    let mut twcc = TwccRecorder::new(local_rtcp_ssrc, 0);
    let mut twcc_out = SeqCounter::new();
    let mut announced_tracks: HashMap<u32, TrackKind> = HashMap::new();
    let mut audio_cc = GccLite::audio();
    let mut video_cc = GccLite::video();

    // Outbound bookkeeping for Sender Reports + stats.
    let mut audio_send = StreamCounters::default();
    let mut video_send = StreamCounters::default();

    let start = Instant::now();
    let mut ice_was_selected = false;
    let mut audio_source = init.audio_source.take();
    let video_source = init.video_source.take();

    let mut next_audio = Duration::ZERO;
    let mut next_twcc = Duration::from_millis(100);
    let mut next_rr = Duration::from_secs(5);
    let mut next_sr = Duration::from_secs(1);
    let mut next_stats = Duration::from_secs(1);
    let mut emitted_candidates = 0usize;

    loop {
        // --- control commands ---------------------------------------------
        match cmd_rx.try_recv() {
            Ok(Command::Stop) => {
                emit(Event::IceConnectionStateChange(IceConnectionState::Closed));
                return Ok(());
            }
            Ok(Command::AddAudioTrack(src)) => audio_source = Some(src),
            Ok(Command::AddVideoTrack(_src)) => { /* video source set at init */ }
            Ok(Command::AddRemoteCandidate(line)) => {
                if let Ok(c) = ice_sdp::parse(&line) {
                    let _ = agent.add_remote_candidate(c);
                }
            }
            Err(TryRecvError::Disconnected) => return Ok(()),
            Err(TryRecvError::Empty) => {}
        }

        // --- trickle our freshly gathered candidates ----------------------
        let locals = agent.local_candidates();
        if locals.len() > emitted_candidates {
            for c in &locals[emitted_candidates..] {
                emit(Event::LocalCandidate(ice_sdp::to_sdp(c)));
            }
            emitted_candidates = locals.len();
        }

        // --- ICE selection edge -------------------------------------------
        let ice_selected = agent.selected_pair().is_some();
        if ice_selected && !ice_was_selected {
            ice_was_selected = true;
            emit(Event::IceConnectionStateChange(IceConnectionState::Connected));
            emit(Event::DtlsStateChange(DtlsState::Connecting));
        }

        // --- inbound datagrams --------------------------------------------
        let now = now_ms();
        loop {
            match in_rx.try_recv() {
                Ok(Inbound::Dtls(b)) => {
                    let _ = init.dtls.feed_recv(&b, now);
                }
                Ok(Inbound::Srtp(mut b)) => {
                    if let Some(ctx) = &mut srtp {
                        handle_rtp(
                            &mut b,
                            ctx,
                            audio_pt,
                            video_pt,
                            &mut receivers,
                            &mut recv_stats,
                            &mut loss,
                            &mut twcc,
                            &mut announced_tracks,
                            &start,
                            evt_tx,
                            &agent,
                            local_rtcp_ssrc,
                        );
                    }
                }
                Ok(Inbound::Srtcp(b)) => {
                    if let Some(ctx) = &mut srtp {
                        if let Ok(plain) = ctx.rtcp_recv.unprotect(&b) {
                            handle_rtcp(&plain, &mut recv_stats, &start);
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        // --- drive DTLS ----------------------------------------------------
        if ice_selected {
            while let Some(record) = init.dtls.poll_send() {
                let _ = agent.send_on_selected(&record);
            }
            if let Some(deadline) = init.dtls.timeout(now) {
                if now >= deadline {
                    let _ = init.dtls.handle_timeout(now);
                    while let Some(record) = init.dtls.poll_send() {
                        let _ = agent.send_on_selected(&record);
                    }
                }
            }
            if srtp.is_none() && init.dtls.is_handshake_done() {
                verify_peer_fingerprint(&init.dtls, &init.expected_fingerprint)?;
                srtp = Some(derive(&init.dtls, init.is_dtls_client)?);
                emit(Event::DtlsStateChange(DtlsState::Connected));
                emit(Event::IceConnectionStateChange(IceConnectionState::Completed));
                emit(Event::Connected);
            }
        }

        let elapsed = start.elapsed();

        // --- outbound audio (paced at 20 ms) ------------------------------
        if let (Some(ctx), Some(sender), Some(src)) =
            (&mut srtp, &mut audio_sender, audio_source.as_ref())
        {
            while elapsed >= next_audio {
                let mut pcm = [0i16; SAMPLES_PER_FRAME];
                if src.next_block(&mut pcm).is_ok() {
                    if let Ok(pkt) = sender.encode(&pcm, &mut twcc_out) {
                        audio_send.record(&pkt);
                        protect_and_send(&agent, ctx, &pkt);
                    }
                }
                next_audio += Duration::from_millis(20);
            }
        }

        // --- outbound video (drained as frames are produced) --------------
        if let (Some(ctx), Some(sender), Some(src)) =
            (&mut srtp, &mut video_sender, video_source.as_ref())
        {
            while let Some(frame) = src.next_frame(Duration::ZERO) {
                for pkt in sender.encode(&frame, &mut twcc_out) {
                    video_send.record(&pkt);
                    protect_and_send(&agent, ctx, &pkt);
                }
            }
        }

        // --- decode/drain receivers ---------------------------------------
        for recv in receivers.values_mut() {
            match recv {
                Receiver_::Audio(a) => {
                    for f in a.drain() {
                        emit(Event::AudioFrame(f.ssrc, f.pcm, f.sample_rate, f.channels));
                    }
                }
                Receiver_::Video(v) => {
                    for au in v.drain() {
                        emit(Event::VideoFrame(au.ssrc, 0, 0, 0, au.rtp_timestamp));
                        emit(Event::VideoAccessUnit(au.ssrc, au.annex_b, au.rtp_timestamp));
                    }
                }
            }
        }

        // --- periodic TWCC feedback ---------------------------------------
        if elapsed >= next_twcc {
            if let Some(ctx) = &mut srtp {
                if twcc.has_pending() {
                    if let Some(fb) = twcc.build() {
                        let compound = CompoundPacket::new()
                            .add(receiver_report(local_rtcp_ssrc, Vec::new()))
                            .add(fb);
                        send_rtcp(&agent, &mut ctx.rtcp_send, &compound.encode());
                    }
                }
            }
            next_twcc += Duration::from_millis(100);
        }

        // --- periodic receiver reports ------------------------------------
        if elapsed >= next_rr {
            if let Some(ctx) = &mut srtp {
                let now_units = (elapsed.as_secs_f64() * 65536.0) as u32;
                let blocks: Vec<_> =
                    recv_stats.values_mut().map(|s| s.to_report_block(now_units)).collect();
                if !blocks.is_empty() {
                    let rr = receiver_report(local_rtcp_ssrc, blocks);
                    send_rtcp(&agent, &mut ctx.rtcp_send, &rr.encode());
                }
            }
            next_rr += Duration::from_secs(5);
        }

        // --- periodic sender reports --------------------------------------
        if elapsed >= next_sr {
            if let Some(ctx) = &mut srtp {
                let ntp = ntp_now();
                if let Some(ssrc) = init.local_audio_ssrc {
                    if audio_send.packets > 0 {
                        let sr = sender_report(
                            ssrc,
                            ntp,
                            audio_send.last_ts,
                            audio_send.packets,
                            audio_send.octets,
                            Vec::new(),
                        );
                        send_rtcp(&agent, &mut ctx.rtcp_send, &sr.encode());
                    }
                }
                if let Some(ssrc) = init.local_video_ssrc {
                    if video_send.packets > 0 {
                        let sr = sender_report(
                            ssrc,
                            ntp,
                            video_send.last_ts,
                            video_send.packets,
                            video_send.octets,
                            Vec::new(),
                        );
                        send_rtcp(&agent, &mut ctx.rtcp_send, &sr.encode());
                    }
                }
            }
            next_sr += Duration::from_secs(1);
        }

        // --- periodic stats ------------------------------------------------
        if elapsed >= next_stats {
            let mut recv_loss = 0.0_f32;
            let mut jitter = 0u32;
            for s in recv_stats.values_mut() {
                recv_loss = recv_loss.max(f32::from(s.fraction_lost()) / 256.0);
                jitter = jitter.max(s.jitter());
            }
            audio_cc.update(recv_loss);
            let target_video = video_cc.update(recv_loss);
            let stats = crate::stats::Stats {
                audio_send_bitrate: audio_send.bitrate_bps(),
                video_send_bitrate: video_send.bitrate_bps(),
                recv_loss,
                jitter,
                target_video_bitrate: target_video,
                ..crate::stats::Stats::default()
            };
            audio_send.tick();
            video_send.tick();
            emit(Event::Stats(stats));
            next_stats += Duration::from_secs(1);
        }

        std::thread::sleep(Duration::from_millis(3));
    }
}

/// Outbound per-stream counters for Sender Reports and bitrate stats.
#[derive(Default)]
struct StreamCounters {
    packets: u32,
    octets: u32,
    last_ts: u32,
    octets_at_tick: u32,
}

impl StreamCounters {
    fn record(&mut self, pkt: &RtpPacketOwned) {
        self.packets = self.packets.wrapping_add(1);
        self.octets = self.octets.wrapping_add(pkt.payload().len() as u32);
        self.last_ts = pkt.header().timestamp();
    }

    /// Bits sent since the previous [`Self::tick`].
    fn bitrate_bps(&self) -> u32 {
        self.octets.wrapping_sub(self.octets_at_tick).wrapping_mul(8)
    }

    fn tick(&mut self) {
        self.octets_at_tick = self.octets;
    }
}

/// 64-bit NTP timestamp for the current wall clock.
fn ntp_now() -> u64 {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    ntp_from_unix(d.as_secs(), d.subsec_nanos())
}

#[allow(clippy::too_many_arguments)]
fn handle_rtp(
    buf: &mut [u8],
    ctx: &mut SrtpContext,
    audio_pt: Option<u8>,
    video_pt: Option<u8>,
    receivers: &mut HashMap<u32, Receiver_>,
    recv_stats: &mut HashMap<u32, ReceptionStats>,
    loss: &mut HashMap<u32, LossTracker>,
    twcc: &mut TwccRecorder,
    announced: &mut HashMap<u32, TrackKind>,
    start: &Instant,
    evt_tx: &SyncSender<Event>,
    agent: &IceAgent,
    local_rtcp_ssrc: u32,
) {
    // The RTP header (incl. SSRC) is in the clear; read it to select the
    // per-SSRC SRTP context before authenticating/decrypting.
    let Some(pkt_ssrc) = rtp::RtpPacket::parse(buf).ok().map(|p| p.header().ssrc()) else {
        return;
    };
    let Ok(session) = ctx.rtp_recv(pkt_ssrc) else {
        return;
    };
    let (ssrc, pt, seq, ts, wire, twcc_seq) = match session.unprotect(buf) {
        Ok(rtp) => {
            let exts = rtp.extensions();
            let twcc_seq = exts.iter().find_map(|e| match e {
                Extension::Twcc { seq, .. } => Some(*seq),
                _ => None,
            });
            (
                rtp.header().ssrc(),
                rtp.header().payload_type(),
                rtp.header().sequence_number(),
                rtp.header().timestamp(),
                rtp.as_bytes().to_vec(),
                twcc_seq,
            )
        }
        Err(_) => return,
    };

    let arrival = start.elapsed();
    recv_stats
        .entry(ssrc)
        .or_insert_with(|| ReceptionStats::new(ssrc, seq))
        .on_packet(seq, ts, arrival.as_millis() as u32);

    if let Some(tseq) = twcc_seq {
        twcc.record(tseq, arrival.as_micros() as i64);
    }

    // NACK forward gaps; PLI on video loss.
    let newly_lost = loss.entry(ssrc).or_default().observe(seq);
    if !newly_lost.is_empty() {
        let is_video = Some(pt) == video_pt;
        let fb = if is_video {
            crate::rtcp::pli(ssrc)
        } else {
            crate::rtcp::nack(ssrc, &newly_lost)
        };
        let compound = CompoundPacket::new()
            .add(receiver_report(local_rtcp_ssrc, Vec::new()))
            .add(fb);
        send_rtcp(agent, &mut ctx.rtcp_send, &compound.encode());
    }

    let kind = if Some(pt) == audio_pt {
        TrackKind::Audio
    } else if Some(pt) == video_pt {
        TrackKind::Video
    } else {
        return;
    };

    if announced.insert(ssrc, kind).is_none() {
        let _ = evt_tx.try_send(Event::TrackAdded(kind, ssrc));
    }

    if !receivers.contains_key(&ssrc) {
        // A remote SSRC can mint a receiver here; never let a decoder-init
        // failure panic the worker thread — drop the packet instead.
        let receiver = match kind {
            TrackKind::Audio => match AudioReceiver::new(&MediaConfig::opus_default(), ssrc) {
                Ok(a) => Receiver_::Audio(a),
                Err(_) => return,
            },
            TrackKind::Video => Receiver_::Video(VideoReceiver::new(ssrc)),
        };
        receivers.insert(ssrc, receiver);
    }
    let entry = receivers.get_mut(&ssrc).expect("just inserted");
    match entry {
        Receiver_::Audio(a) => a.push(seq, wire),
        Receiver_::Video(v) => v.push(seq, wire),
    }
}

fn handle_rtcp(plain: &[u8], recv_stats: &mut HashMap<u32, ReceptionStats>, start: &Instant) {
    let Ok(compound) = CompoundPacket::parse_lenient(plain) else {
        return;
    };
    let now_units = (start.elapsed().as_secs_f64() * 65536.0) as u32;
    for pkt in compound.packets {
        if let RtcpPacket::SenderReport(sr) = pkt {
            if let Some(stats) = recv_stats.get_mut(&sr.ssrc) {
                let ntp_middle = ((sr.ntp_timestamp >> 16) & 0xFFFF_FFFF) as u32;
                stats.on_sender_report(ntp_middle, now_units);
            }
        }
    }
}

fn protect_and_send(agent: &IceAgent, ctx: &mut SrtpContext, pkt: &RtpPacketOwned) {
    let plain_len = pkt.as_bytes().len();
    let mut out = vec![0u8; plain_len + 64];
    let Ok(session) = ctx.rtp_send(pkt.header().ssrc()) else {
        return;
    };
    if let Ok(n) = session.protect(pkt, &mut out) {
        let _ = agent.send_on_selected(&out[..n]);
    }
}

fn send_rtcp(agent: &IceAgent, session: &mut srtp::SrtcpSession, plain: &[u8]) {
    let mut out = vec![0u8; plain.len() + 64];
    if let Ok(n) = session.protect(plain, &mut out) {
        let _ = agent.send_on_selected(&out[..n]);
    }
}
