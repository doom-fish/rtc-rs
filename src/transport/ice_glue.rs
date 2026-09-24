//! ICE agent setup and the [`ice::PacketSink`] that fans inbound datagrams out
//! to the worker by [`ice::PacketKind`].

use super::demux::{muxed_kind, MuxedKind};
use super::{Inbound, InboundKind};
use crate::config::{ResolvedIceServer, RtcConfig};
use crate::error::{Result, RtcError};
use ice::{IceAgent, IceConfig, IceRole, PacketKind, PacketSink, TransportPolicy};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

pub const INBOUND_QUEUE: usize = 1024;
pub const MAX_INBOUND_DATAGRAM: usize = 16 * 1024;

/// The sink installed on the [`IceAgent`]. Non-STUN datagrams are classified
/// and forwarded to the worker; unknown datagrams are counted and dropped.
pub struct WorkerSink {
    tx: SyncSender<Inbound>,
    dropped: AtomicU64,
}

impl WorkerSink {
    /// Create a sink delivering into `tx`.
    #[must_use]
    pub fn new(tx: SyncSender<Inbound>) -> Self {
        Self {
            tx,
            dropped: AtomicU64::new(0),
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl PacketSink for WorkerSink {
    fn on_packet(&self, kind: PacketKind, bytes: &[u8], from: SocketAddr) {
        let kind = match kind {
            PacketKind::Dtls => InboundKind::Dtls,
            PacketKind::Srtp => match muxed_kind(bytes) {
                MuxedKind::Rtp => InboundKind::Srtp,
                MuxedKind::Rtcp => InboundKind::Srtcp,
            },
            PacketKind::Stun | PacketKind::ChannelData | PacketKind::Other => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        // Never block the agent's receive loop: drop on a full channel.
        if bytes.len() > MAX_INBOUND_DATAGRAM
            || self
                .tx
                .try_send(Inbound {
                    kind,
                    bytes: bytes.to_vec(),
                    from,
                })
                .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub struct AgentParams<'a> {
    pub config: &'a RtcConfig,
    pub controlling: bool,
    pub local_ufrag: &'a str,
    pub local_pwd: &'a str,
    pub remote_ufrag: &'a str,
    pub remote_pwd: &'a str,
}

/// Build and start an [`IceAgent`] for one side of the connection.
///
/// `controlling` selects the ICE role (the offerer is controlling). For
/// loopback-only configs the agent binds to `127.0.0.1` so two in-process
/// agents can reach each other with host candidates alone.
pub fn setup_agent(params: &AgentParams<'_>) -> Result<IceAgent> {
    let cfg = params.config;
    let role = if params.controlling {
        IceRole::Controlling
    } else {
        IceRole::Controlled
    };
    let mut ice_cfg = IceConfig::new(
        role,
        params.local_ufrag,
        params.local_pwd,
        params.remote_ufrag,
        params.remote_pwd,
    );
    ice_cfg.clock = Arc::clone(&cfg.clock);
    ice_cfg.mdns = cfg.mdns.clone();
    if cfg.loopback_only {
        ice_cfg.bind_addr = Some(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
    }
    if cfg.relay_only {
        ice_cfg.policy = TransportPolicy::Relay;
    }
    for server in &cfg.ice_servers {
        match server.resolve()? {
            ResolvedIceServer::Stun(endpoint) => ice_cfg.stun_servers.push(endpoint),
            ResolvedIceServer::Turn(turn) => ice_cfg.turn_servers.push(turn),
        }
    }
    IceAgent::new(ice_cfg).map_err(RtcError::from)
}

/// Attach the worker sink and gather host (and any STUN) candidates.
pub fn install_sink_and_gather(
    agent: &IceAgent,
    sink: Arc<WorkerSink>,
    failed: Arc<AtomicBool>,
) -> Result<()> {
    agent.set_packet_sink(sink);
    agent.on_connection_failed(move || failed.store(true, Ordering::Release));
    agent.start_gathering().map_err(RtcError::from)
}
