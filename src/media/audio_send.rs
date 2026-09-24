//! Outbound audio: pull PCM from an [`AudioSource`](crate::AudioSource),
//! Opus-encode it, and build RTP packets carrying MID + TWCC header extensions.

use crate::config::MediaConfig;
use crate::error::{Result, RtcError};
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
    mid_id: Option<u8>,
    twcc_id: Option<u8>,
    seq: u16,
    timestamp: u32,
    scratch: Vec<u8>,
}

impl AudioSender {
    /// Build a sender for `cfg`, tagging packets with `ssrc`, and with `mid`
    /// and a transport-wide sequence number under the negotiated extension
    /// ids when those are present.
    pub fn new(
        cfg: &MediaConfig,
        ssrc: u32,
        mid: String,
        mid_id: Option<u8>,
        twcc_id: Option<u8>,
    ) -> Result<Self> {
        let mut encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(bitrate_i32(cfg.bitrate)?)?;
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

    #[allow(missing_docs)]
    pub fn set_bitrate(&mut self, bits_per_second: u32) -> Result<()> {
        self.encoder.set_bitrate(bitrate_i32(bits_per_second)?)?;
        Ok(())
    }

    /// Encode one 20 ms frame and produce an RTP packet, allocating a fresh
    /// transport-wide sequence number from `twcc` when TWCC is negotiated.
    pub fn encode(
        &mut self,
        pcm: &[i16; SAMPLES_PER_FRAME],
        twcc: &mut SeqCounter,
    ) -> Result<RtpPacketOwned> {
        let n = self.encoder.encode(pcm, &mut self.scratch)?;
        let mut builder = RtpPacket::builder()
            .payload_type(self.payload_type)
            .sequence_number(self.seq)
            .timestamp(self.timestamp)
            .ssrc(self.ssrc)
            .marker(false);
        if let Some(id) = self.mid_id {
            builder = builder.extension(Extension::mid(self.mid.as_bytes()).with_id(id));
        }
        if let Some(id) = self.twcc_id {
            builder = builder.extension(Extension::twcc(twcc.advance()).with_id(id));
        }
        let pkt = builder.payload(&self.scratch[..n]).build()?;
        self.seq = self.seq.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(TS_PER_FRAME);
        Ok(pkt)
    }
}

fn bitrate_i32(bits_per_second: u32) -> Result<i32> {
    i32::try_from(bits_per_second)
        .map_err(|_| RtcError::Config(format!("audio bitrate {bits_per_second} is out of range")))
}
