//! ICE agent setup and the [`ice::PacketSink`] that fans inbound datagrams out
//! to the worker by [`ice::PacketKind`].

use super::demux::{muxed_kind, MuxedKind};
use super::Inbound;
use crate::config::RtcConfig;
use crate::error::{Result, RtcError};
use ice::{IceAgent, IceConfig, IceRole, PacketKind, PacketSink};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

/// The sink installed on the [`IceAgent`]. Non-STUN datagrams are classified
/// and forwarded to the worker; unknown datagrams are counted and dropped.
pub struct WorkerSink {
    tx: SyncSender<Inbound>,
    dropped_other: AtomicU64,
}

impl WorkerSink {
    /// Create a sink delivering into `tx`.
    #[must_use]
    pub fn new(tx: SyncSender<Inbound>) -> Self {
        Self { tx, dropped_other: AtomicU64::new(0) }
    }

    /// Number of `Other`/unroutable datagrams dropped so far.
    #[must_use]
    #[allow(dead_code)]
    pub fn dropped_other(&self) -> u64 {
        self.dropped_other.load(Ordering::Relaxed)
    }
}

impl PacketSink for WorkerSink {
    fn on_packet(&self, kind: PacketKind, bytes: &[u8], _from: SocketAddr) {
        let msg = match kind {
            // STUN is consumed by the agent and never reaches the sink, but be
            // defensive.
            PacketKind::Stun => return,
            PacketKind::Dtls => Inbound::Dtls(bytes.to_vec()),
            PacketKind::Srtp => match muxed_kind(bytes) {
                MuxedKind::Rtp => Inbound::Srtp(bytes.to_vec()),
                MuxedKind::Rtcp => Inbound::Srtcp(bytes.to_vec()),
            },
            PacketKind::ChannelData | PacketKind::Other => {
                self.dropped_other.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        // Never block the agent's receive loop: drop on a full channel.
        let _ = self.tx.try_send(msg);
    }
}

/// Build and start an [`IceAgent`] for one side of the connection.
///
/// `controlling` selects the ICE role (the offerer is controlling). For
/// loopback-only configs the agent binds to `127.0.0.1` so two in-process
/// agents can reach each other with host candidates alone.
pub fn setup_agent(
    cfg: &RtcConfig,
    controlling: bool,
    local_ufrag: &str,
    local_pwd: &str,
    remote_ufrag: &str,
    remote_pwd: &str,
) -> Result<IceAgent> {
    let role = if controlling { IceRole::Controlling } else { IceRole::Controlled };
    let mut ice_cfg = IceConfig::new(role, local_ufrag, local_pwd, remote_ufrag, remote_pwd);

    if cfg.loopback_only {
        ice_cfg.bind_addr = Some("127.0.0.1:0".parse().expect("valid literal"));
    }

    for server in &cfg.ice_servers {
        if server.is_turn() {
            // TURN relay candidates are a deferred feature (see COVERAGE.md);
            // STUN server-reflexive gathering covers the common NAT case.
            continue;
        }
        if let Some((host, port)) = split_host_port(server.host_port()) {
            ice_cfg.stun_servers.push(stun::Endpoint::new(host, port));
        }
    }

    IceAgent::new(ice_cfg).map_err(RtcError::from)
}

/// Attach the worker sink and gather host (and any STUN) candidates.
pub fn install_sink_and_gather(agent: &IceAgent, sink: Arc<WorkerSink>) -> Result<()> {
    agent.set_packet_sink(sink);
    agent.start_gathering().map_err(RtcError::from)?;
    agent.end_of_candidates();
    Ok(())
}

fn split_host_port(host_port: &str) -> Option<(String, u16)> {
    let (host, port) = host_port.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    Some((host.to_string(), port))
}
