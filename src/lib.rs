//! `rtc` — a synchronous, dependency-light WebRTC media engine for native
//! macOS meeting clients.
//!
//! This crate ties together eight sibling crates — `sdp`, `ice`, `dtls`,
//! `srtp`, `rtp`, `opus`, `stun`, and `turn` — behind a single
//! PeerConnection-equivalent API. It performs the full offer/answer exchange,
//! ICE connectivity establishment, the DTLS-SRTP handshake, and Opus audio +
//! H.264 video transport, all driven by one background `std::thread`. There is
//! no `async`, no executor, and no crates.io dependency beyond `zeroize` and
//! `libc` (the latter pulled in transitively).
//!
//! ```text
//!   application
//!        │  create_offer / set_remote_description / add_*_track / events
//!        ▼
//!   PeerConnection (Send handle)  ──Command──►  worker thread
//!        ▲                                          │
//!        └──────────────── Event ◄──────────────────┘
//!                                                    │
//!        ┌───────────────────────────────────────────────────────────┐
//!        │  ICE ⇄ DTLS ⇄ SRTP ⇄ RTP ⇄ {Opus, H.264}  (+ RTCP/TWCC/cc) │
//!        └───────────────────────────────────────────────────────────┘
//! ```
//!
//! The application supplies its own [`AudioSource`] (PCM) and
//! [`VideoSource`] (encoded H.264). The optional `apple-media` feature only
//! compiles the sibling Apple-SDK bindings; it provides no sources yet.

#![warn(missing_docs)]
#![warn(clippy::all)]

mod cc;
mod config;
mod error;
mod event;
mod peer_connection;
mod rtcp;
mod sdp_glue;
mod source;
mod state;
mod stats;
mod transport;
mod util;
mod worker;

pub mod media;

pub use config::{
    Codec, IceServer, MediaConfig, RtcConfig, RtcConfigBuilder, DEFAULT_HANDSHAKE_TIMEOUT,
};
pub use error::{Result, RtcError};
pub use event::{Event, TrackKind};
pub use peer_connection::{PeerConnection, RemoteDescription};
pub use source::{AudioSource, CapturedFrame, SilenceAudioSource, VideoSource};
pub use state::{ConnectionState, DtlsRole, DtlsState, IceConnectionState, SignalingState};
pub use stats::Stats;

pub use ice::{Clock, MdnsPolicy, SystemClock};

/// Re-exported SDP offer/answer types from the sibling `sdp` crate so callers
/// can pass them through their own signaling channel.
pub use sdp::{Answer, Offer};

pub use sdp::writer::write_sdp;
