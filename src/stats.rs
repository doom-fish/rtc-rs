//! Connection-wide statistics emitted via [`crate::Event::Stats`].

/// A snapshot of transport / media statistics.
///
/// All rates are instantaneous estimates over the most recent reporting
/// interval (~1 s). Loss is a fraction in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[non_exhaustive]
pub struct Stats {
    /// Outbound audio bitrate, bits per second.
    pub audio_send_bitrate: u32,
    /// Outbound video bitrate, bits per second.
    pub video_send_bitrate: u32,
    /// Inbound audio bitrate, bits per second.
    pub audio_recv_bitrate: u32,
    /// Inbound video bitrate, bits per second.
    pub video_recv_bitrate: u32,
    /// Fraction of inbound packets lost.
    pub recv_loss: f32,
    /// Fraction of outbound packets the peer reported lost.
    pub send_loss: f32,
    /// Inter-arrival jitter estimate in RTP timestamp units.
    pub jitter: u32,
    /// Round-trip time in milliseconds, when known.
    pub rtt_ms: u32,
    /// Current congestion-controller target bitrate for video, bits/s.
    pub target_video_bitrate: u32,
    /// Remote estimated maximum bitrate (REMB), bits/s, when received.
    pub remb_bitrate: u32,
    #[allow(missing_docs)]
    pub target_audio_bitrate: u32,
    #[allow(missing_docs)]
    pub remote_streams: u32,
    #[allow(missing_docs)]
    pub dropped_unexpected_source: u64,
    #[allow(missing_docs)]
    pub dropped_inbound: u64,
    #[allow(missing_docs)]
    pub srtp_rejected: u64,
    #[allow(missing_docs)]
    pub nacks_received: u64,
    #[allow(missing_docs)]
    pub packets_retransmitted: u64,
    #[allow(missing_docs)]
    pub keyframe_requests: u64,
    #[allow(missing_docs)]
    pub send_errors: u64,
    #[allow(missing_docs)]
    pub dropped_events: u64,
}
