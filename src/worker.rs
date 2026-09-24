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
//! * and timers on the configured clock (media pacing, RTCP, stats, the DTLS
//!   handshake deadline).

use crate::cc::GccLite;
use crate::config::{MediaConfig, RtcConfig};
use crate::error::{Result, RtcError};
use crate::event::{Event, EventQueue, TrackKind};
use crate::media::history::{SendHistory, AUDIO_HISTORY, MAX_RETRANSMIT_PER_NACK, VIDEO_HISTORY};
use crate::media::{
    AudioReceiver, AudioSender, SeqCounter, VideoReceiver, VideoSender, SAMPLES_PER_FRAME,
};
use crate::rtcp::{
    nack, ntp_from_unix, pli, read_feedback, receiver_report, sender_report, Feedback, LocalSsrcs,
    LossTracker, Observation, TwccRecorder,
};
use crate::sdp_glue::{Negotiated, RemoteParams};
use crate::source::{AudioSource, VideoSource};
use crate::state::{ConnectionState, DtlsRole, DtlsState, IceConnectionState};
use crate::stats::Stats;
use crate::transport::ice_glue::{
    install_sink_and_gather, setup_agent, AgentParams, WorkerSink, INBOUND_QUEUE,
};
use crate::transport::srtp_glue::{derive, SrtpContext};
use crate::transport::{Inbound, InboundKind};
use ice::{sdp as ice_sdp, GatherState, IceAgent};
use rtp::rtcp::compound::CompoundPacket;
use rtp::rtcp::RtcpPacket;
use rtp::stats::ReceptionStats;
use rtp::{Extension, Packet, RtpPacketOwned};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

const TICK: Duration = Duration::from_millis(3);
const AUDIO_FRAME: Duration = Duration::from_millis(20);
const MAX_AUDIO_BACKLOG: Duration = Duration::from_millis(100);
const MAX_VIDEO_FRAMES_PER_TICK: usize = 8;
const TWCC_INTERVAL: Duration = Duration::from_millis(100);
const REPORT_INTERVAL: Duration = Duration::from_secs(1);
const STATS_INTERVAL: Duration = Duration::from_secs(1);
const MAX_REPORT_BLOCKS: usize = 31;
const SRTP_OVERHEAD: usize = 64;

/// Control messages sent from [`crate::PeerConnection`] to the worker.
pub(crate) enum Command {
    /// Attach an outbound audio source.
    AddAudioTrack(Arc<dyn AudioSource>),
    /// Attach an outbound video source.
    AddVideoTrack(Arc<dyn VideoSource>),
    /// Add a remote ICE candidate (trickle).
    AddRemoteCandidate(ice::Candidate),
    RemoteEndOfCandidates,
    /// Tear the connection down.
    Stop,
}

/// Everything the worker needs to bootstrap, assembled on the control thread.
pub(crate) struct WorkerInit {
    pub config: RtcConfig,
    pub controlling: bool,
    pub local_ufrag: String,
    pub local_pwd: Zeroizing<String>,
    pub remote: RemoteParams,
    pub remote_candidates: Vec<ice::Candidate>,
    pub negotiated: Negotiated,
    pub dtls: dtls::Connection,
    pub dtls_role: DtlsRole,
    pub local_audio_ssrc: Option<u32>,
    pub local_video_ssrc: Option<u32>,
    pub rtcp_ssrc: u32,
    pub audio_source: Option<Arc<dyn AudioSource>>,
    pub video_source: Option<Arc<dyn VideoSource>>,
    pub events: Arc<EventQueue>,
    pub state: Arc<Mutex<ConnectionState>>,
}

enum Decoder {
    Audio(AudioReceiver),
    Video(VideoReceiver),
}

struct RemoteStream {
    kind: TrackKind,
    clock_rate: u64,
    stats: ReceptionStats,
    loss: LossTracker,
    decoder: Decoder,
    fraction_lost: u8,
}

struct Outbound<S> {
    sender: S,
    ssrc: u32,
    history: SendHistory,
    counters: StreamCounters,
}

#[derive(Default)]
struct LinkFeedback {
    audio_fraction_lost: Option<u8>,
    video_fraction_lost: Option<u8>,
    twcc_received: u64,
    twcc_lost: u64,
}

