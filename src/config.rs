//! Configuration: [`RtcConfig`], [`IceServer`], and [`MediaConfig`].

use crate::error::{Result, RtcError};
use crate::state::DtlsRole;
use ice::{Clock, MdnsPolicy, SystemClock};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

/// An ICE server (STUN or TURN) used for candidate gathering.
#[derive(Clone)]
pub struct IceServer {
    /// The `stun:`/`turn:` URL.
    url: String,
    /// TURN long-term-credential username, if any.
    username: Option<String>,
    /// TURN long-term-credential password, if any.
    credential: Option<Zeroizing<String>>,
}

impl fmt::Debug for IceServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IceServer")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("credential", &self.credential.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl IceServer {
    /// A STUN server, e.g. `IceServer::stun("stun:stun.l.google.com:19302")`.
    pub fn stun(url: impl Into<String>) -> Self {
        Self { url: url.into(), username: None, credential: None }
    }

    /// A TURN server with long-term credentials.
    pub fn turn(
        url: impl Into<String>,
        username: impl Into<String>,
        credential: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            username: Some(username.into()),
            credential: Some(Zeroizing::new(credential.into())),
        }
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub(crate) fn resolve(&self) -> Result<ResolvedIceServer> {
        let url = self.url.as_str();
        if url.starts_with("stuns:") {
            return Err(RtcError::Unsupported("STUN over TLS (stuns:)"));
        }
        if url.starts_with("stun:") {
            let uri: stun::StunUri = url
                .parse()
                .map_err(|e: stun::Error| RtcError::Config(format!("STUN URL {url:?}: {e}")))?;
            return Ok(ResolvedIceServer::Stun(uri.into()));
        }
        if url.starts_with("turn:") || url.starts_with("turns:") {
            let uri: turn::TurnUri = url
                .parse()
                .map_err(|e: turn::Error| RtcError::Config(format!("TURN URL {url:?}: {e}")))?;
            if uri.scheme != turn::Scheme::Udp {
                return Err(RtcError::Unsupported("TURN over TCP or TLS; only UDP TURN relays"));
            }
            let (Some(username), Some(credential)) = (&self.username, &self.credential) else {
                return Err(RtcError::Config(format!(
                    "TURN URL {url:?} needs a username and a credential"
                )));
            };
            let credential = turn::Credential::long_term(username.as_str(), "", credential.as_str());
            return Ok(ResolvedIceServer::Turn(ice::TurnServer { uri, credential }));
        }
        Err(RtcError::Config(format!("ICE server URL {url:?} is neither stun: nor turn:")))
    }
}

pub(crate) enum ResolvedIceServer {
    Stun(stun::Endpoint),
    Turn(ice::TurnServer),
}

/// Codec used for a media kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// Opus audio (the WebRTC-mandated audio codec).
    Opus,
    /// H.264 video, constrained-baseline profile.
    H264,
}

/// Per-media-kind codec configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaConfig {
    /// The codec to negotiate.
    pub codec: Codec,
    /// Dynamic RTP payload type to offer.
    pub payload_type: u8,
    /// RTP clock rate.
    pub clock_rate: u32,
    /// Channel count (audio).
    pub channels: u8,
    /// Target encoder bitrate in bits per second.
    pub bitrate: u32,
    /// `fmtp` line value (without the `a=fmtp:<pt> ` prefix).
    pub fmtp: String,
}

impl MediaConfig {
    /// Opus, 48 kHz mono, 32 kbps VBR, inband-FEC on, DTX off — the meeting
    /// default.
    #[must_use]
    pub fn opus_default() -> Self {
        Self {
            codec: Codec::Opus,
            payload_type: 111,
            clock_rate: 48_000,
            channels: 2,
            bitrate: 32_000,
            fmtp: "minptime=10;useinbandfec=1;usedtx=0".to_string(),
        }
    }

    /// H.264 constrained-baseline (`profile-level-id=42e01f`),
    /// packetization-mode=1 — the most broadly interoperable video profile.
    #[must_use]
    pub fn h264_constrained_baseline() -> Self {
        Self {
            codec: Codec::H264,
            payload_type: 96,
            clock_rate: 90_000,
            channels: 0,
            bitrate: 1_200_000,
            fmtp: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                .to_string(),
        }
    }

