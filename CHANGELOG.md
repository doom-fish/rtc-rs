# Changelog

All notable changes to this crate are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.0.0/) and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2025

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

[0.1.0]: https://github.com/doom-fish/rtc-rs/releases/tag/v0.1.0
