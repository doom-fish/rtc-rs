//! Inbound audio: jitter-buffer SRTP-decrypted RTP, then Opus-decode to PCM.

use crate::config::MediaConfig;
use crate::error::Result;
use crate::media::jitter::JitterBuffer;
use opus::{Channels, Decoder, SampleRate};
use rtp::{Packet, RtpPacket};

/// A decoded inbound audio frame.
pub struct DecodedAudio {
    /// Source SSRC.
    pub ssrc: u32,
    /// Interleaved PCM samples.
    pub pcm: Vec<i16>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u8,
}

/// Reassembles and decodes one inbound Opus stream.
pub struct AudioReceiver {
    decoder: Decoder,
    jitter: JitterBuffer,
    ssrc: u32,
    scratch: Vec<i16>,
}

impl AudioReceiver {
    /// Create a receiver for `ssrc` using `cfg` (50 ms jitter target).
    pub fn new(_cfg: &MediaConfig, ssrc: u32) -> Result<Self> {
        let decoder = Decoder::new(SampleRate::Hz48000, Channels::Mono)?;
        Ok(Self {
            decoder,
            jitter: JitterBuffer::with_target_ms(50, 20),
            ssrc,
            // 120 ms ceiling at 48 kHz mono.
            scratch: vec![0i16; 5760],
        })
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

    /// Decode all packets the jitter buffer is ready to release.
    pub fn drain(&mut self) -> Vec<DecodedAudio> {
        let mut out = Vec::new();
        while let Some(wire) = self.jitter.pop() {
            if let Some(frame) = self.decode_one(&wire) {
                out.push(frame);
            }
        }
        out
    }

    fn decode_one(&mut self, wire: &[u8]) -> Option<DecodedAudio> {
        let pkt = RtpPacket::parse(wire).ok()?;
        let n = self.decoder.decode(pkt.payload(), &mut self.scratch).ok()?;
        Some(DecodedAudio {
            ssrc: self.ssrc,
            pcm: self.scratch[..n].to_vec(),
            sample_rate: 48_000,
            channels: 1,
        })
    }
}
