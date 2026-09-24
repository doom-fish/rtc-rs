use std::collections::HashSet;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use stun::attribute::types;
use stun::{Attribute, Class, ErrorCode, Message, TransactionId};

pub const TURN_USER: &str = "relay-user";
pub const TURN_PASSWORD: &str = "relay-password";
const TURN_REALM: &str = "relay.test";
const TURN_NONCE: &str = "relay-nonce";

#[derive(Default)]
struct State {
    client: Option<SocketAddr>,
    permissions: HashSet<IpAddr>,
}

pub struct FakeTurn {
    pub addr: SocketAddr,
    pub relay_addr: SocketAddr,
    pub allocations: Arc<AtomicUsize>,
    pub relayed: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

fn bound_socket() -> UdpSocket {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    socket
}

fn key() -> Vec<u8> {
    stun::auth::long_term_key(TURN_USER, TURN_REALM, TURN_PASSWORD).to_vec()
}

fn respond(request: &Message, attributes: Vec<Attribute>) -> Vec<u8> {
    let mut builder = Message::builder(request.method(), Class::SuccessResponse)
        .transaction_id(*request.transaction_id());
    for attribute in attributes {
        builder = builder.attribute(attribute);
    }
    builder
        .with_message_integrity_sha1(&key())
        .with_fingerprint()
        .build_and_encode()
        .unwrap()
}

fn handle(
    control: &UdpSocket,
    relay: &UdpSocket,
    relay_addr: SocketAddr,
    state: &Mutex<State>,
    turn: &FakeTurnCounters,
    message: &Message,
    from: SocketAddr,
) {
    let xor = turn::attribute::xor_key(message.transaction_id().as_bytes());
    let attributes: Vec<turn::Attribute> = message
        .attributes()
        .iter()
        .filter_map(|a| turn::Attribute::from_stun(a, &xor).ok().flatten())
        .collect();
    if message.class() == Class::Indication && message.method() == turn::Method::Send.to_stun() {
        let peer = attributes.iter().find_map(|a| match a {
            turn::Attribute::XorPeerAddress(p) => Some(*p),
            _ => None,
        });
        let data = attributes.iter().find_map(|a| match a {
            turn::Attribute::Data(d) => Some(d.clone()),
            _ => None,
        });
        if let (Some(peer), Some(data)) = (peer, data) {
            if state.lock().unwrap().permissions.contains(&peer.ip()) {
                turn.relayed.fetch_add(1, Ordering::Relaxed);
                let _ = relay.send_to(&data, peer);
            }
        }
        return;
    }
    if message.class() != Class::Request {
        return;
    }
    let authenticated = matches!(message.get(types::NONCE), Some(Attribute::Nonce(n)) if n == TURN_NONCE)
        && message.verify_message_integrity(&key()).is_ok();
    let response = if !authenticated {
        Message::builder(message.method(), Class::ErrorResponse)
            .transaction_id(*message.transaction_id())
            .attribute(Attribute::ErrorCode(ErrorCode::new(401, "Unauthorized")))
            .attribute(Attribute::Realm(TURN_REALM.to_owned()))
            .attribute(Attribute::Nonce(TURN_NONCE.to_owned()))
            .build_and_encode()
            .unwrap()
    } else if message.method() == turn::Method::Allocate.to_stun() {
        state.lock().unwrap().client = Some(from);
        turn.allocations.fetch_add(1, Ordering::Relaxed);
        respond(
            message,
            vec![
                turn::Attribute::XorRelayedAddress(relay_addr).to_stun(&xor),
                Attribute::XorMappedAddress(from),
                turn::Attribute::Lifetime(600).to_stun(&xor),
            ],
        )
    } else if message.method() == turn::Method::CreatePermission.to_stun() {
        let mut guard = state.lock().unwrap();
        for attribute in &attributes {
            if let turn::Attribute::XorPeerAddress(peer) = attribute {
                guard.permissions.insert(peer.ip());
            }
        }
        drop(guard);
        respond(message, Vec::new())
    } else if message.method() == turn::Method::Refresh.to_stun() {
        let lifetime = attributes.iter().find_map(|a| match a {
            turn::Attribute::Lifetime(l) => Some(*l),
            _ => None,
        });
        respond(
            message,
            vec![turn::Attribute::Lifetime(lifetime.unwrap_or(600)).to_stun(&xor)],
        )
    } else {
        respond(message, Vec::new())
    };
    let _ = control.send_to(&response, from);
}

struct FakeTurnCounters {
    relayed: Arc<AtomicUsize>,
    allocations: Arc<AtomicUsize>,
}

impl FakeTurn {
    pub fn spawn() -> Self {
        let control = bound_socket();
        let relay = bound_socket();
        let addr = control.local_addr().unwrap();
        let relay_addr = relay.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let allocations = Arc::new(AtomicUsize::new(0));
        let relayed = Arc::new(AtomicUsize::new(0));

        let control_thread = {
            let control = control.try_clone().unwrap();
            let relay = relay.try_clone().unwrap();
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            let counters = FakeTurnCounters {
                relayed: Arc::clone(&relayed),
                allocations: Arc::clone(&allocations),
            };
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while !stop.load(Ordering::SeqCst) {
                    match control.recv_from(&mut buf) {
                        Ok((n, from)) => {
                            if let Ok(message) = Message::decode(&buf[..n]) {
                                handle(
                                    &control, &relay, relay_addr, &state, &counters, &message, from,
                                );
                            }
                        }
                        Err(e)
                            if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                        Err(e) => panic!("TURN control receive failed: {e}"),
                    }
                }
            })
        };
        let relay_thread = {
            let state = Arc::clone(&state);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while !stop.load(Ordering::SeqCst) {
                    let (n, from) = match relay.recv_from(&mut buf) {
                        Ok(received) => received,
                        Err(e)
                            if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                        {
                            continue
                        }
                        Err(e) => panic!("TURN relay receive failed: {e}"),
                    };
                    let guard = state.lock().unwrap();
                    let (Some(client), true) =
                        (guard.client, guard.permissions.contains(&from.ip()))
                    else {
                        continue;
                    };
                    drop(guard);
                    let tid = TransactionId::try_random().unwrap();
                    let xor = turn::attribute::xor_key(tid.as_bytes());
                    let indication =
                        Message::builder(turn::Method::Data.to_stun(), Class::Indication)
                            .transaction_id(tid)
                            .attribute(turn::Attribute::XorPeerAddress(from).to_stun(&xor))
                            .attribute(turn::Attribute::Data(buf[..n].to_vec()).to_stun(&xor))
                            .build_and_encode()
                            .unwrap();
                    let _ = control.send_to(&indication, client);
                }
            })
        };
        Self {
            addr,
            relay_addr,
            allocations,
            relayed,
            stop,
            handles: vec![control_thread, relay_thread],
        }
    }

    pub fn url(&self) -> String {
        format!("turn:127.0.0.1:{}", self.addr.port())
    }
}

impl Drop for FakeTurn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}
