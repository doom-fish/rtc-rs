//! Media plane: Opus audio and H.264 video send/receive pipelines plus the
//! per-SSRC jitter buffer.

pub mod annexb;
pub mod audio_recv;
pub mod audio_send;
pub mod jitter;
pub mod video_recv;
pub mod video_send;

pub use audio_recv::{AudioReceiver, DecodedAudio};
pub use audio_send::{AudioSender, SAMPLES_PER_FRAME};
pub use jitter::JitterBuffer;
pub use video_recv::{VideoAccessUnit, VideoReceiver};
pub use video_send::VideoSender;

/// A monotonically increasing 16-bit counter for transport-wide congestion
/// control (TWCC) sequence numbers, shared across all media streams in a
/// BUNDLE.
#[derive(Debug, Default)]
pub struct SeqCounter {
    next: u16,
}

impl SeqCounter {
    /// Create a counter starting at zero.
    #[must_use]
    pub fn new() -> Self {
        Self { next: 0 }
    }

    /// Return the next sequence number and advance (wrapping).
    pub fn advance(&mut self) -> u16 {
        let v = self.next;
        self.next = self.next.wrapping_add(1);
        v
    }

    /// The value that will be returned by the next call to [`Self::advance`].
    #[must_use]
    pub fn peek(&self) -> u16 {
        self.next
    }
}
