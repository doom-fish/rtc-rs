//! SDP negotiation glue: building local offers/answers and applying remote
//! descriptions onto transport parameters.
//!
//! All WebRTC profile knowledge (BUNDLE, rtcp-mux, ICE, DTLS-SRTP
//! fingerprints, H.264/Opus fmtp) lives in the sibling `sdp` crate; this module
//! only orchestrates it.

mod apply;
mod answer;
mod offer;

pub use apply::{apply_remote, RemoteParams};
pub use answer::build_answer;
pub use offer::build_offer;

use sdp::SetupRole;

/// Local DTLS-SRTP / ICE identity used on every media section.
#[derive(Debug, Clone)]
pub struct LocalSdpIdentity {
    /// ICE username fragment.
    pub ice_ufrag: String,
    /// ICE password.
    pub ice_pwd: String,
    /// Local DTLS certificate fingerprint (SHA-256, 32 bytes).
    pub fingerprint_sha256: [u8; 32],
}

/// One-byte RTP header-extension ids we negotiate (RFC 8285).
pub const MID_EXT_ID: u8 = 1;
/// TWCC transport-wide sequence-number header-extension id.
pub const TWCC_EXT_ID: u8 = 3;

/// Decide our local DTLS role from the remote `a=setup` value (RFC 5763 §5).
///
/// * remote `active`  → we are passive → DTLS **server**
/// * remote `passive` → we are active  → DTLS **client**
/// * remote `actpass` → we choose active → DTLS **client**
#[must_use]
pub fn dtls_role_for_remote(remote: SetupRole) -> dtls::Role {
    match remote {
        SetupRole::Active => dtls::Role::Server,
        SetupRole::Passive | SetupRole::ActPass | SetupRole::HoldConn => dtls::Role::Client,
    }
}