impl LinkFeedback {
    fn absorb(&mut self, feedback: &Feedback) {
        self.audio_fraction_lost = feedback.audio_fraction_lost.or(self.audio_fraction_lost);
        self.video_fraction_lost = feedback.video_fraction_lost.or(self.video_fraction_lost);
        self.twcc_received += feedback.twcc_received;
        self.twcc_lost += feedback.twcc_lost;
    }

    fn video_loss(&self) -> Option<f32> {
        let total = self.twcc_received + self.twcc_lost;
        if total > 0 {
            Some(ratio(self.twcc_lost, total))
        } else {
            self.video_fraction_lost.map(|f| f32::from(f) / 256.0)
        }
    }
}

fn ratio(part: u64, total: u64) -> f32 {
    let scaled = part.saturating_mul(10_000) / total.max(1);
    f32::from(u16::try_from(scaled).unwrap_or(10_000)) / 10_000.0
}

/// Run the worker loop until [`Command::Stop`] or a fatal error.
pub(crate) fn run(init: WorkerInit, commands: Receiver<Command>) {
    let events = Arc::clone(&init.events);
    let state = Arc::clone(&init.state);
    match Worker::start(init) {
        Ok(mut worker) => worker.run(&commands),
        Err(error) => {
            set_state(&state, &events, ConnectionState::Failed);
            events.push(Event::Failed(error));
        }
    }
    events.close();
}

fn set_state(state: &Mutex<ConnectionState>, events: &EventQueue, next: ConnectionState) {
    let mut current = state.lock().unwrap_or_else(PoisonError::into_inner);
    if *current != next {
        *current = next;
        drop(current);
        events.push(Event::ConnectionStateChange(next));
    }
}

struct Worker {
    events: Arc<EventQueue>,
    state: Arc<Mutex<ConnectionState>>,
    clock: Arc<dyn ice::Clock>,
    start: Instant,
    agent: IceAgent,
    sink: Arc<WorkerSink>,
    ice_failed: Arc<AtomicBool>,
    inbound: Receiver<Inbound>,
    dtls: dtls::Connection,
    dtls_role: DtlsRole,
    handshake_timeout: Duration,
    srtp: Option<SrtpContext>,
    own_ssrcs: Vec<u32>,
    selected_at: Option<Duration>,
    selected_remote: Option<SocketAddr>,
    emitted_candidates: usize,
    gathering_complete: bool,
    negotiated: Negotiated,
    local: LocalSsrcs,
    rtcp_ssrc: u32,
    audio_source: Option<Arc<dyn AudioSource>>,
    video_source: Option<Arc<dyn VideoSource>>,
    audio: Option<Outbound<AudioSender>>,
    video: Option<Outbound<VideoSender>>,
    audio_bitrate: u32,
    twcc_out: SeqCounter,
    twcc_in: TwccRecorder,
    twcc_feedback: bool,
    remote: HashMap<u32, RemoteStream>,
    last_fir_seq: Option<u8>,
    audio_cc: GccLite,
    video_cc: GccLite,
    link: LinkFeedback,
    rtt_ms: u32,
    remb_bps: Option<u64>,
    stats: Stats,
    audio_recv_octets: u64,
    video_recv_octets: u64,
    next_audio: Duration,
    next_twcc: Duration,
    next_report: Duration,
    next_stats: Duration,
}

