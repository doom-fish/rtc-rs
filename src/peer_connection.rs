//! The synchronous, PeerConnection-equivalent public API.

use crate::config::{ResolvedIceServer, RtcConfig};
use crate::error::{Result, RtcError};
use crate::event::{Event, EventQueue};
use crate::sdp_glue::{
    apply_remote, build_answer, build_offer, dtls_role_for_answer, dtls_role_for_offer,
    setup_for_role, LocalSdpIdentity, Negotiated, OfferSsrcs, RemoteParams,
};
use crate::source::{AudioSource, VideoSource};
use crate::state::{ConnectionState, DtlsRole, SignalingState};
use crate::transport::dtls_glue::new_connection;
use crate::util::{random_ice_string, random_ssrcs, random_u64, ICE_PWD_LEN, ICE_UFRAG_LEN};
use crate::worker::{self, Command, WorkerInit};
use sdp::{Answer, Offer};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;
use zeroize::Zeroizing;

const MAX_PENDING_CANDIDATES: usize = 100;

/// A remote session description applied via
/// [`PeerConnection::set_remote_description`].
#[derive(Debug, Clone)]
pub enum RemoteDescription {
    /// A remote offer (we will answer).
    Offer(Offer),
    /// A remote answer to our offer.
    Answer(Answer),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Unknown,
    Offerer,
    Answerer,
}

struct WorkerHandle {
    join: JoinHandle<()>,
    cmd: Sender<Command>,
}

/// A single peer-to-peer connection: SDP negotiation, ICE, DTLS-SRTP, and Opus
/// audio + H.264 video, all driven by one background thread.
///
/// The handle itself is a thin, `Send` facade; all transport and codec state
/// lives on the worker thread.
pub struct PeerConnection {
    config: RtcConfig,
    ice_ufrag: String,
    ice_pwd: Zeroizing<String>,
    role: Role,
    signaling: SignalingState,
    local_audio_ssrc: Option<u32>,
    local_video_ssrc: Option<u32>,
    rtcp_ssrc: u32,
    audio_source: Option<Arc<dyn AudioSource>>,
    video_source: Option<Arc<dyn VideoSource>>,
    // Set when our DTLS identity is committed (offerer: at create_offer;
    // answerer: at set_remote_description(Offer)).
    dtls: Option<dtls::Connection>,
    dtls_role: Option<DtlsRole>,
    identity: Option<LocalSdpIdentity>,
    // Answerer keeps the remote offer until create_answer.
    pending_offer: Option<Offer>,
    pending_remote: Option<RemoteParams>,
    pending_candidates: Vec<ice::Candidate>,
    pending_end_of_candidates: bool,
    events: Arc<EventQueue>,
    state: Arc<Mutex<ConnectionState>>,
    worker: Option<WorkerHandle>,
}

impl PeerConnection {
    /// Create a new connection with the given configuration. The worker thread
    /// is not started until negotiation produces both ICE credentials and a
    /// remote description.
    pub fn new(config: RtcConfig) -> Result<Self> {
        let mut turn_servers = 0;
        for server in &config.ice_servers {
            if matches!(server.resolve()?, ResolvedIceServer::Turn(_)) {
                turn_servers += 1;
            }
        }
        if config.relay_only && turn_servers == 0 {
            return Err(RtcError::Config(
                "relay_only needs at least one TURN server".into(),
            ));
        }
        let ice_ufrag = random_ice_string(ICE_UFRAG_LEN)?.to_string();
        let ice_pwd = random_ice_string(ICE_PWD_LEN)?;
        let ssrcs = random_ssrcs(3)?;
        let local_audio_ssrc = config.audio.as_ref().map(|_| ssrcs[0]);
        let local_video_ssrc = config.video.as_ref().map(|_| ssrcs[1]);
        Ok(Self {
            rtcp_ssrc: local_audio_ssrc.or(local_video_ssrc).unwrap_or(ssrcs[2]),
            config,
            ice_ufrag,
            ice_pwd,
            role: Role::Unknown,
            signaling: SignalingState::Stable,
            local_audio_ssrc,
            local_video_ssrc,
            audio_source: None,
            video_source: None,
            dtls: None,
            dtls_role: None,
            identity: None,
            pending_offer: None,
            pending_remote: None,
            pending_candidates: Vec::new(),
            pending_end_of_candidates: false,
            events: Arc::new(EventQueue::default()),
            state: Arc::new(Mutex::new(ConnectionState::New)),
            worker: None,
        })
    }

