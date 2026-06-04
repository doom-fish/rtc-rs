//! SRTP/SRTCP session lifecycle: derive keys from the DTLS exporter and build
//! the sender/receiver sessions (RFC 5764 §4.2).
//!
//! With BUNDLE + rtcp-mux a single transport carries several SSRCs (audio,
//! video, …). Each SSRC is its own SRTP cryptographic context — own rollover
//! counter and replay window — even though they share one master key. The
//! `srtp` crate's [`SrtpSession`] models exactly one such context, so we mint a
//! session per SSRC on demand and keep them in a map.

use crate::error::{Result, RtcError};
use srtp::{Profile, SrtcpSession, SrtpKey, SrtpSession};
use std::collections::HashMap;

/// Map a negotiated DTLS SRTP profile to the `srtp` crate profile.
#[must_use]
pub fn map_profile(p: dtls::SrtpProfile) -> Profile {
    match p {
        dtls::SrtpProfile::AeadAes128Gcm => Profile::AeadAes128Gcm,
        dtls::SrtpProfile::AeadAes256Gcm => Profile::AeadAes256Gcm,
    }
}

/// Master key + salt for one direction, used to mint per-SSRC sessions.
struct DirectionKey {
    key: Vec<u8>,
    salt: Vec<u8>,
    profile: Profile,
}

impl DirectionKey {
    fn sender(&self) -> Result<SrtpSession> {
        let key = SrtpKey::from_master(&self.key, &self.salt, self.profile)?;
        Ok(SrtpSession::new_sender(key))
    }

    fn receiver(&self) -> Result<SrtpSession> {
        let key = SrtpKey::from_master(&self.key, &self.salt, self.profile)?;
        Ok(SrtpSession::new_receiver(key))
    }
}

/// The SRTP/SRTCP cryptographic state for a connection. RTP sessions are
/// per-SSRC; RTCP is single-context per direction (one local report sender).
pub struct SrtpContext {
    send: DirectionKey,
    recv: DirectionKey,
    rtp_send: HashMap<u32, SrtpSession>,
    rtp_recv: HashMap<u32, SrtpSession>,
    /// Protects our outbound RTCP.
    pub rtcp_send: SrtcpSession,
    /// Unprotects inbound RTCP.
    pub rtcp_recv: SrtcpSession,
    /// The negotiated profile.
    #[allow(dead_code)]
    pub profile: Profile,
}

impl SrtpContext {
    /// The outbound SRTP session for `ssrc`, created on first use.
    pub fn rtp_send(&mut self, ssrc: u32) -> Result<&mut SrtpSession> {
        if !self.rtp_send.contains_key(&ssrc) {
            let s = self.send.sender()?;
            self.rtp_send.insert(ssrc, s);
        }
        Ok(self.rtp_send.get_mut(&ssrc).expect("just inserted"))
    }

    /// The inbound SRTP session for `ssrc`, created on first use.
    pub fn rtp_recv(&mut self, ssrc: u32) -> Result<&mut SrtpSession> {
        if !self.rtp_recv.contains_key(&ssrc) {
            let s = self.recv.receiver()?;
            self.rtp_recv.insert(ssrc, s);
        }
        Ok(self.rtp_recv.get_mut(&ssrc).expect("just inserted"))
    }
}

/// Derive the SRTP key block from the DTLS exporter and build the context.
///
/// Per RFC 5764 §4.2 the exporter output is split as
/// `client_master_key | server_master_key | client_master_salt |
/// server_master_salt`. `is_dtls_client` selects which half we send with.
pub fn derive(conn: &dtls::Connection, is_dtls_client: bool) -> Result<SrtpContext> {
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
    let client_key = &keys[..key_len];
    let server_key = &keys[key_len..];
    let client_salt = &salts[..salt_len];
    let server_salt = &salts[salt_len..];

    // The DTLS client sends with the client keys, the server with the server
    // keys (RFC 5764 §4.2).
    let (send_key, send_salt, recv_key, recv_salt) = if is_dtls_client {
        (client_key, client_salt, server_key, server_salt)
    } else {
        (server_key, server_salt, client_key, client_salt)
    };

    let send = DirectionKey {
        key: send_key.to_vec(),
        salt: send_salt.to_vec(),
        profile,
    };
    let recv = DirectionKey {
        key: recv_key.to_vec(),
        salt: recv_salt.to_vec(),
        profile,
    };

    let rtcp_send = SrtcpSession::new_sender(SrtpKey::from_master(send_key, send_salt, profile)?);
    let rtcp_recv = SrtcpSession::new_receiver(SrtpKey::from_master(recv_key, recv_salt, profile)?);

    Ok(SrtpContext {
        send,
        recv,
        rtp_send: HashMap::new(),
        rtp_recv: HashMap::new(),
        rtcp_send,
        rtcp_recv,
        profile,
    })
}

