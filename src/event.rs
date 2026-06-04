//! Events delivered to the application over the [`crate::PeerConnection`]
//! event channel.

use crate::state::{DtlsState, IceConnectionState};
use crate::stats::Stats;

/// The kind of a media track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    /// Opus audio.
    Audio,
    /// H.264 video.
    Video,
}

/// Asynchronous events produced by a [`crate::PeerConnection`].
///
/// The worker thread pushes these onto a bounded channel; the application
/// drains them via [`crate::PeerConnection::poll_event`]. If the application falls
/// behind, the worker drops the oldest media events rather than blocking the
/// pipeline (see the crate docs).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Event {
    /// The ICE connection state changed.
    IceConnectionStateChange(IceConnectionState),
    /// The DTLS transport state changed.
    DtlsStateChange(DtlsState),
    /// A freshly gathered local ICE candidate (trickle ICE). The string is the
    /// `a=candidate:` SDP line value.
    LocalCandidate(String),
    /// The full DTLS-SRTP transport is established and media can flow.
    Connected,
    /// A new inbound media track was discovered (via its first RTP packet /
    /// MID extension).
    TrackAdded(TrackKind, u32),
    /// A decoded inbound audio frame: `(ssrc, pcm_samples, sample_rate,
    /// channels)`. PCM is interleaved `i16`.
    AudioFrame(u32, Vec<i16>, u32, u8),
    /// A decoded inbound video frame: `(ssrc, cv_pixel_buffer_ptr, width,
    /// height, rtp_timestamp)`. The pointer is a `CVPixelBufferRef` when the
    /// native decoder is active, or `0` when running without `apple-media`
    /// (the raw access unit is delivered through [`Event::VideoAccessUnit`]).
    VideoFrame(u32, usize, u32, u32, u32),
    /// A reassembled H.264 access unit prior to (or instead of) hardware
    /// decode: `(ssrc, annex_b_bytes, rtp_timestamp)`. Always emitted so a
    /// caller can decode it themselves.
    VideoAccessUnit(u32, Vec<u8>, u32),
    /// Periodic statistics snapshot.
    Stats(Stats),
    /// The connection was torn down; the string explains why.
    Disconnected(String),
    /// A non-fatal error occurred in the pipeline.
    Error(String),
}
