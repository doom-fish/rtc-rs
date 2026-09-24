//! Transport plane: ICE connectivity, the DTLS handshake, SRTP key derivation,
//! and SRTP/SRTCP demultiplexing.

pub mod demux;
pub mod dtls_glue;
pub mod ice_glue;
pub mod srtp_glue;

use std::net::SocketAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundKind {
    /// A DTLS record to feed into [`dtls::Connection::feed_recv`].
    Dtls,
    /// An SRTP media packet.
    Srtp,
    /// An SRTCP control packet.
    Srtcp,
}

/// An inbound datagram routed off the shared ICE socket, demultiplexed by
/// [`ice::PacketKind`] and (for the RTP family) by [`demux`].
#[derive(Debug, Clone)]
pub struct Inbound {
    pub kind: InboundKind,
    pub bytes: Vec<u8>,
    pub from: SocketAddr,
}
