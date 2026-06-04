//! The synchronous, PeerConnection-equivalent public API.

use crate::config::RtcConfig;
use crate::error::{Result, RtcError};
use crate::event::Event;
use crate::sdp_glue::{apply_remote, build_answer, build_offer, dtls_role_for_remote, LocalSdpIdentity};
use crate::source::{AudioSource, VideoSource};
use crate::state::SignalingState;
use crate::transport::dtls_glue::new_connection;
use crate::util::{random_ice_string, Rng};
use crate::worker::{self, Command, WorkerInit};
use sdp::{Answer, Offer};
use std::sync::mpsc::{sync_channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

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
    events: Receiver<Event>,
}

/// A single peer-to-peer connection: SDP negotiation, ICE, DTLS-SRTP, and Opus
/// audio + H.264 video, all driven by one background thread.
///
/// The handle itself is a thin, `Send` facade; all transport and codec state
/// lives on the worker thread.
pub struct PeerConnection {
    config: RtcConfig,
    rng: Rng,
    ice_ufrag: String,
    ice_pwd: String,
    role: Role,
    signaling: SignalingState,
    local_audio_ssrc: Option<u32>,
    local_video_ssrc: Option<u32>,
    audio_source: Option<Arc<dyn AudioSource>>,
    video_source: Option<Arc<dyn VideoSource>>,
    // Set when our DTLS identity is committed (offerer: at create_offer;
    // answerer: at set_remote_description(Offer)).
    dtls: Option<dtls::Connection>,
    identity: Option<LocalSdpIdentity>,
    // Answerer keeps the remote offer until create_answer.
    pending_offer: Option<Offer>,
    pending_remote: Option<crate::sdp_glue::RemoteParams>,
    pending_candidates: Vec<String>,
    worker: Option<WorkerHandle>,
}

