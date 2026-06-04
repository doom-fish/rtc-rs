//! Connection-lifecycle state enums mirrored from the WebRTC spec.

/// ICE transport state (RFC 8445 / W3C `RTCIceConnectionState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IceConnectionState {
    /// No checks running yet.
    New,
    /// Connectivity checks in progress.
    Checking,
    /// A usable candidate pair was nominated.
    Connected,
    /// All checks completed and the best pair is in use.
    Completed,
    /// Connectivity was lost (consent failure or timeout).
    Failed,
    /// Temporarily disconnected; may recover.
    Disconnected,
    /// The transport was shut down.
    Closed,
}

/// DTLS transport state (W3C `RTCDtlsTransportState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DtlsState {
    /// Waiting for ICE to select a pair before the handshake can start.
    New,
    /// DTLS handshake in progress.
    Connecting,
    /// Handshake complete, SRTP keys derived.
    Connected,
    /// Handshake failed or the transport closed.
    Failed,
    /// Closed cleanly.
    Closed,
}

/// Signaling state (W3C `RTCSignalingState`, JSEP subset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalingState {
    /// No offer or answer exchanged.
    Stable,
    /// A local offer has been created/applied.
    HaveLocalOffer,
    /// A remote offer has been applied.
    HaveRemoteOffer,
    /// The connection is closed.
    Closed,
}
