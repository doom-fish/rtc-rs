//! Outbound video: split an Annex-B access unit into NAL units, H.264
//! packetize, and build RTP packets with MID + TWCC extensions.

use crate::config::MediaConfig;
use crate::media::annexb::split_nals;
use crate::media::SeqCounter;
use crate::source::CapturedFrame;
use rtp::payload::h264::Packetizer;
use rtp::{Extension, RtpPacket, RtpPacketOwned};

/// Default RTP payload MTU, leaving room under a 1200-byte SCTP/UDP budget.
const MTU: usize = 1100;

/// Packetizes one outbound H.264 stream.
pub struct VideoSender {
    packetizer: Packetizer,
    ssrc: u32,
    payload_type: u8,
    mid: String,
    mid_id: u8,
    twcc_id: u8,
    seq: u16,
}

impl VideoSender {
    /// Build a sender for `cfg`, tagging packets with `ssrc` and `mid`.
    #[must_use]
    pub fn new(cfg: &MediaConfig, ssrc: u32, mid: String, mid_id: u8, twcc_id: u8) -> Self {
        Self {
            packetizer: Packetizer::new(MTU),
            ssrc,
            payload_type: cfg.payload_type,
            mid,
            mid_id,
            twcc_id,
            seq: 1,
        }
    }

    /// Our SSRC.
    #[must_use]
    pub fn ssrc(&self) -> u32 {
        self.ssrc
    }

    /// Packetize one captured frame (its `data` is Annex-B) into RTP packets.
    /// The marker bit is set on the final packet of the access unit.
    pub fn encode(&mut self, frame: &CapturedFrame, twcc: &mut SeqCounter) -> Vec<RtpPacketOwned> {
        let nals = split_nals(&frame.data);
        if nals.is_empty() {
            return Vec::new();
        }
        let payloads = self.packetizer.packetize_access_unit(&nals);
        let last = payloads.len().saturating_sub(1);
        let mut out = Vec::with_capacity(payloads.len());
        for (i, payload) in payloads.iter().enumerate() {
            let pkt = RtpPacket::builder()
                .payload_type(self.payload_type)
                .sequence_number(self.seq)
                .timestamp(frame.rtp_timestamp)
                .ssrc(self.ssrc)
                .marker(i == last)
                .extension(Extension::mid(self.mid.as_bytes()).with_id(self.mid_id))
                .extension(Extension::twcc(twcc.advance()).with_id(self.twcc_id))
                .payload(payload)
                .build();
            self.seq = self.seq.wrapping_add(1);
            out.push(pkt);
        }
        out
    }
}
