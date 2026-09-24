# Coverage

`rtc-rs` is an **orchestration** crate: it does not re-implement the wire
protocols itself but composes the sibling crates (`sdp`, `ice`, `dtls`, `srtp`,
`rtp`, `opus`, `stun`, `turn`) into a single synchronous PeerConnection-style
engine. The tables below record what the orchestration layer wires up today,
what is intentionally deferred, and the honest gaps. A ✅ means the behaviour is
implemented and exercised by a test named in the last table.

## Session lifecycle

| Stage | Status | Notes |
|-------|--------|-------|
| Offer/answer (SDP) | ✅ | `create_offer`, `create_answer`, `set_remote_description` via the `sdp` crate; `write_sdp` re-exported for serialization |
| Media-line negotiation | ✅ | Opus audio + H.264 (packetization-mode 1) video; payload types, MIDs, header-extension ids, direction and `a=rtcp-fb` follow the answer |
| Data channel (`m=application`) | ⚠️ refused | An active application section is rejected with a negotiation error; a rejected (port 0) section is tolerated. SCTP is not implemented. |
| ICE gathering (host) | ✅ | Host candidates always gathered (`127.0.0.1` with `loopback_only`) |
| ICE gathering (STUN srflx) | ✅ | Gathered on the agent's socket; does not block the worker |
| ICE gathering (TURN relay) | ✅ UDP | `IceServer::turn` over UDP through `turn-rs`; `relay_only` restricts ICE to relayed candidates. `turns:` and `?transport=tcp` are refused. |
| Trickle ICE | ✅ | `add_remote_candidate[_immediate]`, `add_remote_end_of_candidates`, `Event::LocalCandidate`, `Event::IceGatheringComplete`; SDP-inlined candidates are used |
| mDNS candidates | ⚠️ policy | `RtcConfig::mdns`; rejected by default |
| ICE restart / renegotiation | ❌ deferred | A second offer or answer returns `InvalidState` |
| DTLS roles | ✅ | The offerer creates its certificate with the offer and becomes the DTLS server for an `active` answer or the client for a `passive` one; `actpass`, `holdconn` or no `a=setup` in an answer is refused. The answerer honours the offer's `a=setup` and `answer_dtls_role` for `actpass`. |
| Fingerprint pinning | ✅ | The single SHA-256 remote fingerprint is pinned in the DTLS configuration, so a mismatch fails the handshake |
| Key export + SRTP keying | ✅ | RFC 5764 §4.2 split, `EXTRACTOR-dtls_srtp` |
| Failure reporting | ✅ | DTLS errors, a handshake timeout, ICE failure and consent expiry give `Event::Failed`, `ConnectionState::Failed`, and stop the worker |

## Media plane

| Capability | Status | Notes |
|------------|--------|-------|
| Opus encode/decode | ✅ | `opus` crate; 48 kHz mono, jitter-buffered receive |
| Audio jitter buffer | ✅ | Sequence-unwrap, reorder, late-drop, depth target |
| H.264 packetize/depacketize | ✅ | Annex-B in/out, FU-A fragmentation, access-unit reassembly |
| Video jitter buffer + flush | ✅ | End-of-stream `flush()` drains below target depth |
| Per-SSRC SRTP and SRTCP contexts | ✅ | Created only after a packet authenticates; at most 32 per direction, least recently used evicted |
| Source filtering | ✅ | After selection, only the selected pair's remote may send DTLS, SRTP or SRTCP |
| rtcp-mux / BUNDLE | ✅ | One transport for audio + video |
| RTCP sender/receiver reports | ✅ | Every second; report blocks give loss, jitter and round-trip time |
| TWCC | ✅ | Wrap-aware recording and feedback; sent only when `transport-cc` is negotiated |
| NACK | ✅ | Sent for small gaps when negotiated; received NACKs are answered from a 128/512-packet send history. No RTX. |
| PLI / FIR | ✅ | PLI sent on video discontinuities when negotiated; received PLI/FIR call `VideoSource::request_keyframe` |
| REMB | ⚠️ read only | Caps the reported video target; never sent |
| Congestion control | ⚠️ limited | Loss-based AIMD from report blocks and TWCC; no delay-based estimation. The audio target drives the Opus encoder; the video target is only reported. |
| FEC / RTX / simulcast / SVC | ❌ deferred | — |

## Native media sources (`apple-media` feature)

| Source | Status | Notes |
|--------|--------|-------|
| Application-supplied `AudioSource` / `VideoSource` | ✅ | The only integration path: PCM audio, encoded H.264 video |
| `SilenceAudioSource` | ✅ | Built-in, always available |
| Camera/microphone capture | ❌ not implemented | `apple-media` only compiles `avcapture`, `audiounit` and `audiotoolbox` |
| Hardware H.264 encode/decode | ❌ not implemented | `apple-media` only compiles `videotoolbox`; received video is delivered as Annex-B through `Event::VideoAccessUnit` |
| Rendering | ❌ not implemented | `apple-metal` and `coreanimation` are compiled, not used |

`apple-media` is **off by default** so the core stack builds and its tests run
on any host without camera, microphone, or hardware-codec access.

## Concurrency model

| Property | Status |
|----------|--------|
| Synchronous API (no `async`) | ✅ |
| One worker thread per connection | ✅ |
| Command channel in, bounded event queue out (oldest media, stats and error events dropped first) | ✅ |
| `Send` `PeerConnection` handle | ✅ |
| crates.io runtime deps limited to `zeroize` and (transitively) `libc` | ✅ |

## Tests

| Test | Asserts |
|------|---------|
| `offer_answer` | Signaling states, `a=setup` in offer and answer, answerer server preference, random credentials and SSRCs, ICE server validation, redacted TURN credentials |
| `reject_data_channel` | Active data channel refused; rejected one tolerated |
| `jitter_buffer` | Reorder/release, gap skipping, late counting |
| `audio_round_trip` | Opus encode → RTP → decode yields PCM |
| `video_round_trip` | H.264 access unit spanning multiple packets reassembles |
| `twcc_seq_stamping` | Shared counter stamps consecutive transport sequences |
| `loopback_full_stack` | **Gating:** two peers complete SDP+ICE+DTLS+SRTP in both offer/answer directions and after a passive answer (offerer as DTLS client), exchanging Opus audio + H.264 video with RTT and bitrate statistics |
| `dtls_roles` | rtc-rs as DTLS server against a scripted DTLS-client peer (passive preference and `a=setup:active` offers) with audio both ways under non-default payload type, MID and extension ids; the offerer takes the client role for a passive answer; `actpass` and `holdconn` answers refused |
| `failure_states` | Fingerprint mismatch on the server and on the client side, a stalled DTLS handshake, and consent expiry with an injected clock all reach `Failed`; media stops after consent expiry |
| `turn_relay` | A relay-only call through an in-process TURN server carries audio both ways |
| unit tests | CSPRNG credentials, spoofed-SSRC floods (no contexts), bounded eviction, per-SSRC SRTCP, TWCC wrap, reordering and delta reconstruction, NACK gap bounds, feedback parsing, event-queue drop policy |
