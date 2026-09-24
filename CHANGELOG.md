# Changelog

All notable changes to this crate are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - Unreleased

### Security

- The remote SDP fingerprint is pinned in the DTLS configuration of both sides
  (the connection is created once the remote description is known), so a
  certificate that does not match fails the handshake itself. The post-handshake
  comparison, which ran with `expected_peer_fingerprint: None`, is removed.
- ICE ufrag (132 bits), ICE password (192 bits), SSRCs and the SDP session id
  come from the system CSPRNG (`SecRandomCopyBytes`). The clock-seeded
  xorshift generator is removed.
- An SRTP receive context is only stored once a packet for its SSRC
  authenticates. SRTP and SRTCP keep at most 32 receive contexts each, least
  recently used first out, and session keys are derived once per direction
  instead of once per packet from an unknown SSRC.
- DTLS, SRTP and SRTCP datagrams are only accepted from the selected ICE
  pair's remote (DTLS from an ICE-validated remote before selection), and the
  source address is passed to `dtls::Connection::feed_recv`.
- ICE passwords and TURN credentials are held in zeroizing storage and
  redacted in `Debug`.

### Fixed

- DTLS errors, including the retransmission `Timeout`, are no longer
  discarded: they fail the connection after the fatal alert is flushed. The
  DTLS timer fires when `timeout()` reports 0 ms left instead of treating the
  remaining milliseconds as a deadline.
- The answerer honours the offer's `a=setup`, and the offerer follows the
  answer's: it creates its certificate (`dtls::Certificate`) with the offer and
  its DTLS connection when the answer arrives, as the server for `active` and
  the client for `passive`. A `passive` answer no longer leaves both sides
  waiting as DTLS servers.
- Every remote SSRC has its own SRTCP receive context, so RTCP from a slower
  SSRC is no longer dropped as a replay; every local sender SSRC has its own
  SRTCP send context.
- TWCC sequence numbers are unwrapped before sorting, so a wrap no longer
  produces a 65,536-entry span. A long gap or a delta beyond 16 bits starts a
  new feedback packet, and deltas are measured from the 64 ms reference time in
  exact 250 µs ticks.
- Received TWCC sequence numbers were never recorded, because parsed header
  extensions are always generic; they are read under the negotiated id.
- Payload types, MIDs, header-extension ids and directions follow the answer
  instead of local defaults, so peers that choose other values interoperate.
  The H.264 payload type chosen for an answer must use packetization-mode 1.
- ICE failure and RFC 7675 consent expiry are registered through
  `on_connection_failed`, and a `ConsentExpired` or `Failed` send ends the
  connection, so media stops.
- STUN and TURN gathering no longer block the worker, and end-of-candidates is
  signalled when gathering completes rather than right after the host
  candidates.
- Candidates and `a=end-of-candidates` inlined in the remote SDP are used, and
  the remote end of candidates can be signalled, so ICE can report that every
  pair failed.
- `add_video_track` after negotiation replaces the video source instead of
  being ignored.
- The congestion controller is driven by report blocks and TWCC feedback about
  our outgoing media instead of inbound loss, starts at and never exceeds the
  configured bitrate, and its audio target is applied to the Opus encoder.
- Inbound jitter is computed from arrival times in the media clock rate
  instead of milliseconds, and the statistics no longer reset the loss interval
  that receiver reports use.
- NACK generation is bounded: a gap of more than 64 packets is a discontinuity
  (PLI for video), and losses older than 512 packets are forgotten.
- The event queue drops the oldest media, statistics and non-fatal error events
  when full, as documented, and keeps state changes.
- A video source that always has a frame can no longer stall the worker, and
  audio pacing skips ahead after a stall instead of sending a burst.
- A close_notify from the peer after the DTLS handshake ends the connection as
  `Closed` instead of `Failed`.
- The SSRC of an incoming SRTP packet is read from the RTP header alone, so
  packets with the padding bit set are no longer rejected before decryption.
- A pending H.264 access unit and FU-A reassembly are capped at 4 MiB
  (`media::video_recv::MAX_ACCESS_UNIT_BYTES`); a larger access unit is dropped
  instead of buffered without limit.
- The decrypted RTP receive path no longer panics when an Opus receiver cannot
  be created (`c9e7a04`, committed after the 0.1.0 tag).

### Changed

- **Breaking:** sibling requirements are `sdp-rs`, `ice-rs`, `dtls-rs`,
  `srtp-rs`, `rtp-rs`, `opus-rs`, `stun-rs` and `turn-rs` `>=0.2, <0.3`, and
  for `apple-media` `avcapture` `>=0.7, <0.8`, `audiounit` `>=0.4, <0.5`,
  `audiotoolbox` `>=0.5, <0.6`, `videotoolbox` `>=0.21, <0.22`, `apple-metal`
  `>=0.10, <0.11` and `coreanimation-rs` `>=0.6, <0.7`, and the `rtp-rs`
  dev-dependency is `>=0.2, <0.3`. The unused direct `libc` dependency is
  removed and `zeroize` `>=1.6, <1.9` is added.
- **Breaking:** binaries need macOS 14 or later (inherited from
  `networkframework` through `stun-rs`, `turn-rs` and `ice-rs`), and the crate
  needs Rust 1.82.
- **Breaking:** `PeerConnection::new` returns `Result` (CSPRNG draws and ICE
  server validation), and `add_audio_track`/`add_video_track` return `Result`
  (kind not configured, worker gone).
- **Breaking:** `add_remote_candidate` and `add_remote_candidate_immediate`
  parse the line on the calling thread, applying `RtcConfig::mdns`, and return
  parse errors.
