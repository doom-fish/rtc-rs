# Coverage audit

This crate is an orchestrator, not a protocol implementation, so there is no
single upstream SDK whose symbol surface it mirrors. Instead this audit records,
honestly, which protocol features the orchestration layer actually drives in the
sibling crates versus what those crates expose but `rtc-rs` does not yet call.

## What the engine exercises

| Sibling crate | Exercised by `rtc-rs` | Exposed but unused |
|---------------|------------------------|--------------------|
| `sdp` | Offer/answer build + parse, m-line negotiation, DTLS fingerprint, ICE ufrag/pwd, rtcp-mux, BUNDLE, codec/payload mapping | Renegotiation edge cases, data-channel (SCTP) attributes |
| `ice` | Host gathering, STUN srflx gathering, candidate trickle, connectivity checks, selected-pair send/recv | TURN relay candidates, ICE restart, aggressive nomination tuning |
| `dtls` | Handshake (client + server roles), `use_srtp` profile negotiation, `export_keying_material` | Certificate pinning beyond fingerprint match, session resumption |
| `srtp` | Per-SSRC SRTP protect/unprotect (GCM), SRTCP protect/unprotect | AES-CM (HMAC-SHA1) profiles — GCM is negotiated in practice |
| `rtp` | Packet build/parse, header extensions (MID, TWCC), sequence helpers | Some extension types not negotiated here |
| `opus` | Encode/decode at 48 kHz mono/stereo | FEC/DTX tuning knobs |
| `stun` | Endpoint config for srflx gathering | Standalone binding/keepalive use |
| `turn` | — (declared dependency) | Entire relay client — TURN is deferred |

## Known gaps (intentional for 0.1.0)

- **TURN relay is not wired.** `IceServer` accepts TURN URLs and the `turn`
  crate is a dependency, but no allocation is performed. Connectivity relies on
  host and STUN server-reflexive candidates.
- **No SCTP / data channels.** An active `m=application` section is refused; a
  pre-rejected one is tolerated.
- **No retransmission resilience.** NACK/RTX, FEC, simulcast, and SVC are out of
  scope for this release.
- **Congestion control is observational.** TWCC feedback is recorded; sender
  bitrate adaptation is minimal.
- **Native capture/codec is feature-gated and deferred.** Without the
  `apple-media` feature, `Event::VideoFrame` reports a null pixel-buffer pointer
  and callers decode the Annex-B access unit from `Event::VideoAccessUnit`
  themselves.

## Verification

The end-to-end behaviour is verified by `tests/loopback_full_stack.rs`, which
stands up two in-process peers and asserts both complete the full
SDP + ICE + DTLS + SRTP negotiation and exchange Opus audio and H.264 video in
both directions. The remaining integration tests pin the individual pipeline
stages (jitter buffering, Opus round-trip, H.264 reassembly, TWCC stamping,
data-channel refusal). All tests run without network, camera, microphone, or
hardware-codec access.
