//! Error types for the orchestration crate.

use std::fmt;

/// Result alias used across the public API.
pub type Result<T> = core::result::Result<T, RtcError>;

/// Errors surfaced by [`crate::PeerConnection`] and its subsystems.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RtcError {
    /// SDP parsing or negotiation failed.
    Sdp(String),
    /// ICE transport setup or connectivity failed.
    Ice(String),
    /// DTLS handshake or key export failed.
    Dtls(String),
    /// SRTP/SRTCP session setup or (un)protect failed.
    Srtp(String),
    /// RTP/RTCP encode/decode failed.
    Rtp(String),
    /// Audio/video codec error.
    Codec(String),
    /// A required media line or attribute was missing/invalid.
    Negotiation(String),
    /// The peer certificate fingerprint did not match the SDP `a=fingerprint`.
    FingerprintMismatch,
    /// The operation is not valid in the current signaling/connection state.
    InvalidState(&'static str),
    /// The background worker thread is gone.
    WorkerGone,
    /// An I/O error occurred.
    Io(String),
    #[allow(missing_docs)]
    Config(String),
    #[allow(missing_docs)]
    Unsupported(&'static str),
    #[allow(missing_docs)]
    Timeout(&'static str),
    #[allow(missing_docs)]
    ConsentExpired,
    #[allow(missing_docs)]
    IceFailed,
    #[allow(missing_docs)]
    Rng,
}

impl fmt::Display for RtcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sdp(m) => write!(f, "sdp error: {m}"),
            Self::Ice(m) => write!(f, "ice error: {m}"),
            Self::Dtls(m) => write!(f, "dtls error: {m}"),
            Self::Srtp(m) => write!(f, "srtp error: {m}"),
            Self::Rtp(m) => write!(f, "rtp error: {m}"),
            Self::Codec(m) => write!(f, "codec error: {m}"),
            Self::Negotiation(m) => write!(f, "negotiation error: {m}"),
            Self::FingerprintMismatch => write!(f, "peer fingerprint mismatch"),
            Self::InvalidState(m) => write!(f, "invalid state: {m}"),
            Self::WorkerGone => write!(f, "worker thread terminated"),
            Self::Io(m) => write!(f, "io error: {m}"),
            Self::Config(m) => write!(f, "invalid configuration: {m}"),
            Self::Unsupported(m) => write!(f, "unsupported: {m}"),
            Self::Timeout(m) => write!(f, "timed out: {m}"),
            Self::ConsentExpired => write!(f, "ICE consent to send expired (RFC 7675)"),
            Self::IceFailed => write!(f, "ICE failed: no candidate pair works"),
            Self::Rng => write!(f, "the system random number generator failed"),
        }
    }
}

impl std::error::Error for RtcError {}

impl From<sdp::SdpError> for RtcError {
    fn from(e: sdp::SdpError) -> Self {
        Self::Sdp(e.to_string())
    }
}

impl From<ice::Error> for RtcError {
    fn from(e: ice::Error) -> Self {
        match e {
            ice::Error::ConsentExpired => Self::ConsentExpired,
            ice::Error::Failed => Self::IceFailed,
            ice::Error::Rng => Self::Rng,
            other => Self::Ice(other.to_string()),
        }
    }
}

impl From<dtls::Error> for RtcError {
    fn from(e: dtls::Error) -> Self {
        match e {
            dtls::Error::FingerprintMismatch => Self::FingerprintMismatch,
            dtls::Error::Timeout => Self::Timeout("DTLS handshake retransmissions exhausted"),
            other => Self::Dtls(other.to_string()),
        }
    }
}

impl From<srtp::SrtpError> for RtcError {
    fn from(e: srtp::SrtpError) -> Self {
        Self::Srtp(e.to_string())
    }
}

impl From<rtp::Error> for RtcError {
    fn from(e: rtp::Error) -> Self {
        Self::Rtp(e.to_string())
    }
}

impl From<opus::OpusError> for RtcError {
    fn from(e: opus::OpusError) -> Self {
        Self::Codec(e.to_string())
    }
}

impl From<std::io::Error> for RtcError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}