impl Worker {
    fn start(mut init: WorkerInit) -> Result<Self> {
        let (tx, inbound) = sync_channel::<Inbound>(INBOUND_QUEUE);
        let sink = Arc::new(WorkerSink::new(tx));
        let ice_failed = Arc::new(AtomicBool::new(false));
        let agent = setup_agent(&AgentParams {
            config: &init.config,
            controlling: init.controlling,
            local_ufrag: &init.local_ufrag,
            local_pwd: &init.local_pwd,
            remote_ufrag: &init.remote.ice_ufrag,
            remote_pwd: &init.remote.ice_pwd,
        })?;
        install_sink_and_gather(&agent, Arc::clone(&sink), Arc::clone(&ice_failed))?;
        for candidate in init.remote_candidates.drain(..) {
            if let Err(error) = agent.add_remote_candidate(candidate) {
                init.events.push(Event::Error(error.into()));
            }
        }
        if init.remote.end_of_candidates {
            agent.remote_end_of_candidates();
        }

        let audio = match (
            &init.config.audio,
            init.local_audio_ssrc,
            &init.negotiated.audio,
        ) {
            (Some(cfg), Some(ssrc), Some(neg)) if neg.send => {
                let cfg = MediaConfig {
                    payload_type: neg.payload_type,
                    ..cfg.clone()
                };
                Some(Outbound {
                    sender: AudioSender::new(
                        &cfg,
                        ssrc,
                        neg.mid.clone(),
                        neg.mid_ext_id,
                        neg.twcc_ext_id,
                    )?,
                    ssrc,
                    history: SendHistory::new(AUDIO_HISTORY),
                    counters: StreamCounters::default(),
                })
            }
            _ => None,
        };
        let video = match (
            &init.config.video,
            init.local_video_ssrc,
            &init.negotiated.video,
        ) {
            (Some(cfg), Some(ssrc), Some(neg)) if neg.send => {
                let cfg = MediaConfig {
                    payload_type: neg.payload_type,
                    ..cfg.clone()
                };
                Some(Outbound {
                    sender: VideoSender::new(
                        &cfg,
                        ssrc,
                        neg.mid.clone(),
                        neg.mid_ext_id,
                        neg.twcc_ext_id,
                    ),
                    ssrc,
                    history: SendHistory::new(VIDEO_HISTORY),
                    counters: StreamCounters::default(),
                })
            }
            _ => None,
        };
        let audio_bitrate = init.config.audio.as_ref().map_or(0, |c| c.bitrate);
        let video_bitrate = init.config.video.as_ref().map_or(0, |c| c.bitrate);
        let local = LocalSsrcs {
            audio: audio.as_ref().map(|a| a.ssrc),
            video: video.as_ref().map(|v| v.ssrc),
        };
        let mut own_ssrcs = vec![init.rtcp_ssrc];
        own_ssrcs.extend(init.local_audio_ssrc);
        own_ssrcs.extend(init.local_video_ssrc);
        let twcc_feedback = init.negotiated.twcc_ext_id().is_some()
            && [&init.negotiated.audio, &init.negotiated.video]
                .into_iter()
                .flatten()
                .any(|m| m.transport_cc);
        let clock = Arc::clone(&init.config.clock);
        let start = clock.now();

        Ok(Self {
            events: init.events,
            state: init.state,
            clock,
            start,
            agent,
            sink,
            ice_failed,
            inbound,
            dtls: init.dtls,
            dtls_role: init.dtls_role,
            handshake_timeout: init.config.handshake_timeout,
            srtp: None,
            own_ssrcs,
            selected_at: None,
            selected_remote: None,
            emitted_candidates: 0,
            gathering_complete: false,
            negotiated: init.negotiated,
            local,
            rtcp_ssrc: init.rtcp_ssrc,
            audio_source: init.audio_source,
            video_source: init.video_source,
            audio,
            video,
            audio_bitrate,
            twcc_out: SeqCounter::new(),
            twcc_in: TwccRecorder::new(init.rtcp_ssrc, 0),
            twcc_feedback,
            remote: HashMap::new(),
            last_fir_seq: None,
            audio_cc: GccLite::capped(GccLite::audio(), audio_bitrate),
            video_cc: GccLite::capped(GccLite::video(), video_bitrate),
            link: LinkFeedback::default(),
            rtt_ms: 0,
            remb_bps: None,
            stats: Stats::default(),
            audio_recv_octets: 0,
            video_recv_octets: 0,
            next_audio: Duration::ZERO,
            next_twcc: Duration::ZERO,
            next_report: Duration::ZERO,
            next_stats: STATS_INTERVAL,
        })
    }