    /// The RTP encoding name as it appears in an `a=rtpmap` line.
    #[must_use]
    pub fn encoding_name(&self) -> &'static str {
        match self.codec {
            Codec::Opus => "opus",
            Codec::H264 => "H264",
        }
    }
}

#[allow(missing_docs)]
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// The full configuration for a [`crate::PeerConnection`].
#[derive(Clone)]
pub struct RtcConfig {
    /// ICE servers for candidate gathering.
    pub ice_servers: Vec<IceServer>,
    /// Audio codec configuration, if audio is enabled.
    pub audio: Option<MediaConfig>,
    /// Video codec configuration, if video is enabled.
    pub video: Option<MediaConfig>,
    /// Bind ICE to loopback only (`127.0.0.1`). Used by the in-process
    /// loopback test/example so two agents can reach each other.
    pub loopback_only: bool,
    #[allow(missing_docs)]
    pub relay_only: bool,
    #[allow(missing_docs)]
    pub answer_dtls_role: DtlsRole,
    #[allow(missing_docs)]
    pub handshake_timeout: Duration,
    #[allow(missing_docs)]
    pub mdns: MdnsPolicy,
    #[allow(missing_docs)]
    pub clock: Arc<dyn Clock>,
}

impl fmt::Debug for RtcConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RtcConfig")
            .field("ice_servers", &self.ice_servers)
            .field("audio", &self.audio)
            .field("video", &self.video)
            .field("loopback_only", &self.loopback_only)
            .field("relay_only", &self.relay_only)
            .field("answer_dtls_role", &self.answer_dtls_role)
            .field("handshake_timeout", &self.handshake_timeout)
            .field("mdns", &self.mdns)
            .finish_non_exhaustive()
    }
}

impl RtcConfig {
    /// Start building a configuration.
    #[must_use]
    pub fn builder() -> RtcConfigBuilder {
        RtcConfigBuilder::default()
    }
}

impl Default for RtcConfig {
    fn default() -> Self {
        Self {
            ice_servers: Vec::new(),
            audio: Some(MediaConfig::opus_default()),
            video: Some(MediaConfig::h264_constrained_baseline()),
            loopback_only: false,
            relay_only: false,
            answer_dtls_role: DtlsRole::default(),
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            mdns: MdnsPolicy::default(),
            clock: Arc::new(SystemClock),
        }
    }
}

/// Builder for [`RtcConfig`].
#[derive(Debug, Clone)]
pub struct RtcConfigBuilder {
    config: RtcConfig,
}

impl Default for RtcConfigBuilder {
    fn default() -> Self {
        Self {
            config: RtcConfig {
                audio: None,
                video: None,
                ..RtcConfig::default()
            },
        }
    }
}

impl RtcConfigBuilder {
    /// Add an ICE server.
    #[must_use]
    pub fn ice_server(mut self, server: IceServer) -> Self {
        self.config.ice_servers.push(server);
        self
    }

    /// Enable audio with the given codec config.
    #[must_use]
    pub fn audio(mut self, cfg: MediaConfig) -> Self {
        self.config.audio = Some(cfg);
        self
    }

    /// Enable video with the given codec config.
    #[must_use]
    pub fn video(mut self, cfg: MediaConfig) -> Self {
        self.config.video = Some(cfg);
        self
    }

    /// Restrict ICE to loopback (`127.0.0.1`).
    #[must_use]
    pub fn loopback_only(mut self, yes: bool) -> Self {
        self.config.loopback_only = yes;
        self
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn relay_only(mut self, yes: bool) -> Self {
        self.config.relay_only = yes;
        self
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn answer_dtls_role(mut self, role: DtlsRole) -> Self {
        self.config.answer_dtls_role = role;
        self
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn handshake_timeout(mut self, timeout: Duration) -> Self {
        self.config.handshake_timeout = timeout;
        self
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn mdns(mut self, policy: MdnsPolicy) -> Self {
        self.config.mdns = policy;
        self
    }

    #[allow(missing_docs)]
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.config.clock = clock;
        self
    }

    /// Finalize the configuration.
    #[must_use]
    pub fn build(self) -> RtcConfig {
        self.config
    }
}
