//! Outbound audio: pull PCM from an [`AudioSource`](crate::AudioSource),
//! Opus-encode it, and build RTP packets carrying MID + TWCC header extensions.

use crate::config::MediaConfig;
use crate::error::Result;
use crate::media::SeqCounter;
use opus::{Application, Channels, Encoder, SampleRate};
use rtp::{Extension, RtpPacket, RtpPacketOwned};

/// Samples in one 20 ms frame at 48 kHz mono.
pub const SAMPLES_PER_FRAME: usize = 960;
/// RTP timestamp increment per 20 ms frame at the 48 kHz clock.
const TS_PER_FRAME: u32 = 960;

/// Encodes and RTP-packetizes one outbound Opus stream.
pub struct AudioSender {
    encoder: Encoder,
    ssrc: u32,
    payload_type: u8,
    mid: String,
    mid_id: u8,
    twcc_id: u8,
    seq: u16,
    timestamp: u32,
    scratch: Vec<u8>,
}

impl AudioSender {
    /// Build a sender for `cfg`, tagging packets with `ssrc` and `mid`.
    pub fn new(cfg: &MediaConfig, ssrc: u32, mid: String, mid_id: u8, twcc_id: u8) -> Result<Self> {
        let mut encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(cfg.bitrate as i32)?;
        encoder.set_vbr(true)?;
        encoder.set_inband_fec(true)?;
        encoder.set_dtx(false)?;
        Ok(Self {
            encoder,
            ssrc,
            payload_type: cfg.payload_type,
            mid,
            mid_id,
            twcc_id,
            seq: 1,
            timestamp: 0,
            scratch: vec![0u8; 4000],
        })
    }

    /// Our SSRC.
    #[must_use]
    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    /// Encode one 20 ms frame and produce an RTP packet, allocating a fresh
    /// transport-wide sequence number from `twcc`.
    pub fn encode(
        &mut self,
        pcm: &[i16; SAMPLES_PER_FRAME],
        twcc: &mut SeqCounter,
    ) -> Result<RtpPacketOwned> {
        let n = self.encoder.encode(pcm, &mut self.scratch)?;
        let pkt = RtpPacket::builder()
            .payload_type(self.payload_type)
            .sequence_number(self.seq)
            .timestamp(self.timestamp)
            .ssrc(self.ssrc)
            .marker(false)
            .extension(Extension::mid(self.mid.as_bytes()).with_id(self.mid_id))
            .extension(Extension::twcc(twcc.advance()).with_id(self.twcc_id))
            .payload(&self.scratch[..n])
            .build();
        self.seq = self.seq.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(TS_PER_FRAME);
        Ok(pkt)
    }
}
