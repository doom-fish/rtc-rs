//! Pluggable media source traits.
//!
//! The orchestrator pulls raw media from caller-supplied sources. Default
//! implementations backed by the native macOS bindings are available behind
//! the `apple-media` feature ([`crate::media::DefaultAudioSource`] /
//! [`crate::media::DefaultVideoSource`]); without that feature the application
//! supplies its own.

use crate::error::Result;
use std::time::Duration;

/// A captured video frame handed to the encode loop.
///
/// When backed by `avcapture`, `pixel_buffer` is a retained `CVPixelBufferRef`
/// the encoder consumes directly. For synthetic / test sources, set
/// `pixel_buffer` to `0` and provide the raw encoded or Annex-B `data` instead;
/// the encode path will packetize `data` as-is.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// A `CVPixelBufferRef` (retained), or `0` for synthetic frames.
    pub pixel_buffer: usize,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp in 90 kHz RTP clock units.
    pub rtp_timestamp: u32,
    /// Whether this frame is (or should be encoded as) a keyframe / IDR.
    pub keyframe: bool,
    /// Pre-encoded Annex-B H.264 bytes for synthetic / passthrough sources.
    /// Ignored when `pixel_buffer != 0` and the native encoder is active.
    pub data: Vec<u8>,
}

/// A source of video frames, pulled by the outbound encode loop.
pub trait VideoSource: Send + Sync {
    /// Block up to `timeout` for the next frame. Return `None` on timeout.
    fn next_frame(&self, timeout: Duration) -> Option<CapturedFrame>;
}

/// A source of audio, pulled by the outbound encode loop.
///
/// Each call must fill `out` with exactly 20 ms of mono signed-16 PCM at
/// 48 kHz (960 samples).
pub trait AudioSource: Send + Sync {
    /// Fill `out` with 960 mono `i16` samples (20 ms @ 48 kHz).
    fn next_block(&self, out: &mut [i16; 960]) -> Result<()>;
}

/// A trivial [`AudioSource`] that yields silence — useful for tests and for
/// exercising the pipeline without a microphone.
#[derive(Debug, Default, Clone, Copy)]
pub struct SilenceAudioSource;

impl AudioSource for SilenceAudioSource {
    fn next_block(&self, out: &mut [i16; 960]) -> Result<()> {
        out.fill(0);
        Ok(())
    }
}
