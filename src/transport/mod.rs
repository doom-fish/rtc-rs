//! Transport plane: ICE connectivity, the DTLS handshake, SRTP key derivation,
//! and SRTP/SRTCP demultiplexing.

pub mod demux;
pub mod dtls_glue;
pub mod ice_glue;
pub mod srtp_glue;

/// An inbound datagram routed off the shared ICE socket, demultiplexed by
/// [`ice::PacketKind`] and (for the RTP family) by [`demux`].
#[derive(Debug, Clone)]
pub enum Inbound {
    /// A DTLS record to feed into [`dtls::Connection::feed_recv`].
    Dtls(Vec<u8>),
    /// An SRTP media packet.
    Srtp(Vec<u8>),
    /// An SRTCP control packet.
    Srtcp(Vec<u8>),
}