impl PeerConnection {
    /// Create a new connection with the given configuration. The worker thread
    /// is not started until negotiation produces both ICE credentials and a
    /// remote description.
    #[must_use]
    pub fn new(config: RtcConfig) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SALT: AtomicU64 = AtomicU64::new(0xC0FF_EE15_600D);
        let salt = SALT.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
        let mut rng = Rng::new(salt);
        let ice_ufrag = random_ice_string(&mut rng, 8);
        let ice_pwd = random_ice_string(&mut rng, 24);
        let local_audio_ssrc = config.audio.as_ref().map(|_| rng.next_u32());
        let local_video_ssrc = config.video.as_ref().map(|_| rng.next_u32());
        Self {
            config,
            rng,
            ice_ufrag,
            ice_pwd,
            role: Role::Unknown,
            signaling: SignalingState::Stable,
            local_audio_ssrc,
            local_video_ssrc,
            audio_source: None,
            video_source: None,
            dtls: None,
            identity: None,
            pending_offer: None,
            pending_remote: None,
            pending_candidates: Vec::new(),
            worker: None,
        }
    }

    /// Current signaling state.
    #[must_use]
    pub fn signaling_state(&self) -> SignalingState {
        self.signaling
    }

    /// Attach an outbound audio source. Call before negotiation, or any time —
    /// once the worker is running the source is hot-swapped in.
    pub fn add_audio_track(&mut self, source: Arc<dyn AudioSource>) {
        if let Some(w) = &self.worker {
            let _ = w.cmd.send(Command::AddAudioTrack(source));
        } else {
            self.audio_source = Some(source);
        }
    }

    /// Attach an outbound video source.
    pub fn add_video_track(&mut self, source: Arc<dyn VideoSource>) {
        if let Some(w) = &self.worker {
            let _ = w.cmd.send(Command::AddVideoTrack(source));
        } else {
            self.video_source = Some(source);
        }
    }

    /// Create a local offer. Commits our DTLS identity as the **server**
    /// (the answerer will choose `setup:active`).
    pub fn create_offer(&mut self) -> Result<Offer> {
        if self.role == Role::Answerer {
            return Err(RtcError::InvalidState("cannot create an offer after receiving one"));
        }
        let dtls = new_connection(dtls::Role::Server)?;
        let identity = LocalSdpIdentity {
            ice_ufrag: self.ice_ufrag.clone(),
            ice_pwd: self.ice_pwd.clone(),
            fingerprint_sha256: dtls.local_fingerprint(),
        };
        let (offer, ssrcs) = build_offer(&self.config, &identity, &mut self.rng);
        self.local_audio_ssrc = ssrcs.audio.or(self.local_audio_ssrc);
        self.local_video_ssrc = ssrcs.video.or(self.local_video_ssrc);
        self.dtls = Some(dtls);
        self.identity = Some(identity);
        self.role = Role::Offerer;
        self.signaling = SignalingState::HaveLocalOffer;
        Ok(offer)
    }

    /// Create an answer for a previously applied remote offer. Starts the
    /// worker as the ICE-**controlled**, DTLS-**client** side.
    pub fn create_answer(&mut self) -> Result<Answer> {
        if self.role != Role::Answerer {
            return Err(RtcError::InvalidState("create_answer requires a remote offer first"));
        }
        let offer = self
            .pending_offer
            .clone()
            .ok_or(RtcError::InvalidState("no pending remote offer"))?;
        let identity = self
            .identity
            .clone()
            .ok_or(RtcError::InvalidState("no local identity"))?;
        let answer = build_answer(&offer, &self.config, &identity)?;
        let remote = self
            .pending_remote
            .clone()
            .ok_or(RtcError::InvalidState("no remote params"))?;
        let dtls = self.dtls.take().ok_or(RtcError::InvalidState("no dtls connection"))?;
        let is_client = matches!(dtls_role_for_remote(remote.setup), dtls::Role::Client);
        self.start_worker(false, is_client, remote, dtls)?;
        self.signaling = SignalingState::Stable;
        Ok(answer)
    }

    /// Apply a remote description. An [`RemoteDescription::Offer`] makes us the
    /// answerer (then call [`Self::create_answer`]); an
    /// [`RemoteDescription::Answer`] starts our worker as the offerer.
    pub fn set_remote_description(&mut self, desc: RemoteDescription) -> Result<()> {
        match desc {
            RemoteDescription::Offer(offer) => {
                if self.role == Role::Offerer {
                    return Err(RtcError::InvalidState("already created a local offer"));
                }
                let remote = apply_remote(offer.sdp())?;
                let dtls_role = dtls_role_for_remote(remote.setup);
                let dtls = new_connection(dtls_role)?;
                let identity = LocalSdpIdentity {
                    ice_ufrag: self.ice_ufrag.clone(),
                    ice_pwd: self.ice_pwd.clone(),
                    fingerprint_sha256: dtls.local_fingerprint(),
                };
                self.dtls = Some(dtls);
                self.identity = Some(identity);
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
                let dtls = self.dtls.take().ok_or(RtcError::InvalidState("no dtls connection"))?;
                self.start_worker(true, false, remote, dtls)?;
                self.signaling = SignalingState::Stable;
                Ok(())
            }
        }
    }

    /// Add a remote ICE candidate (trickle), as an `a=candidate:` line value.
    pub fn add_remote_candidate(&mut self, line: impl Into<String>) -> Result<()> {
        let line = line.into();
        if let Some(w) = &self.worker {
            w.cmd
                .send(Command::AddRemoteCandidate(line))
                .map_err(|_| RtcError::WorkerGone)?;
        } else {
            self.pending_candidates.push(line);
        }
        Ok(())
    }

    /// Add a remote ICE candidate without requiring `&mut self`.
    ///
    /// Unlike [`add_remote_candidate`](Self::add_remote_candidate), this never
    /// buffers: it requires the worker to be running and is intended for the
    /// trickle exchange loop where candidates only appear after both peers have
    /// started gathering. Returns [`RtcError::WorkerGone`] if no worker exists.
    pub fn add_remote_candidate_immediate(&self, line: impl Into<String>) -> Result<()> {
        let w = self.worker.as_ref().ok_or(RtcError::WorkerGone)?;
        w.cmd
            .send(Command::AddRemoteCandidate(line.into()))
            .map_err(|_| RtcError::WorkerGone)
    }

    /// Non-blocking event poll.
    #[must_use]
    pub fn poll_event(&self) -> Option<Event> {
        self.worker.as_ref().and_then(|w| w.events.try_recv().ok())
    }

    /// Block up to `timeout` for the next event.
    #[must_use]
    pub fn recv_event_timeout(&self, timeout: Duration) -> Option<Event> {
        self.worker.as_ref().and_then(|w| w.events.recv_timeout(timeout).ok())
    }

    /// Close the connection and join the worker thread.
    pub fn close(&mut self) {
        if let Some(w) = self.worker.take() {
            let _ = w.cmd.send(Command::Stop);
            let _ = w.join.join();
        }
    }

    fn start_worker(
        &mut self,
        controlling: bool,
        is_dtls_client: bool,
        remote: crate::sdp_glue::RemoteParams,
        dtls: dtls::Connection,
    ) -> Result<()> {
        let expected_fingerprint = remote.fingerprint_sha256;
        let init = WorkerInit {
            config: self.config.clone(),
            controlling,
            local_ufrag: self.ice_ufrag.clone(),
            local_pwd: self.ice_pwd.clone(),
            remote,
            dtls,
            expected_fingerprint,
            is_dtls_client,
            local_audio_ssrc: self.local_audio_ssrc,
            local_video_ssrc: self.local_video_ssrc,
            audio_cfg: self.config.audio.clone(),
            video_cfg: self.config.video.clone(),
            audio_source: self.audio_source.clone(),
            video_source: self.video_source.clone(),
        };

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Command>();
        let (evt_tx, evt_rx): (SyncSender<Event>, Receiver<Event>) = sync_channel(1024);
        let join = std::thread::Builder::new()
            .name("rtc-worker".into())
            .spawn(move || worker::run(init, cmd_rx, evt_tx))
            .map_err(RtcError::from)?;

        for line in self.pending_candidates.drain(..) {
            let _ = cmd_tx.send(Command::AddRemoteCandidate(line));
        }

        self.worker = Some(WorkerHandle { join, cmd: cmd_tx, events: evt_rx });
        Ok(())
    }
}

impl Drop for PeerConnection {
    fn drop(&mut self) {
        self.close();
    }
}
