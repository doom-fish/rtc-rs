//! SRTP/SRTCP session lifecycle: derive keys from the DTLS exporter and build
//! the sender/receiver sessions (RFC 5764 §4.2).
//!
//! With BUNDLE + rtcp-mux a single transport carries several SSRCs (audio,
//! video, …). Each SSRC is its own SRTP and SRTCP cryptographic context — own
//! rollover counter, index and replay window — even though they share one
//! master key. A receive context is only kept once a packet for its SSRC has
//! authenticated, and at most [`MAX_REMOTE_SSRCS`] are kept, least recently
//! used first out. Send contexts exist only for our own SSRCs, one each.

use crate::error::{Result, RtcError};
use crate::state::DtlsRole;
use srtp::{Profile, SrtcpSession, SrtpKey, SrtpSession};
use std::collections::HashMap;

pub const MAX_REMOTE_SSRCS: usize = 32;

/// Map a negotiated DTLS SRTP profile to the `srtp` crate profile.
#[must_use]
pub fn map_profile(p: dtls::SrtpProfile) -> Profile {
    match p {
        dtls::SrtpProfile::AeadAes128Gcm => Profile::AeadAes128Gcm,
        dtls::SrtpProfile::AeadAes256Gcm => Profile::AeadAes256Gcm,
    }
}

struct Bounded<T> {
    entries: HashMap<u32, (T, u64)>,
    tick: u64,
}

impl<T> Bounded<T> {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            tick: 0,
        }
    }

    fn get_mut(&mut self, ssrc: u32) -> Option<&mut T> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(&ssrc).map(|(value, used)| {
            *used = tick;
            value
        })
    }

    fn insert(&mut self, ssrc: u32, value: T) -> Option<u32> {
        self.tick += 1;
        let evicted = if self.entries.len() >= MAX_REMOTE_SSRCS && !self.entries.contains_key(&ssrc)
        {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(&ssrc, _)| ssrc);
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
            oldest
        } else {
            None
        };
        self.entries.insert(ssrc, (value, self.tick));
        evicted
    }
}

/// The SRTP/SRTCP cryptographic state for a connection.
pub struct SrtpContext {
    send_key: SrtpKey,
    recv_key: SrtpKey,
    local_ssrcs: Vec<u32>,
    rtp_send: HashMap<u32, SrtpSession>,
    rtcp_send: HashMap<u32, SrtcpSession>,
    rtp_recv: Bounded<SrtpSession>,
    rtcp_recv: Bounded<SrtcpSession>,
}

impl SrtpContext {
    pub fn new(
        profile: Profile,
        send: (&[u8], &[u8]),
        recv: (&[u8], &[u8]),
        local_ssrcs: Vec<u32>,
    ) -> Result<Self> {
        Ok(Self {
            send_key: SrtpKey::from_master(send.0, send.1, profile)?,
            recv_key: SrtpKey::from_master(recv.0, recv.1, profile)?,
            local_ssrcs,
            rtp_send: HashMap::new(),
            rtcp_send: HashMap::new(),
            rtp_recv: Bounded::new(),
            rtcp_recv: Bounded::new(),
        })
    }

    fn check_local(&self, ssrc: u32) -> Result<()> {
        if self.local_ssrcs.contains(&ssrc) {
            Ok(())
        } else {
            Err(RtcError::Srtp(format!(
                "SSRC {ssrc:#010x} is not one of ours"
            )))
        }
    }

    pub fn protect_rtp(&mut self, pkt: &rtp::RtpPacketOwned, out: &mut [u8]) -> Result<usize> {
        use rtp::Packet;
        let ssrc = pkt.header().ssrc();
        self.check_local(ssrc)?;
        let key = &self.send_key;
        let session = self
            .rtp_send
            .entry(ssrc)
            .or_insert_with(|| SrtpSession::new_sender(key.clone()));
        Ok(session.protect(pkt, out)?)
    }

    pub fn protect_rtcp(&mut self, plain: &[u8], out: &mut [u8]) -> Result<usize> {
        let ssrc = rtcp_sender_ssrc(plain)?;
        self.check_local(ssrc)?;
        let key = &self.send_key;
        let session = self
            .rtcp_send
            .entry(ssrc)
            .or_insert_with(|| SrtcpSession::new_sender(key.clone()));
        Ok(session.protect(plain, out)?)
    }