    fn run(&mut self, commands: &Receiver<Command>) {
        self.set_state(ConnectionState::Connecting);
        self.events.push(Event::IceConnectionStateChange(
            IceConnectionState::Checking,
        ));
        loop {
            match self.step(commands) {
                Ok(true) => std::thread::sleep(TICK),
                Ok(false) => {
                    self.events
                        .push(Event::IceConnectionStateChange(IceConnectionState::Closed));
                    self.events.push(Event::DtlsStateChange(DtlsState::Closed));
                    self.set_state(ConnectionState::Closed);
                    return;
                }
                Err(error) => {
                    self.fail(error);
                    return;
                }
            }
        }
    }

    fn fail(&mut self, error: RtcError) {
        match &error {
            RtcError::ConsentExpired | RtcError::IceFailed | RtcError::Ice(_) => {
                self.events
                    .push(Event::IceConnectionStateChange(IceConnectionState::Failed));
            }
            RtcError::Dtls(_) | RtcError::FingerprintMismatch | RtcError::Timeout(_) => {
                self.events.push(Event::DtlsStateChange(DtlsState::Failed));
            }
            _ => {}
        }
        self.set_state(ConnectionState::Failed);
        self.events.push(Event::Failed(error));
    }

    fn set_state(&self, next: ConnectionState) {
        set_state(&self.state, &self.events, next);
    }

    fn elapsed(&self) -> Duration {
        self.clock.now().saturating_duration_since(self.start)
    }

    fn step(&mut self, commands: &Receiver<Command>) -> Result<bool> {
        loop {
            match commands.try_recv() {
                Ok(Command::Stop) | Err(TryRecvError::Disconnected) => return Ok(false),
                Ok(Command::AddAudioTrack(source)) => self.audio_source = Some(source),
                Ok(Command::AddVideoTrack(source)) => self.video_source = Some(source),
                Ok(Command::AddRemoteCandidate(candidate)) => {
                    if let Err(error) = self.agent.add_remote_candidate(candidate) {
                        self.events.push(Event::Error(error.into()));
                    }
                }
                Ok(Command::RemoteEndOfCandidates) => self.agent.remote_end_of_candidates(),
                Err(TryRecvError::Empty) => break,
            }
        }
        self.trickle();
        if self.ice_failed.load(Ordering::Acquire) {
            return Err(if self.selected_remote.is_some() {
                RtcError::ConsentExpired
            } else {
                RtcError::IceFailed
            });
        }
        let now = self.elapsed();
        self.track_selection(now);
        self.drain_inbound(now)?;
        self.drive_dtls(now)?;
        if self.srtp.is_some() {
            self.send_media(now)?;
            self.send_periodic_rtcp(now)?;
        }
        self.drain_decoders();
        if now >= self.next_stats {
            self.next_stats = now + STATS_INTERVAL;
            self.emit_stats();
        }
        Ok(true)
    }

    fn trickle(&mut self) {
        let complete = self.agent.gathering_state() == GatherState::Complete;
        let locals = self.agent.local_candidates();
        if locals.len() > self.emitted_candidates {
            for candidate in &locals[self.emitted_candidates..] {
                self.events
                    .push(Event::LocalCandidate(ice_sdp::to_sdp(candidate)));
            }
            self.emitted_candidates = locals.len();
        }
        if complete && !self.gathering_complete {
            self.gathering_complete = true;
            self.agent.end_of_candidates();
            self.events.push(Event::IceGatheringComplete);
        }
    }

    fn track_selection(&mut self, now: Duration) {
        let Some((_, remote)) = self.agent.selected_pair() else {
            return;
        };
        if self.selected_remote.is_none() {
            self.selected_at = Some(now);
            self.events.push(Event::IceConnectionStateChange(
                IceConnectionState::Connected,
            ));
            self.events
                .push(Event::DtlsStateChange(DtlsState::Connecting));
        }
        self.selected_remote = Some(remote.addr);
    }

    fn drain_inbound(&mut self, now: Duration) -> Result<()> {
        while let Ok(Inbound {
            kind,
            mut bytes,
            from,
        }) = self.inbound.try_recv()
        {
            let expected = match self.selected_remote {
                Some(remote) => remote == from,
                None => kind == InboundKind::Dtls,
            };
            if !expected {
                self.stats.dropped_unexpected_source += 1;
                continue;
            }
            match kind {
                InboundKind::Dtls => {
                    if let Err(error) = self.dtls.feed_recv(&bytes, from, millis(now)) {
                        self.flush_alert();
                        return Err(error.into());
                    }
                }
                InboundKind::Srtp => self.handle_rtp(&mut bytes, now)?,
                InboundKind::Srtcp => self.handle_rtcp(&bytes, now)?,
            }
        }
        Ok(())
    }

