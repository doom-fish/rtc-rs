//! Drive the DTLS handshake over the ICE agent's selected pair.
//!
//! `dtls::Connection` is sans-I/O: we pull its outbound records with
//! [`dtls::Connection::poll_send`] and ship them through
//! [`ice::IceAgent::send_on_selected`], and feed inbound DTLS datagrams in with
//! [`dtls::Connection::feed_recv`]. Retransmission timers are serviced from the
//! worker loop.

use crate::error::{Result, RtcError};
use crate::state::DtlsRole;

/// Create a DTLS connection for the given role, offering AES-128-GCM SRTP.
pub fn new_connection(
    role: DtlsRole,
    certificate: dtls::Certificate,
    expected_peer: [u8; 32],
) -> Result<dtls::Connection> {
    let config = match role {
        DtlsRole::Client => dtls::Config::client(),
        DtlsRole::Server => dtls::Config::server(),
    };
    let config = config
        .with_certificate(certificate)
        .with_expected_peer_fingerprint(expected_peer);
    dtls::Connection::new(config).map_err(RtcError::from)
}