    pub fn unprotect_rtp<'a>(
        &mut self,
        buf: &'a mut [u8],
    ) -> Result<(rtp::RtpPacket<'a>, Option<u32>)> {
        use rtp::Packet;
        let ssrc = rtp::RtpPacket::parse(buf)?.header().ssrc();
        if let Some(session) = self.rtp_recv.get_mut(ssrc) {
            return Ok((session.unprotect(buf)?, None));
        }
        let mut session = SrtpSession::new_receiver(self.recv_key.clone());
        let packet = session.unprotect(buf)?;
        let evicted = self.rtp_recv.insert(ssrc, session);
        Ok((packet, evicted))
    }

    pub fn unprotect_rtcp(&mut self, buf: &[u8]) -> Result<Vec<u8>> {
        let ssrc = rtcp_sender_ssrc(buf)?;
        if let Some(session) = self.rtcp_recv.get_mut(ssrc) {
            return Ok(session.unprotect(buf)?);
        }
        let mut session =
            SrtcpSession::new_receiver(self.recv_key.clone()).require_encryption(true);
        let plain = session.unprotect(buf)?;
        self.rtcp_recv.insert(ssrc, session);
        Ok(plain)
    }

    #[cfg(test)]
    pub fn remote_rtp_contexts(&self) -> usize {
        self.rtp_recv.entries.len()
    }

    #[cfg(test)]
    pub fn remote_rtcp_contexts(&self) -> usize {
        self.rtcp_recv.entries.len()
    }
}

fn rtcp_sender_ssrc(packet: &[u8]) -> Result<u32> {
    packet
        .get(4..8)
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map(u32::from_be_bytes)
        .ok_or_else(|| RtcError::Srtp("RTCP packet shorter than its header".into()))
}

