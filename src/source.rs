//! Pluggable media source traits.
//!
//! The orchestrator pulls media from caller-supplied sources: raw PCM for
//! audio (encoded to Opus internally) and already-encoded H.264 access units
//! for video.

use crate::error::Result;
use std::time::Duration;

/// An encoded video frame handed to the send loop.
///
/// `data` is one H.264 access unit in Annex-B form; the send path packetizes
/// it as-is.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Presentation timestamp in 90 kHz RTP clock units.
    pub rtp_timestamp: u32,
    /// Whether this frame is a keyframe / IDR.
    pub keyframe: bool,
    /// The Annex-B H.264 access unit.
    pub data: Vec<u8>,
}

/// A source of video frames, pulled by the outbound send loop.
pub trait VideoSource: Send + Sync {
    /// Block up to `timeout` for the next frame. Return `None` on timeout.
    fn next_frame(&self, timeout: Duration) -> Option<CapturedFrame>;

    #[allow(missing_docs)]
    fn request_keyframe(&self) {}
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