    fn drive_dtls(&mut self, now: Duration) -> Result<()> {
        let Some(selected_at) = self.selected_at else {
            return Ok(());
        };
        if self.dtls.timeout(millis(now)) == Some(0) {
            if let Err(error) = self.dtls.handle_timeout(millis(now)) {
                self.flush_alert();
                return Err(error.into());
            }
        }
        self.flush_dtls()?;
        if self.srtp.is_some() {
            return Ok(());
        }
        if self.dtls.is_handshake_done() {
            self.srtp = Some(derive(&self.dtls, self.dtls_role, self.own_ssrcs.clone())?);
            self.events
                .push(Event::DtlsStateChange(DtlsState::Connected));
            self.events.push(Event::IceConnectionStateChange(
                IceConnectionState::Completed,
            ));
            self.set_state(ConnectionState::Connected);
            self.events.push(Event::Connected);
            self.next_audio = now;
        } else if now.saturating_sub(selected_at) >= self.handshake_timeout {
            return Err(RtcError::Timeout("DTLS handshake"));
        }
        Ok(())
    }

    fn flush_dtls(&mut self) -> Result<()> {
        while let Some(record) = self.dtls.poll_send() {
            self.transmit(&record)?;
        }
        Ok(())
    }

    fn flush_alert(&mut self) {
        if self.selected_remote.is_some() {
            if let Err(error) = self.flush_dtls() {
                self.events.push(Event::Error(error));
            }
        }
    }

    fn transmit(&mut self, bytes: &[u8]) -> Result<()> {
        match self.agent.send_on_selected(bytes) {
            Ok(()) => Ok(()),
            Err(ice::Error::ConsentExpired) => Err(RtcError::ConsentExpired),
            Err(ice::Error::Failed) => Err(RtcError::IceFailed),
            Err(_) => {
                self.stats.send_errors += 1;
                Ok(())
            }
        }
    }