    /// Current signaling state.
    #[must_use]
    pub fn signaling_state(&self) -> SignalingState {
        self.signaling
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn connection_state(&self) -> ConnectionState {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn dtls_role(&self) -> Option<DtlsRole> {
        if self.role == Role::Offerer && self.worker.is_none() {
            None
        } else {
            self.dtls_role
        }
    }

    /// Attach an outbound audio source. Call before negotiation, or any time —
    /// once the worker is running the source is hot-swapped in.
    pub fn add_audio_track(&mut self, source: Arc<dyn AudioSource>) -> Result<()> {
        if self.config.audio.is_none() {
            return Err(RtcError::InvalidState("audio is not configured"));
        }
        match &self.worker {
            Some(w) => w
                .cmd
                .send(Command::AddAudioTrack(source))
                .map_err(|_| RtcError::WorkerGone),
            None => {
                self.audio_source = Some(source);
                Ok(())
            }
        }
    }

    /// Attach an outbound video source.
    pub fn add_video_track(&mut self, source: Arc<dyn VideoSource>) -> Result<()> {
        if self.config.video.is_none() {
            return Err(RtcError::InvalidState("video is not configured"));
        }
        match &self.worker {
            Some(w) => w
                .cmd
                .send(Command::AddVideoTrack(source))
                .map_err(|_| RtcError::WorkerGone),
            None => {
                self.video_source = Some(source);
                Ok(())
            }
        }
    }

    /// Create a local offer. Commits our DTLS identity as the **server**
    /// (the answerer will choose `setup:active`).
    pub fn create_offer(&mut self) -> Result<Offer> {
        if self.worker.is_some() {
            return Err(RtcError::InvalidState("renegotiation is not supported"));
        }
        if self.role == Role::Answerer {
            return Err(RtcError::InvalidState(
                "cannot create an offer after receiving one",
            ));
        }
        let dtls = new_connection(DtlsRole::Server, None)?;
        let identity = LocalSdpIdentity {
            ice_ufrag: self.ice_ufrag.clone(),
            ice_pwd: self.ice_pwd.clone(),
            fingerprint_sha256: dtls.local_fingerprint(),
        };
        let ssrcs = OfferSsrcs {
            audio: self.local_audio_ssrc,
            video: self.local_video_ssrc,
        };
        let offer = build_offer(&self.config, &identity, ssrcs, random_u64()?);
        self.dtls = Some(dtls);
        self.identity = Some(identity);
        self.role = Role::Offerer;
        self.signaling = SignalingState::HaveLocalOffer;
        Ok(offer)
    }

    /// Create an answer for a previously applied remote offer and start the
    /// worker as the ICE-**controlled** side. The answer's `a=setup` is the
    /// DTLS role chosen when the offer was applied.
    pub fn create_answer(&mut self) -> Result<Answer> {
        if self.role != Role::Answerer {
            return Err(RtcError::InvalidState(
                "create_answer requires a remote offer first",
            ));
        }
        if self.worker.is_some() {
            return Err(RtcError::InvalidState(
                "the answer has already been created",
            ));
        }
        let offer = self
            .pending_offer
            .as_ref()
            .ok_or(RtcError::InvalidState("no pending remote offer"))?;
        let identity = self
            .identity
            .as_ref()
            .ok_or(RtcError::InvalidState("no local identity"))?;
        let role = self
            .dtls_role
            .ok_or(RtcError::InvalidState("no DTLS role"))?;
        let answer = build_answer(offer, &self.config, identity, setup_for_role(role))?;
        let negotiated = Negotiated::from_answer(
            answer.sdp(),
            true,
            self.config.audio.as_ref(),
            self.config.video.as_ref(),
        );
        let remote = self
            .pending_remote
            .take()
            .ok_or(RtcError::InvalidState("no remote params"))?;
        let dtls = self
            .dtls
            .take()
            .ok_or(RtcError::InvalidState("no dtls connection"))?;
        self.start_worker(false, remote, negotiated, dtls)?;
        self.pending_offer = None;
        self.signaling = SignalingState::Stable;
        Ok(answer)
    }

    /// Apply a remote description. An [`RemoteDescription::Offer`] makes us the
    /// answerer (then call [`Self::create_answer`]); an
    /// [`RemoteDescription::Answer`] starts our worker as the offerer.
    pub fn set_remote_description(&mut self, desc: RemoteDescription) -> Result<()> {
        if self.worker.is_some() {
            return Err(RtcError::InvalidState("renegotiation is not supported"));
        }
        match desc {
            RemoteDescription::Offer(offer) => {
                if self.role == Role::Offerer {
                    return Err(RtcError::InvalidState("already created a local offer"));
                }
                let remote = apply_remote(offer.sdp())?;
                let role = dtls_role_for_offer(remote.setup, self.config.answer_dtls_role)?;
                let dtls = new_connection(role, Some(remote.fingerprint_sha256))?;
                self.identity = Some(LocalSdpIdentity {
                    ice_ufrag: self.ice_ufrag.clone(),
                    ice_pwd: self.ice_pwd.clone(),
                    fingerprint_sha256: dtls.local_fingerprint(),
                });
                self.buffer_inline_candidates(&remote);
                self.dtls = Some(dtls);
                self.dtls_role = Some(role);
                self.pending_offer = Some(offer);
                self.pending_remote = Some(remote);
                self.role = Role::Answerer;
                self.signaling = SignalingState::HaveRemoteOffer;
                Ok(())
            }
            RemoteDescription::Answer(answer) => {
                if self.role != Role::Offerer {
                    return Err(RtcError::InvalidState("no local offer to answer"));
                }
                let remote = apply_remote(answer.sdp())?;
                let role = dtls_role_for_answer(remote.setup)?;
                let negotiated = Negotiated::from_answer(
                    answer.sdp(),
                    false,
                    self.config.audio.as_ref(),
                    self.config.video.as_ref(),
                );
                let mut dtls = self
                    .dtls
                    .take()
                    .ok_or(RtcError::InvalidState("no dtls connection"))?;
                dtls.set_expected_peer_fingerprint(remote.fingerprint_sha256)?;
                self.buffer_inline_candidates(&remote);
                self.dtls_role = Some(role);
                self.start_worker(true, remote, negotiated, dtls)?;
                self.signaling = SignalingState::Stable;
                Ok(())
            }
        }
    }

    fn buffer_inline_candidates(&mut self, remote: &RemoteParams) {
        for line in &remote.candidates {
            match ice::sdp::parse(line, &self.config.mdns) {
                Ok(candidate) if self.pending_candidates.len() < MAX_PENDING_CANDIDATES => {
                    self.pending_candidates.push(candidate);
                }
                Ok(_) => self.events.push(Event::Error(RtcError::Ice(
                    "too many remote candidates before the connection started".into(),
                ))),
                Err(error) => self.events.push(Event::Error(error.into())),
            }
        }
    }

    /// Add a remote ICE candidate (trickle), as an `a=candidate:` line value.
    pub fn add_remote_candidate(&mut self, line: impl Into<String>) -> Result<()> {
        let candidate = ice::sdp::parse(&line.into(), &self.config.mdns)?;
        if let Some(w) = &self.worker {
            w.cmd
                .send(Command::AddRemoteCandidate(candidate))
                .map_err(|_| RtcError::WorkerGone)
        } else if self.pending_candidates.len() >= MAX_PENDING_CANDIDATES {
            Err(RtcError::Ice(
                "too many remote candidates before the connection started".into(),
            ))
        } else {
            self.pending_candidates.push(candidate);
            Ok(())
        }
    }

    /// Add a remote ICE candidate without requiring `&mut self`.
    ///
    /// Unlike [`add_remote_candidate`](Self::add_remote_candidate), this never
    /// buffers: it requires the worker to be running and is intended for the
    /// trickle exchange loop where candidates only appear after both peers have
    /// started gathering. Returns [`RtcError::WorkerGone`] if no worker exists.
    pub fn add_remote_candidate_immediate(&self, line: impl Into<String>) -> Result<()> {
        let w = self.worker.as_ref().ok_or(RtcError::WorkerGone)?;
        let candidate = ice::sdp::parse(&line.into(), &self.config.mdns)?;
        w.cmd
            .send(Command::AddRemoteCandidate(candidate))
            .map_err(|_| RtcError::WorkerGone)
    }

    #[allow(missing_docs)]
    pub fn add_remote_end_of_candidates(&mut self) -> Result<()> {
        match &self.worker {
            Some(w) => w
                .cmd
                .send(Command::RemoteEndOfCandidates)
                .map_err(|_| RtcError::WorkerGone),
            None => {
                self.pending_end_of_candidates = true;
                Ok(())
            }
        }
    }

    /// Non-blocking event poll.
    #[must_use]
    pub fn poll_event(&self) -> Option<Event> {
        self.events.try_pop()
    }

    /// Block up to `timeout` for the next event.
    #[must_use]
    pub fn recv_event_timeout(&self, timeout: Duration) -> Option<Event> {
        self.events.pop_timeout(timeout)
    }

    /// Close the connection and join the worker thread.
    pub fn close(&mut self) {
        if let Some(w) = self.worker.take() {
            w.cmd.send(Command::Stop).ok();
            if w.join.join().is_err() {
                self.events.push(Event::Failed(RtcError::WorkerGone));
            }
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if *state != ConnectionState::Closed {
            *state = ConnectionState::Closed;
            drop(state);
            self.events
                .push(Event::ConnectionStateChange(ConnectionState::Closed));
        }
        self.signaling = SignalingState::Closed;
        self.events.close();
    }

    fn start_worker(
        &mut self,
        controlling: bool,
        mut remote: RemoteParams,
        negotiated: Negotiated,
        dtls: dtls::Connection,
    ) -> Result<()> {
        let dtls_role = self
            .dtls_role
            .ok_or(RtcError::InvalidState("no DTLS role"))?;
        remote.end_of_candidates |= self.pending_end_of_candidates;
        let init = WorkerInit {
            config: self.config.clone(),
            controlling,
            local_ufrag: self.ice_ufrag.clone(),
            local_pwd: self.ice_pwd.clone(),
            remote,
            remote_candidates: std::mem::take(&mut self.pending_candidates),
            negotiated,
            dtls,
            dtls_role,
            local_audio_ssrc: self.local_audio_ssrc,
            local_video_ssrc: self.local_video_ssrc,
            rtcp_ssrc: self.rtcp_ssrc,
            audio_source: self.audio_source.clone(),
            video_source: self.video_source.clone(),
            events: Arc::clone(&self.events),
            state: Arc::clone(&self.state),
        };
        let (cmd, commands) = channel::<Command>();
        let join = std::thread::Builder::new()
            .name("rtc-worker".into())
            .spawn(move || worker::run(init, commands))
            .map_err(RtcError::from)?;
        self.worker = Some(WorkerHandle { join, cmd });
        Ok(())
    }
}

impl Drop for PeerConnection {
    fn drop(&mut self) {
        self.close();
    }
}
