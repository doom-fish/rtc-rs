# Coverage

`rtc-rs` is an **orchestration** crate: it does not re-implement the wire
protocols itself but composes the sibling crates (`sdp`, `ice`, `dtls`, `srtp`,
`rtp`, `opus`, `stun`, `turn`) into a single synchronous PeerConnection-style
engine. The tables below record what the orchestration layer wires up today,
what is intentionally deferred, and the honest gaps.

## Session lifecycle

| Stage | Status | Notes |
|-------|--------|-------|
| Offer/answer (SDP) | ✅ | `create_offer`, `create_answer`, `set_remote_description` via the `sdp` crate |
| Media-line negotiation | ✅ | Opus audio + H.264 video; codec payload-type matching |
| Data channel (`m=application`) | ⚠️ refused | An active application section is rejected with a negotiation error; an already-rejected (port 0) section is tolerated. SCTP is not implemented. |
| ICE gathering | ✅ host | Host candidates always gathered |
| ICE gathering (STUN srflx) | ✅ | Server-reflexive when a STUN server is configured |
| ICE gathering (TURN relay) | ❌ deferred | `IceServer` accepts TURN URLs but relay allocation is not wired (`turn` crate present, not invoked) |
| Trickle ICE | ✅ | `add_remote_candidate[_immediate]`, `Event::LocalCandidate` |
| ICE restart | ❌ deferred | — |
| DTLS-SRTP handshake | ✅ | Client/server role from offer/answer |
| Key export + SRTP keying | ✅ | RFC 5764 §4.2 split, `EXTRACTOR-dtls_srtp` |

## Media plane

| Capability | Status | Notes |
|------------|--------|-------|
| Opus encode/decode | ✅ | `opus` crate; 48 kHz, jitter-buffered receive |
| Audio jitter buffer | ✅ | Sequence-unwrap, reorder, late-drop, depth target |
| H.264 packetize/depacketize | ✅ | Annex-B in/out, FU-A fragmentation, access-unit reassembly |
| Video jitter buffer + flush | ✅ | End-of-stream `flush()` drains below target depth |
| Per-SSRC SRTP contexts | ✅ | Independent ROC / replay window per SSRC over one transport |
| rtcp-mux | ✅ | RTP and RTCP share the selected ICE pair |
| BUNDLE | ✅ | Single transport for audio + video |
| RTCP sender/receiver reports | ✅ | Loss, jitter, round-trip inputs |
| TWCC (transport-wide cc) | ✅ | Header-extension stamping + feedback |
| Congestion control reaction | ⚠️ partial | TWCC feedback is parsed/recorded; bitrate adaptation is minimal |
| NACK / RTX retransmission | ❌ deferred | — |
| FEC | ❌ deferred | — |
| Simulcast / SVC | ❌ deferred | — |

## Native media sources (`apple-media` feature)

| Source | Status | Notes |
|--------|--------|-------|
| Application-supplied `AudioSource` / `VideoSource` | ✅ | The default integration path |
| `SilenceAudioSource` | ✅ | Built-in, always available |
| Camera/microphone capture | ❌ deferred | Reserved behind `apple-media`; sibling bindings declared as optional deps |
| Hardware H.264 encode/decode | ❌ deferred | Without `apple-media`, `Event::VideoFrame` carries pointer `0`; the raw access unit is delivered via `Event::VideoAccessUnit` for caller-side decode |

`apple-media` is **off by default** so the core stack and the
`loopback_full_stack` test build and run on any host without camera,
microphone, or hardware-codec access.

## Concurrency model

| Property | Status |
|----------|--------|
| Synchronous API (no `async`) | ✅ |
| One worker thread per connection | ✅ |
| `std::sync::mpsc` command/event channels | ✅ |
| `Send` `PeerConnection` handle | ✅ |
| Zero crates.io runtime deps (only `libc` + siblings) | ✅ |

## Tests

| Test | Asserts |
|------|---------|
| `offer_answer` | Signaling completes; cannot answer own offer |
| `reject_data_channel` | Active data channel refused; rejected one tolerated |
| `jitter_buffer` | Reorder/release, gap skipping, late counting |
| `audio_round_trip` | Opus encode → RTP → decode yields PCM |
| `video_round_trip` | H.264 access unit spanning multiple packets reassembles |
| `twcc_seq_stamping` | Shared counter stamps consecutive transport sequences |
| `loopback_full_stack` | **Gating:** two peers complete SDP+ICE+DTLS+SRTP and exchange Opus audio + H.264 video both directions |
