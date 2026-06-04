//! Drive the DTLS handshake over the ICE agent's selected pair.
//!
//! `dtls::Connection` is sans-I/O: we pull its outbound records with
//! [`dtls::Connection::poll_send`] and ship them through
//! [`ice::IceAgent::send_on_selected`], and feed inbound DTLS datagrams in with
//! [`dtls::Connection::feed_recv`]. Retransmission timers are serviced from the
//! worker loop.

use crate::error::{Result, RtcError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Create a DTLS connection for the given role, offering AES-128-GCM SRTP.
pub fn new_connection(role: dtls::Role) -> Result<dtls::Connection> {
    let config = dtls::Config {
        role,
        srtp_profiles: vec![dtls::SrtpProfile::AeadAes128Gcm],
        expected_peer_fingerprint: None,
    };
    dtls::Connection::new(config).map_err(RtcError::from)
}

/// A millisecond clock used for DTLS retransmission timers.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Verify the peer certificate fingerprint against the value from the remote
/// SDP `a=fingerprint`. Returns [`RtcError::FingerprintMismatch`] on mismatch.
pub fn verify_peer_fingerprint(
    conn: &dtls::Connection,
    expected: &[u8; 32],
) -> Result<()> {
    match conn.peer_fingerprint() {
        Some(actual) if &actual == expected => Ok(()),
        Some(_) => Err(RtcError::FingerprintMismatch),
        None => Err(RtcError::Dtls("handshake done but no peer certificate".into())),
    }
}
