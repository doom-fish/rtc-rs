//! `rtc` — a synchronous, dependency-light WebRTC media engine for native
//! macOS meeting clients.
//!
//! This crate ties together eight sibling crates — `sdp`, `ice`, `dtls`,
//! `srtp`, `rtp`, `opus`, `stun`, and `turn` — behind a single
//! PeerConnection-equivalent API. It performs the full offer/answer exchange,
//! ICE connectivity establishment, the DTLS-SRTP handshake, and Opus audio +
//! H.264 video transport, all driven by one background `std::thread` with
//! `std::sync::mpsc` channels. There is no `async`, no executor, and no
//! crates.io dependency beyond `libc` (pulled in transitively).
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
//! The optional `apple-media` feature wires the sibling Apple-SDK bindings
//! (`avcapture`, `audiounit`, `videotoolbox`, …) as default capture/render
//! sources; without it the application supplies its own [`AudioSource`] /
//! [`VideoSource`].

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

pub use config::{Codec, IceServer, MediaConfig, RtcConfig, RtcConfigBuilder};
pub use error::{Result, RtcError};
pub use event::{Event, TrackKind};
pub use peer_connection::{PeerConnection, RemoteDescription};
pub use source::{AudioSource, CapturedFrame, SilenceAudioSource, VideoSource};
pub use state::{DtlsState, IceConnectionState, SignalingState};
pub use stats::Stats;

/// Re-exported SDP offer/answer types from the sibling `sdp` crate so callers
/// can pass them through their own signaling channel.
pub use sdp::{Answer, Offer};