    fn handle_rtp(&mut self, bytes: &mut [u8], now: Duration) -> Result<()> {
        let Some(ctx) = self.srtp.as_mut() else {
            return Ok(());
        };
        let twcc_id = self.negotiated.twcc_ext_id();
        let (packet, evicted) = match ctx.unprotect_rtp(bytes) {
            Ok(unprotected) => unprotected,
            Err(_) => {
                self.stats.srtp_rejected += 1;
                return Ok(());
            }
        };
        let header = packet.header();
        let (ssrc, pt, seq, ts) = (
            header.ssrc(),
            header.payload_type(),
            header.sequence_number(),
            header.timestamp(),
        );
        let payload_len = packet.payload().len() as u64;
        let twcc_seq = twcc_id.and_then(|id| {
            packet
                .extensions()
                .iter()
                .find(|e| e.id() == id)
                .and_then(Extension::as_twcc)
        });
        let wire = packet.as_bytes().to_vec();

        if let Some(gone) = evicted {
            self.remote.remove(&gone);
        }
        if let Some(twcc_seq) = twcc_seq {
            self.twcc_in.record(twcc_seq, micros(now));
        }

        let (kind, media) = if let Some(m) = self
            .negotiated
            .audio
            .as_ref()
            .filter(|m| m.payload_type == pt)
        {
            (TrackKind::Audio, m)
        } else if let Some(m) = self
            .negotiated
            .video
            .as_ref()
            .filter(|m| m.payload_type == pt)
        {
            (TrackKind::Video, m)
        } else {
            return Ok(());
        };
        if !media.recv {
            return Ok(());
        }
        let (can_nack, can_pli) = (media.nack, media.pli);

        let stream = match self.remote.entry(ssrc) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                // A remote SSRC can mint a receiver here; never let a decoder-init
                // failure panic the worker thread — drop the packet instead.
                let (decoder, clock_rate) = match kind {
                    TrackKind::Audio => {
                        match AudioReceiver::new(&MediaConfig::opus_default(), ssrc) {
                            Ok(receiver) => (Decoder::Audio(receiver), 48_000),
                            Err(error) => {
                                self.events.push(Event::Error(error));
                                return Ok(());
                            }
                        }
                    }
                    TrackKind::Video => (Decoder::Video(VideoReceiver::new(ssrc)), 90_000),
                };
                self.events.push(Event::TrackAdded(kind, ssrc));
                entry.insert(RemoteStream {
                    kind,
                    clock_rate,
                    stats: ReceptionStats::new(ssrc, seq),
                    loss: LossTracker::default(),
                    decoder,
                    fraction_lost: 0,
                })
            }
        };
        if stream.kind != kind {
            return Ok(());
        }
        stream
            .stats
            .on_packet(seq, ts, media_clock(now, stream.clock_rate));
        let observation = stream.loss.observe(seq);
        match &mut stream.decoder {
            Decoder::Audio(a) => a.push(seq, wire),
            Decoder::Video(v) => v.push(seq, wire),
        }
        match kind {
            TrackKind::Audio => self.audio_recv_octets += payload_len,
            TrackKind::Video => self.video_recv_octets += payload_len,
        }

        let request = match observation {
            Observation::Lost(lost) if can_nack => Some(nack(self.rtcp_ssrc, ssrc, &lost)),
            Observation::Lost(_) | Observation::Discontinuity
                if kind == TrackKind::Video && can_pli =>
            {
                Some(pli(self.rtcp_ssrc, ssrc))
            }
            _ => None,
        };
        if let Some(request) = request {
            self.send_rtcp(vec![receiver_report(self.rtcp_ssrc, Vec::new()), request])?;
        }
        Ok(())
    }

    fn handle_rtcp(&mut self, bytes: &[u8], now: Duration) -> Result<()> {
        let Some(ctx) = self.srtp.as_mut() else {
            return Ok(());
        };
        let plain = match ctx.unprotect_rtcp(bytes) {
            Ok(plain) => plain,
            Err(_) => {
                self.stats.srtp_rejected += 1;
                return Ok(());
            }
        };
        let Ok(compound) = CompoundPacket::parse_lenient(&plain) else {
            return Ok(());
        };
        let feedback = read_feedback(
            &compound.packets,
            self.local,
            ntp_middle(ntp_now()),
            &mut self.last_fir_seq,
        );
        let arrival = units_65536(now);
        for (ssrc, middle) in &feedback.sender_reports {
            if let Some(stream) = self.remote.get_mut(ssrc) {
                stream.stats.on_sender_report(*middle, arrival);
            }
        }
        for (ssrc, lost) in &feedback.nacks {
            self.retransmit(*ssrc, lost)?;
        }
        if feedback.keyframe_requested {
            self.stats.keyframe_requests += 1;
            if let Some(source) = &self.video_source {
                source.request_keyframe();
            }
        }
        if let Some(rtt) = feedback.rtt_ms {
            self.rtt_ms = rtt;
        }
        if let Some(remb) = feedback.remb_bps {
            self.remb_bps = Some(remb);
        }
        self.link.absorb(&feedback);
        Ok(())
    }

    fn retransmit(&mut self, ssrc: u32, lost: &[u16]) -> Result<()> {
        self.stats.nacks_received += 1;
        let history = match (&self.audio, &self.video) {
            (Some(a), _) if a.ssrc == ssrc => &a.history,
            (_, Some(v)) if v.ssrc == ssrc => &v.history,
            _ => return Ok(()),
        };
        let resend: Vec<Vec<u8>> = lost
            .iter()
            .filter_map(|&seq| history.get(seq).map(<[u8]>::to_vec))
            .take(MAX_RETRANSMIT_PER_NACK)
            .collect();
        for bytes in resend {
            self.transmit(&bytes)?;
            self.stats.packets_retransmitted += 1;
        }
        Ok(())
    }

    fn send_media(&mut self, now: Duration) -> Result<()> {
        let mut outgoing = Vec::new();
        if let (Some(out), Some(source)) = (self.audio.as_mut(), self.audio_source.as_ref()) {
            if now > self.next_audio + MAX_AUDIO_BACKLOG {
                self.next_audio = now;
            }
            while now >= self.next_audio {
                let mut pcm = [0i16; SAMPLES_PER_FRAME];
                if source.next_block(&mut pcm).is_ok() {
                    match out.sender.encode(&pcm, &mut self.twcc_out) {
                        Ok(pkt) => outgoing.push(pkt),
                        Err(error) => self.events.push(Event::Error(error)),
                    }
                }
                self.next_audio += AUDIO_FRAME;
            }
        }
        if let (Some(out), Some(source)) = (self.video.as_mut(), self.video_source.as_ref()) {
            for _ in 0..MAX_VIDEO_FRAMES_PER_TICK {
                let Some(frame) = source.next_frame(Duration::ZERO) else {
                    break;
                };
                match out.sender.encode(&frame, &mut self.twcc_out) {
                    Ok(pkts) => outgoing.extend(pkts),
                    Err(error) => self.events.push(Event::Error(error)),
                }
            }
        }
        for pkt in &outgoing {
            self.send_rtp(pkt)?;
        }
        Ok(())
    }

    fn send_rtp(&mut self, pkt: &RtpPacketOwned) -> Result<()> {
        let Some(ctx) = self.srtp.as_mut() else {
            return Ok(());
        };
        let mut out = vec![0u8; pkt.as_bytes().len() + SRTP_OVERHEAD];
        let n = ctx.protect_rtp(pkt, &mut out)?;
        out.truncate(n);
        let (ssrc, seq) = (pkt.header().ssrc(), pkt.header().sequence_number());
        if let Some(a) = self.audio.as_mut().filter(|a| a.ssrc == ssrc) {
            a.history.record(seq, &out);
            a.counters.record(pkt);
        } else if let Some(v) = self.video.as_mut().filter(|v| v.ssrc == ssrc) {
            v.history.record(seq, &out);
            v.counters.record(pkt);
        }
        self.transmit(&out)
    }

    fn send_rtcp(&mut self, packets: Vec<RtcpPacket>) -> Result<()> {
        let Some(ctx) = self.srtp.as_mut() else {
            return Ok(());
        };
        let plain = match (CompoundPacket { packets }).encode() {
            Ok(plain) => plain,
            Err(error) => {
                self.events.push(Event::Error(error.into()));
                return Ok(());
            }
        };
        let mut out = vec![0u8; plain.len() + SRTP_OVERHEAD];
        let n = ctx.protect_rtcp(&plain, &mut out)?;
        self.transmit(&out[..n])
    }

    fn send_periodic_rtcp(&mut self, now: Duration) -> Result<()> {
        if now >= self.next_twcc {
            self.next_twcc = now + TWCC_INTERVAL;
            if self.twcc_feedback && self.twcc_in.has_pending() {
                for feedback in self.twcc_in.build() {
                    self.send_rtcp(vec![receiver_report(self.rtcp_ssrc, Vec::new()), feedback])?;
                }
            }
        }
        if now < self.next_report {
            return Ok(());
        }
        self.next_report = now + REPORT_INTERVAL;
        let arrival = units_65536(now);
        let mut blocks = Vec::new();
        for stream in self.remote.values_mut().take(MAX_REPORT_BLOCKS) {
            let block = stream.stats.to_report_block(arrival);
            stream.fraction_lost = block.fraction_lost;
            blocks.push(block);
        }
        let ntp = ntp_now();
        let mut packets = Vec::new();
        let senders = [
            self.audio.as_ref().map(|a| (a.ssrc, &a.counters)),
            self.video.as_ref().map(|v| (v.ssrc, &v.counters)),
        ];
        for (ssrc, c) in senders.into_iter().flatten() {
            if c.packets > 0 {
                packets.push(sender_report(
                    ssrc,
                    ntp,
                    c.last_ts,
                    c.packets,
                    c.octets,
                    std::mem::take(&mut blocks),
                ));
            }
        }
        if packets.is_empty() {
            packets.push(receiver_report(self.rtcp_ssrc, blocks));
        }
        self.send_rtcp(packets)
    }

    fn drain_decoders(&mut self) {
        for stream in self.remote.values_mut() {
            match &mut stream.decoder {
                Decoder::Audio(a) => {
                    for f in a.drain() {
                        self.events.push(Event::AudioFrame(
                            f.ssrc,
                            f.pcm,
                            f.sample_rate,
                            f.channels,
                        ));
                    }
                }
                Decoder::Video(v) => {
                    for au in v.drain() {
                        self.events.push(Event::VideoAccessUnit(
                            au.ssrc,
                            au.annex_b,
                            au.rtp_timestamp,
                        ));
                    }
                }
            }
        }
    }

    fn emit_stats(&mut self) {
        let mut recv_loss = 0.0_f32;
        let mut jitter = 0u32;
        for stream in self.remote.values() {
            recv_loss = recv_loss.max(f32::from(stream.fraction_lost) / 256.0);
            jitter = jitter.max(stream.stats.jitter());
        }
        let link = std::mem::take(&mut self.link);
        let audio_loss = link.audio_fraction_lost.map(|f| f32::from(f) / 256.0);
        let video_loss = link.video_loss();
        if let (Some(out), Some(loss)) = (self.audio.as_mut(), audio_loss) {
            let target = self.audio_cc.update(loss);
            if target != self.audio_bitrate {
                match out.sender.set_bitrate(target) {
                    Ok(()) => self.audio_bitrate = target,
                    Err(error) => self.events.push(Event::Error(error)),
                }
            }
        }
        if let Some(loss) = video_loss {
            self.video_cc.update(loss);
        }
        let remb = self.remb_bps.map(|b| u32::try_from(b).unwrap_or(u32::MAX));
        let target_video = remb.map_or(self.video_cc.target_bps(), |r| {
            r.min(self.video_cc.target_bps())
        });
        let (audio_send, video_send) = (
            self.audio.as_mut().map_or(0, |a| a.counters.tick()),
            self.video.as_mut().map_or(0, |v| v.counters.tick()),
        );
        let stats = Stats {
            audio_send_bitrate: audio_send,
            video_send_bitrate: video_send,
            audio_recv_bitrate: u32::try_from(std::mem::take(&mut self.audio_recv_octets) * 8)
                .unwrap_or(u32::MAX),
            video_recv_bitrate: u32::try_from(std::mem::take(&mut self.video_recv_octets) * 8)
                .unwrap_or(u32::MAX),
            recv_loss,
            send_loss: audio_loss.into_iter().chain(video_loss).fold(0.0, f32::max),
            jitter,
            rtt_ms: self.rtt_ms,
            target_video_bitrate: target_video,
            remb_bitrate: remb.unwrap_or(0),
            target_audio_bitrate: self.audio_bitrate,
            remote_streams: u32::try_from(self.remote.len()).unwrap_or(u32::MAX),
            dropped_inbound: self.sink.dropped(),
            dropped_events: self.events.dropped(),
            ..self.stats
        };
        self.events.push(Event::Stats(stats));
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
        self.octets = self
            .octets
            .wrapping_add(u32::try_from(pkt.payload().len()).unwrap_or(u32::MAX));
        self.last_ts = pkt.header().timestamp();
    }

    fn tick(&mut self) -> u32 {
        let bits = self
            .octets
            .wrapping_sub(self.octets_at_tick)
            .wrapping_mul(8);
        self.octets_at_tick = self.octets;
        bits
    }
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

fn micros(elapsed: Duration) -> i64 {
    i64::try_from(elapsed.as_micros()).unwrap_or(i64::MAX)
}

fn units_65536(elapsed: Duration) -> u32 {
    ((elapsed.as_micros() << 16) / 1_000_000) as u32
}

fn media_clock(elapsed: Duration, clock_rate: u64) -> u32 {
    (elapsed.as_micros() * u128::from(clock_rate) / 1_000_000) as u32
}

fn ntp_middle(ntp: u64) -> u32 {
    (ntp >> 16) as u32
}

/// 64-bit NTP timestamp for the current wall clock.
fn ntp_now() -> u64 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    ntp_from_unix(d.as_secs(), d.subsec_nanos())
}
