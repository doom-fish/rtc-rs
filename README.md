# rtc-rs

A synchronous, dependency-light WebRTC media engine for native macOS meeting
clients. `rtc-rs` ties eight sibling crates — `sdp`, `ice`, `dtls`, `srtp`,
`rtp`, `opus`, `stun`, and `turn` — together behind a single
**PeerConnection-equivalent** API.

It performs the full offer/answer exchange, ICE connectivity establishment, the
DTLS-SRTP handshake, and Opus audio + H.264 video transport. Everything runs on
one background `std::thread`: there is **no `async`, no executor, and no
crates.io dependency** beyond `zeroize` and `libc` (the latter pulled in
transitively by the sibling crates).

## Requirements

- macOS 14 or later at run time. `stun-rs`, `turn-rs` and `ice-rs` link
  `networkframework`, whose Swift bridge targets macOS 14; `dtls-rs` and
  `srtp-rs` add `cryptokit-rs`/`security-rs` (macOS 12) and `apple-cf`
  (macOS 13).
- Rust 1.82 or later.

## Installation

The crate is not published to crates.io; depend on it by path from a sibling
checkout:

```toml
[dependencies]
rtc-rs = { path = "../rtc-rs", version = "0.2" }
```

## Architecture

```text
  application
       │  create_offer / set_remote_description / add_*_track / poll_event
       ▼
  PeerConnection (Send handle)  ──Command──►  worker thread
       ▲                                          │
       └──────────────── Event ◄──────────────────┘
                                                  │
       ┌─────────────────────────────────────────────────────────────┐
       │  ICE ⇄ DTLS ⇄ SRTP ⇄ RTP ⇄ {Opus, H.264}  (+ RTCP/TWCC/cc)   │
       └─────────────────────────────────────────────────────────────┘
```

The public handle (`PeerConnection`) is a thin, `Send` front end. All protocol
state lives on the worker thread; the handle talks to it with commands and
receives `Event`s back through a bounded channel that the application drains
with `poll_event()` or `recv_event_timeout()`.

## Quick start

```rust
use std::sync::Arc;
use std::time::Duration;
use rtc::{
    Event, MediaConfig, PeerConnection, RemoteDescription, RtcConfig,
    SilenceAudioSource,
};

// Offerer.
let mut pc = PeerConnection::new(
    RtcConfig::builder()
        .audio(MediaConfig::opus_default())
        .loopback_only(true)
        .build(),
)?;
pc.add_audio_track(Arc::new(SilenceAudioSource::default()))?;

let offer = pc.create_offer()?;
let offer_text = rtc::write_sdp(offer.sdp())?;
// ...send `offer_text` over your own signaling channel, receive an answer...
# let answer_text: String = unimplemented!();
pc.set_remote_description(RemoteDescription::Answer(rtc::Answer::parse(&answer_text)?))?;

// Trickle remote candidates in as they arrive, then pump events.
while let Some(ev) = pc.recv_event_timeout(Duration::from_millis(100)) {
    match ev {
        Event::LocalCandidate(line) => println!("send to the peer: {line}"),
        Event::Connected => println!("media is flowing"),
        Event::AudioFrame(ssrc, pcm, rate, channels) => {
            // render `pcm` (interleaved i16)
        }
        Event::Failed(error) => {
            eprintln!("connection failed: {error}");
            break;
        }
        _ => {}
    }
}
# Ok::<(), rtc::RtcError>(())
```

## Examples

| Example | What it shows |
|---------|---------------|
| `01_offer_answer_local` | Drive the offer/answer + candidate exchange between two in-process peers without media. |
| `02_loopback` | A complete two-peer call: SDP + ICE + DTLS-SRTP plus Opus audio and synthetic H.264 video. |
| `03_silence_call` | The minimal audio-only path using the built-in silence source. |

```sh
cargo run --example 02_loopback
```

## Security model

- **Peer identity.** The remote SDP's single SHA-256 `a=fingerprint` is pinned
  in the DTLS configuration, which each side creates once the remote
  description is known, so a certificate that does not match
  fails the handshake itself with `RtcError::FingerprintMismatch`. Several
  distinct SHA-256 fingerprints are refused, and bundled media sections must
  agree on their ICE and DTLS parameters.
- **ICE credentials and SSRCs** come from the system CSPRNG
  (`SecRandomCopyBytes`): the ufrag carries 132 random bits and the password
  192.
- **Source addresses.** Before a pair is selected only DTLS from an
  ICE-validated remote is accepted; afterwards every DTLS, SRTP and SRTCP
  datagram must come from the selected pair's remote address.