- **Breaking:** `actpass` (RFC 5763 reserves it for offers), `holdconn` or no
  `a=setup` in an answer, and `holdconn` in an offer, return
  `RtcError::Negotiation`. Several distinct
  SHA-256 fingerprints return `Unsupported`, and bundled sections that disagree
  on ICE or DTLS parameters return `Negotiation`.
- **Breaking:** `Event::Disconnected(String)` is replaced by
  `Event::Failed(RtcError)`, `Event::Error` carries an `RtcError`, and
  `Event::VideoFrame`, which always carried a null pointer, is removed.
- **Breaking:** `CapturedFrame::pixel_buffer` is removed: nothing encoded or
  released the pixel buffer. `data` is the encoded Annex-B access unit.
- **Breaking:** `IceServer` fields are private (`url()`, `username()`),
  `is_turn()` and `host_port()` are removed, and `PartialEq`/`Eq` are no longer
  derived. URLs are parsed with `stun::StunUri` and `turn::TurnUri`: `stuns:`,
  `turns:` and `?transport=tcp` are `Unsupported`; other schemes and TURN
  without credentials are `Config` errors.
- **Breaking:** `RtcConfig` gains `answer_dtls_role`, `handshake_timeout`,
  `mdns`, `clock` and `relay_only`, loses `Debug` derive in favour of a
  redacting impl, and `RtcConfigBuilder` wraps an `RtcConfig`.
- **Breaking:** `RtcError` derives `Clone`, `PartialEq` and `Eq` and gains
  `Config`, `Unsupported`, `Timeout`, `ConsentExpired`, `IceFailed` and `Rng`;
  DTLS fingerprint mismatches map to `FingerprintMismatch`.
- **Breaking:** `Stats` is `#[non_exhaustive]`.
- **Breaking:** `media::AudioSender::new` and `VideoSender::new` take optional
  MID and TWCC extension ids, and `VideoSender::encode` returns `Result`.
- The offer advertises `transport-cc`, `nack`, `nack pli` and `ccm fir`
  feedback, and the answer echoes the offered ones rtc-rs handles.

### Added

- `ConnectionState` with `Failed`, `Event::ConnectionStateChange`,
  `PeerConnection::connection_state`, `DtlsRole` and
  `PeerConnection::dtls_role`.
- A DTLS handshake timeout (`RtcConfig::handshake_timeout`, 30 s by default,
  `DEFAULT_HANDSHAKE_TIMEOUT`).
- TURN relay candidates over UDP through `turn-rs`, and `relay_only`.
- `add_remote_end_of_candidates` and `Event::IceGatheringComplete`.
- Reading RTCP feedback: report blocks (send loss, round-trip time), NACK
  (retransmitted from a 128/512-packet history), PLI and FIR
  (`VideoSource::request_keyframe`), REMB and TWCC.
- `Stats` fields `target_audio_bitrate`, `remote_streams`,
  `dropped_unexpected_source`, `dropped_inbound`, `srtp_rejected`,
  `nacks_received`, `packets_retransmitted`, `keyframe_requests`,
  `send_errors` and `dropped_events`; `send_loss`, `rtt_ms`, `remb_bitrate` and
  the receive bitrates are now filled in.
- Re-exports of `write_sdp`, `Clock`, `SystemClock` and `MdnsPolicy`.
- Tests for both answer directions on loopback (the offerer as DTLS server and,
  after a passive answer, as client), rtc-rs as DTLS server against a scripted
  peer, fingerprint mismatches, a
  stalled handshake, consent expiry with an injected clock, a relay-only call,
  spoofed-SSRC floods and the TWCC wrap.

### Removed

- The xorshift `Rng` and the post-handshake `verify_peer_fingerprint`.

## [0.1.0] - 2026-06-04

Initial release.

### Added

- `PeerConnection`, a synchronous PeerConnection-equivalent handle that drives
  a full WebRTC media session from one background thread with `std::sync::mpsc`
  channels — no `async`, no executor, no crates.io runtime dependency.
- Offer/answer signaling (`create_offer`, `create_answer`,
  `set_remote_description`) layered on the sibling `sdp` crate, re-exporting
  `Offer` / `Answer` so callers own their signaling channel.
- Trickle ICE: host candidate gathering plus STUN server-reflexive support,
  with `add_remote_candidate` / `add_remote_candidate_immediate` and
  `Event::LocalCandidate`.
- DTLS-SRTP handshake and key derivation (RFC 5764 §4.2), establishing
  `AEAD_AES_128_GCM` / `AEAD_AES_256_GCM` SRTP protection.
- Per-SSRC SRTP cryptographic contexts so bundled audio and video tracks
  maintain independent rollover counters and replay windows over a single
  rtcp-muxed transport.
- Opus audio send/receive with a jitter buffer, and H.264 (Annex-B)
  packetize/depacketize with access-unit reassembly. Video access units are
  always surfaced through `Event::VideoAccessUnit`.
- RTCP with sender/receiver reports, loss tracking, and TWCC
  (transport-wide congestion control) sequence stamping and feedback.
- An `Event` stream (`Connected`, `TrackAdded`, `AudioFrame`,
  `VideoFrame`, `VideoAccessUnit`, state-change and `Stats` events) drained via
  `poll_event` / `recv_event_timeout`.
- Pluggable `AudioSource` / `VideoSource` traits and a built-in
  `SilenceAudioSource`.
- Optional `apple-media` feature reserving the sibling Apple-SDK bindings as
  turn-key capture/render sources (off by default).
- Three runnable examples (`01_offer_answer_local`, `02_loopback`,
  `03_silence_call`) and an end-to-end `loopback_full_stack` integration test.

[0.2.0]: https://github.com/doom-fish/rtc-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/doom-fish/rtc-rs/releases/tag/v0.1.0
