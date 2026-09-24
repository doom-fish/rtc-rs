//! Inbound video: jitter-buffer SRTP-decrypted RTP, H.264-depacketize into NAL
//! units, and reassemble Annex-B access units on the RTP marker boundary.

use crate::media::annexb::join_nals;
use crate::media::jitter::JitterBuffer;
use rtp::payload::h264::Depacketizer;
use rtp::{Packet, RtpPacket};

/// A reassembled inbound H.264 access unit.
pub struct VideoAccessUnit {
    /// Source SSRC.
    pub ssrc: u32,
    /// Annex-B bytes (start-code prefixed NAL units).
    pub annex_b: Vec<u8>,
    /// RTP timestamp (90 kHz).
    pub rtp_timestamp: u32,
}

#[allow(missing_docs)]
pub const MAX_ACCESS_UNIT_BYTES: usize = 4 * 1024 * 1024;

/// Reassembles one inbound H.264 stream.
pub struct VideoReceiver {
    depacketizer: Depacketizer,
    jitter: JitterBuffer,
    ssrc: u32,
    pending: Vec<Vec<u8>>,
    pending_bytes: usize,
    oversized: bool,
    current_ts: Option<u32>,
}

impl VideoReceiver {
    /// Create a receiver for `ssrc` (100 ms jitter target).
    #[must_use]
    pub fn new(ssrc: u32) -> Self {
        Self {
            depacketizer: Depacketizer::with_max_nal_size(MAX_ACCESS_UNIT_BYTES),
            jitter: JitterBuffer::with_target_ms(100, 33),
            ssrc,
            pending: Vec::new(),
            pending_bytes: 0,
            oversized: false,
            current_ts: None,
        }
    }

    /// Our SSRC.
    #[must_use]
    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    /// Push decrypted RTP wire bytes into the jitter buffer.
    pub fn push(&mut self, seq: u16, wire: Vec<u8>) {
        self.jitter.push(seq, wire);
    }

    /// Reassemble any access units the jitter buffer is ready to release.
    pub fn drain(&mut self) -> Vec<VideoAccessUnit> {
        let mut out = Vec::new();
        while let Some(wire) = self.jitter.pop() {
            self.consume(&wire, &mut out);
        }
        out
    }

    /// Drain the jitter buffer completely, ignoring the target-depth gate, and
    /// emit any remaining access units. Use at end-of-stream so the final
    /// (still-buffered) frames are not lost.
    pub fn flush(&mut self) -> Vec<VideoAccessUnit> {
        let mut out = Vec::new();
        while let Some(wire) = self.jitter.flush_one() {
            self.consume(&wire, &mut out);
        }
        // A trailing AU with no marker bit (timestamp never advanced again)
        // still needs to be emitted.
        if let Some(ts) = self.current_ts.take() {
            self.emit_au(ts, &mut out);
        }
        out
    }

    fn consume(&mut self, wire: &[u8], out: &mut Vec<VideoAccessUnit>) {
        let Ok(pkt) = RtpPacket::parse(wire) else {
            return;
        };
        let ts = pkt.header().timestamp();
        let seq = pkt.header().sequence_number();
        let marker = pkt.header().marker();

        // A timestamp change without a marker bit also ends the previous AU.
        if let Some(prev) = self.current_ts {
            if prev != ts {
                self.emit_au(prev, out);
            }
        }
        self.current_ts = Some(ts);

        if let Ok(Some(nal)) = self.depacketizer.push(seq, pkt.payload()) {
            self.push_nal(nal);
            while let Some(nal) = self.depacketizer.pop() {
                self.push_nal(nal);
            }
        }

        if marker {
            self.emit_au(ts, out);
            self.current_ts = None;
        }
    }

    fn push_nal(&mut self, nal: Vec<u8>) {
        if self.oversized {
            return;
        }
        if self.pending_bytes.saturating_add(nal.len()) > MAX_ACCESS_UNIT_BYTES {
            self.pending.clear();
            self.pending_bytes = 0;
            self.oversized = true;
            return;
        }
        self.pending_bytes += nal.len();
        self.pending.push(nal);
    }

    fn emit_au(&mut self, ts: u32, out: &mut Vec<VideoAccessUnit>) {
        self.oversized = false;
        self.pending_bytes = 0;
        if self.pending.is_empty() {
            return;
        }
        let nals = std::mem::take(&mut self.pending);
        out.push(VideoAccessUnit {
            ssrc: self.ssrc,
            annex_b: join_nals(&nals),
            rtp_timestamp: ts,
        });
    }
}