/// Derive the SRTP key block from the DTLS exporter and build the context.
///
/// Per RFC 5764 §4.2 the exporter output is split as
/// `client_master_key | server_master_key | client_master_salt |
/// server_master_salt`. `role` selects which half we send with.
pub fn derive(
    conn: &dtls::Connection,
    role: DtlsRole,
    local_ssrcs: Vec<u32>,
) -> Result<SrtpContext> {
    let dtls_profile = conn
        .selected_srtp_profile()
        .ok_or_else(|| RtcError::Srtp("DTLS did not negotiate an SRTP profile".into()))?;
    let profile = map_profile(dtls_profile);

    let key_len = profile.master_key_len();
    let salt_len = profile.master_salt_len();
    let block_len = 2 * (key_len + salt_len);

    let block = conn
        .export_keying_material(b"EXTRACTOR-dtls_srtp", None, block_len)
        .map_err(RtcError::from)?;
    if block.len() != block_len {
        return Err(RtcError::Srtp(format!(
            "exporter returned {} bytes, expected {block_len}",
            block.len()
        )));
    }

    // Split per RFC 5764 §4.2.
    let (keys, salts) = block.split_at(2 * key_len);
    let client = (&keys[..key_len], &salts[..salt_len]);
    let server = (&keys[key_len..], &salts[salt_len..]);

    // The DTLS client sends with the client keys, the server with the server
    // keys (RFC 5764 §4.2).
    let (send, recv) = match role {
        DtlsRole::Client => (client, server),
        DtlsRole::Server => (server, client),
    };
    SrtpContext::new(profile, send, recv, local_ssrcs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtp::Packet;

    const KEY_A: [u8; 16] = [1; 16];
    const SALT_A: [u8; 12] = [2; 12];
    const KEY_B: [u8; 16] = [3; 16];
    const SALT_B: [u8; 12] = [4; 12];

    fn pair(remote_ssrcs: Vec<u32>) -> (SrtpContext, SrtpContext) {
        let local = SrtpContext::new(
            Profile::AeadAes128Gcm,
            (&KEY_A, &SALT_A),
            (&KEY_B, &SALT_B),
            vec![0x1111],
        )
        .unwrap();
        let remote = SrtpContext::new(
            Profile::AeadAes128Gcm,
            (&KEY_B, &SALT_B),
            (&KEY_A, &SALT_A),
            remote_ssrcs,
        )
        .unwrap();
        (local, remote)
    }

    fn rtp_packet(ssrc: u32, seq: u16) -> rtp::RtpPacketOwned {
        rtp::RtpPacket::builder()
            .payload_type(111)
            .sequence_number(seq)
            .timestamp(u32::from(seq) * 960)
            .ssrc(ssrc)
            .payload([0xAB; 40])
            .build()
            .unwrap()
    }

    fn rtcp_packet(ssrc: u32) -> Vec<u8> {
        rtp::rtcp::RtcpPacket::receiver_report(ssrc)
            .build()
            .encode()
            .unwrap()
    }

    fn protect(ctx: &mut SrtpContext, pkt: &rtp::RtpPacketOwned) -> Vec<u8> {
        let mut out = vec![0u8; pkt.as_bytes().len() + 64];
        let n = ctx.protect_rtp(pkt, &mut out).unwrap();
        out.truncate(n);
        out
    }

    fn protect_rtcp(ctx: &mut SrtpContext, plain: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; plain.len() + 64];
        let n = ctx.protect_rtcp(plain, &mut out).unwrap();
        out.truncate(n);
        out
    }

    #[test]
    fn spoofed_ssrc_flood_creates_no_contexts() {
        const FLOOD: u32 = 4_096;
        let (mut local, _) = pair(Vec::new());
        let mut attacker = SrtpContext::new(
            Profile::AeadAes128Gcm,
            (&[9; 16], &[9; 12]),
            (&[8; 16], &[8; 12]),
            (0..FLOOD).collect(),
        )
        .unwrap();
        for ssrc in 0..FLOOD {
            let mut forged = protect(&mut attacker, &rtp_packet(ssrc, 1));
            assert!(local.unprotect_rtp(&mut forged).is_err());
            let mut garbage = rtp_packet(ssrc, 2).into_bytes();
            garbage.extend_from_slice(&[0u8; 16]);
            assert!(local.unprotect_rtp(&mut garbage).is_err());
            let forged_rtcp = protect_rtcp(&mut attacker, &rtcp_packet(ssrc));
            assert!(local.unprotect_rtcp(&forged_rtcp).is_err());
        }
        assert_eq!(local.remote_rtp_contexts(), 0);
        assert_eq!(local.remote_rtcp_contexts(), 0);
    }

    #[test]
    fn authenticated_ssrcs_stay_bounded_and_evict_the_least_recent() {
        let ssrcs: Vec<u32> = (1..=100).collect();
        let (mut local, mut remote) = pair(ssrcs.clone());
        let mut evicted = Vec::new();
        for &ssrc in &ssrcs {
            let mut wire = protect(&mut remote, &rtp_packet(ssrc, 7));
            let (packet, gone) = local.unprotect_rtp(&mut wire).unwrap();
            assert_eq!(packet.header().ssrc(), ssrc);
            evicted.extend(gone);
            assert!(local.remote_rtp_contexts() <= MAX_REMOTE_SSRCS);
        }
        assert_eq!(local.remote_rtp_contexts(), MAX_REMOTE_SSRCS);
        assert_eq!(evicted, (1..=68).collect::<Vec<u32>>());

        let mut wire = protect(&mut remote, &rtp_packet(100, 8));
        assert!(local.unprotect_rtp(&mut wire).unwrap().1.is_none());
    }

    #[test]
    fn replayed_packets_are_rejected_per_ssrc() {
        let (mut local, mut remote) = pair(vec![7]);
        let wire = protect(&mut remote, &rtp_packet(7, 1));
        assert!(local.unprotect_rtp(&mut wire.clone()).is_ok());
        assert!(local.unprotect_rtp(&mut wire.clone()).is_err());
    }

    #[test]
    fn each_remote_ssrc_has_its_own_srtcp_context() {
        let (mut local, mut remote) = pair(vec![0xA, 0xB]);
        for _ in 0..1_100 {
            let wire = protect_rtcp(&mut remote, &rtcp_packet(0xA));
            local.unprotect_rtcp(&wire).unwrap();
        }
        let slow = protect_rtcp(&mut remote, &rtcp_packet(0xB));
        local.unprotect_rtcp(&slow).unwrap();
        assert_eq!(local.remote_rtcp_contexts(), 2);
    }

    #[test]
    fn only_local_ssrcs_can_send() {
        let (mut local, _) = pair(Vec::new());
        let mut out = vec![0u8; 256];
        assert!(local.protect_rtp(&rtp_packet(0x2222, 1), &mut out).is_err());
        assert!(local.protect_rtcp(&rtcp_packet(0x2222), &mut out).is_err());
        assert!(local.protect_rtp(&rtp_packet(0x1111, 1), &mut out).is_ok());
        assert!(local.protect_rtcp(&rtcp_packet(0x1111), &mut out).is_ok());
    }
}
