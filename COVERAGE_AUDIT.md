# Coverage audit

This crate is an orchestrator, not a protocol implementation, so there is no
single upstream SDK whose symbol surface it mirrors. Instead this audit records,
honestly, which protocol features the orchestration layer actually drives in the
sibling crates versus what those crates expose but `rtc-rs` does not yet call.

## What the engine exercises

| Sibling crate | Exercised by `rtc-rs` | Exposed but unused |
|---------------|------------------------|--------------------|
| `sdp` | Offer/answer build + parse, session-level fallbacks, m-line negotiation (payload type, MID, extmap ids, direction, `rtcp-fb`), DTLS fingerprint and `a=setup`, ICE ufrag/pwd, inline candidates, `end-of-candidates`, rtcp-mux, BUNDLE | Renegotiation (`previous_answer`), data-channel (SCTP) attributes |
| `ice` | Host, STUN srflx and UDP TURN relay gathering on the agent socket, relay-only policy, trickle, remote end-of-candidates, mDNS policy, injectable clock, connectivity checks, consent, failure callback, source-filtered delivery, selected-pair send | ICE restart, TCP candidates, custom limits and integrity modes |
| `dtls` | Handshake in both roles with the peer fingerprint pinned (config for the answerer, `set_expected_peer_fingerprint` for the offerer), fatal errors and `Timeout`, retransmission timer, `use_srtp`, `export_keying_material` | Role-independent certificates (not offered by `dtls` 0.2), session resumption |
| `srtp` | Per-SSRC SRTP and SRTCP sessions (GCM) from one derived key per direction, encrypted SRTCP required on receive | AES-CM (HMAC-SHA1) profiles, custom replay windows, `with_roc` |
| `rtp` | Packet build/parse, header extensions by negotiated id (MID, TWCC), SR/RR, NACK, PLI, FIR, REMB, TWCC encode/parse, reception statistics | abs-send-time, SDES, BYE, APP, SLI |
| `opus` | Encode/decode at 48 kHz mono, run-time bitrate changes | FEC/PLC (`decode_fec`, `decode_lost`), DTX tuning |
| `stun` | STUN URI parsing, the system CSPRNG (`stun::rng`) for credentials, SSRCs and session ids | Standalone binding client |
| `turn` | TURN URI parsing and long-term credentials handed to `ice` (which runs the allocation) | Direct `TurnClient` use, TCP/TLS transports |

## Known gaps (0.2.0)

- **The offerer cannot be the DTLS client.** `dtls` 0.2 generates the
  certificate inside `Connection::new` for a fixed role, so the fingerprint in
  an offer is bound to the server role. An answer with `a=setup:passive` is
  refused with `RtcError::Unsupported`.
- **One certificate per peer.** An SDP carrying several distinct SHA-256
  fingerprints is refused; `dtls` pins exactly one.
- **No SCTP / data channels.** An active `m=application` section is refused; a
  rejected one is tolerated.
- **No RTX, FEC, simulcast or SVC.** NACKs are answered by resending the
  original SRTP packet from a bounded history.
- **Congestion control is loss-based only.** The video target is reported for
  the application's encoder; nothing in `rtc-rs` encodes video.
- **No native media.** The `apple-media` feature compiles the sibling
  Apple-SDK bindings but implements no capture, encode, decode or render
  source.

## Verification

The end-to-end behaviour is verified by `tests/loopback_full_stack.rs`, which
stands up two in-process peers in both offer/answer directions and asserts both
complete the full SDP + ICE + DTLS + SRTP negotiation and exchange Opus audio
and H.264 video. `tests/dtls_roles.rs` runs rtc-rs as the DTLS server against a
peer scripted from the sibling crates, `tests/failure_states.rs` covers
fingerprint mismatches, a stalled handshake and consent expiry, and
`tests/turn_relay.rs` carries a call through an in-process TURN server. The
remaining tests pin the individual pipeline stages. All tests run without
network access beyond `127.0.0.1`, camera, microphone, or hardware-codec
access.