- **SRTP state.** A receive context for an SSRC is only stored once one of its
  packets authenticates. SRTP and SRTCP each keep their own per-SSRC receive
  contexts, at most 32 each, least recently used first out. Send contexts exist
  only for our own SSRCs.
- **Failure.** DTLS errors, a DTLS handshake that has not finished
  `RtcConfig::handshake_timeout` (30 s by default) after ICE selected a pair,
  ICE failure and RFC 7675 consent expiry end the connection:
  `Event::Failed(error)` and `ConnectionState::Failed` are reported, media
  stops and the worker exits.
- rtc-rs keeps its own copies of the ICE password and TURN credentials in
  zeroizing storage and redacts them in `Debug` output. The SDP documents
  carry the ICE password in clear, as the protocol requires.

## DTLS roles

The offerer creates its DTLS certificate with the offer and offers
`a=setup:actpass`; the DTLS connection is created when the answer arrives. An
answer with `a=setup:active` makes the offerer the DTLS server and one with
`a=setup:passive` the client. An answer with `actpass` (which RFC 5763 reserves
for offers), `holdconn` or no `a=setup` is refused with
`RtcError::Negotiation`. The answerer honours the offer's `a=setup` (`active`
makes it the server, `passive` the client) and uses
`RtcConfig::answer_dtls_role` (client by default) for `actpass`.

## ICE

Host candidates are always gathered; STUN servers add server-reflexive and TURN
servers relayed candidates, both gathered on the agent's own socket without
blocking the worker. TURN runs over UDP through `turn-rs` (`turns:` and
`?transport=tcp` are refused at construction), and `relay_only` restricts ICE
to relayed candidates. Remote mDNS (`.local`) candidates follow
`RtcConfig::mdns` and are rejected by default. Candidates are trickled
(`Event::LocalCandidate`, `Event::IceGatheringComplete`,
`add_remote_candidate`, `add_remote_end_of_candidates`), and candidates inlined
in the remote SDP are used too.

## RTCP and congestion control

The worker sends a sender or receiver report every second, NACK or PLI when the
peer's `a=rtcp-fb` allows it, and TWCC feedback every 100 ms when
`transport-cc` and the TWCC header extension are negotiated. From the peer it
reads report blocks about our streams (loss and round-trip time), NACK (answered
from a history of the last 128 audio and 512 video packets), PLI and FIR
(forwarded to `VideoSource::request_keyframe`), REMB and TWCC. Payload types,
MIDs and header-extension ids follow the answer.

The congestion controller is loss-based AIMD only, with no delay-based
estimation. The audio target is applied to the Opus encoder; the video target
is reported in `Stats::target_video_bitrate` for the application's own encoder
to apply, capped by REMB.

## Media sources

The engine is source-agnostic. You implement [`AudioSource`] (PCM) and
[`VideoSource`] (encoded H.264 access units), or use the built-in
[`SilenceAudioSource`], and the worker pulls frames from them on its own
cadence. Received video is delivered as Annex-B access units for the caller to
decode.

The optional `apple-media` feature only compiles the sibling Apple-SDK
bindings (`avcapture`, `audiounit`, `audiotoolbox`, `videotoolbox`,
`apple-metal`, `coreanimation`); no capture, encode, decode or render sources
are implemented yet. It is **off by default**. See
[`COVERAGE.md`](COVERAGE.md) for the current state.

## Testing

```sh
cargo test
```

The gating integration test is `tests/loopback_full_stack.rs`: two in-process
`PeerConnection`s complete the entire SDP + ICE + DTLS + SRTP negotiation and
exchange Opus audio and H.264 video, asserting both directions receive both
media types, with the offerer as DTLS server and, after a passive answer, as
DTLS client. Other suites drive rtc-rs as the DTLS server against a scripted
peer built from the sibling crates, fingerprint mismatches on both sides, a
stalled handshake, consent expiry with an injected clock, and a relay-only
call through an in-process TURN server. Every socket binds `127.0.0.1`.

## Scope and non-goals

- **Synchronous only.** No `async`/`await`, no runtime. One worker thread per
  connection.
- **One negotiation.** No renegotiation, ICE restart or data channels (an
  active `m=application` section is refused).
- **GCM SRTP profiles** (`AEAD_AES_128_GCM` / `AEAD_AES_256_GCM`), negotiated
  through DTLS-SRTP; the connection fails instead of rekeying when a key's
  lifetime is used up.
- **Opus audio, H.264 video** (packetization-mode 1). Video access units are
  delivered as Annex-B for the caller to decode.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.

[`AudioSource`]: https://docs.rs/rtc
[`VideoSource`]: https://docs.rs/rtc
[`SilenceAudioSource`]: https://docs.rs/rtc
