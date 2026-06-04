//! Configuration: [`RtcConfig`], [`IceServer`], and [`MediaConfig`].

/// An ICE server (STUN or TURN) used for candidate gathering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceServer {
    /// The `stun:`/`turn:` URL.
    pub url: String,
    /// TURN long-term-credential username, if any.
    pub username: Option<String>,
    /// TURN long-term-credential password, if any.
    pub credential: Option<String>,
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
            credential: Some(credential.into()),
        }
    }

    /// `true` when this is a `turn:` URL with credentials.
    #[must_use]
    pub fn is_turn(&self) -> bool {
        self.url.starts_with("turn:") || self.url.starts_with("turns:")
    }

    /// The `host:port` portion of the URL, without the scheme.
    #[must_use]
    pub fn host_port(&self) -> &str {
        let s = self.url.as_str();
        let s = s.strip_prefix("stun:").unwrap_or(s);
        let s = s.strip_prefix("stuns:").unwrap_or(s);
        let s = s.strip_prefix("turn:").unwrap_or(s);
        let s = s.strip_prefix("turns:").unwrap_or(s);
        // Drop any `?transport=...` suffix.
        s.split('?').next().unwrap_or(s)
    }
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

/// The full configuration for a [`crate::PeerConnection`].
#[derive(Debug, Clone)]
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
        }
    }
}

/// Builder for [`RtcConfig`].
#[derive(Debug, Default, Clone)]
pub struct RtcConfigBuilder {
    ice_servers: Vec<IceServer>,
    audio: Option<MediaConfig>,
    video: Option<MediaConfig>,
    loopback_only: bool,
}

impl RtcConfigBuilder {
    /// Add an ICE server.
    #[must_use]
    pub fn ice_server(mut self, server: IceServer) -> Self {
        self.ice_servers.push(server);
        self
    }

    /// Enable audio with the given codec config.
    #[must_use]
    pub fn audio(mut self, cfg: MediaConfig) -> Self {
        self.audio = Some(cfg);
        self
    }

    /// Enable video with the given codec config.
    #[must_use]
    pub fn video(mut self, cfg: MediaConfig) -> Self {
        self.video = Some(cfg);
        self
    }

    /// Restrict ICE to loopback (`127.0.0.1`).
    #[must_use]
    pub fn loopback_only(mut self, yes: bool) -> Self {
        self.loopback_only = yes;
        self
    }

    /// Finalize the configuration.
    #[must_use]
    pub fn build(self) -> RtcConfig {
        RtcConfig {
            ice_servers: self.ice_servers,
            audio: self.audio,
            video: self.video,
            loopback_only: self.loopback_only,
        }
    }
}
