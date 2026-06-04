# rtc-rs

A synchronous, dependency-light WebRTC media engine for native macOS meeting
clients. `rtc-rs` ties eight sibling crates — `sdp`, `ice`, `dtls`, `srtp`,
`rtp`, `opus`, `stun`, and `turn` — together behind a single
**PeerConnection-equivalent** API.

It performs the full offer/answer exchange, ICE connectivity establishment, the
DTLS-SRTP handshake, and Opus audio + H.264 video transport. Everything runs on
one background `std::thread` driven by `std::sync::mpsc` channels: there is **no
`async`, no executor, and no crates.io dependency** beyond `libc` (pulled in
transitively by the sibling crates).

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
);
pc.add_audio_track(Arc::new(SilenceAudioSource::default()));

let offer = pc.create_offer()?;
// ...send `offer` over your own signaling channel, receive an `answer`...
# let answer: rtc::Answer = unimplemented!();
pc.set_remote_description(RemoteDescription::Answer(answer))?;

// Trickle remote candidates in as they arrive, then pump events.
while let Some(ev) = pc.recv_event_timeout(Duration::from_millis(100)) {
    match ev {
        Event::Connected => println!("media is flowing"),
        Event::AudioFrame(ssrc, pcm, rate, channels) => {
            // render `pcm` (interleaved i16)
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

## Media sources

The engine is source-agnostic. You implement [`AudioSource`] and
[`VideoSource`] (or use the built-in [`SilenceAudioSource`]) and the worker
pulls frames from them on its own cadence.

The optional `apple-media` feature is reserved for wiring the sibling Apple-SDK
bindings (`avcapture`, `audiounit`, `videotoolbox`, …) as turn-key
capture/render sources. It is **off by default** so the core stack — and the
`loopback_full_stack` integration test — builds and runs on any host without
camera, microphone, or hardware-codec access. See
[`COVERAGE.md`](COVERAGE.md) for the current state.

## Testing

```sh
cargo test
```

The gating integration test is `tests/loopback_full_stack.rs`: two in-process
`PeerConnection`s complete the entire SDP + ICE + DTLS + SRTP negotiation and
exchange Opus audio and H.264 video, asserting both directions receive both
media types.

## Scope and non-goals

- **Synchronous only.** No `async`/`await`, no runtime. One worker thread per
  connection.
- **Trickle ICE.** Host candidates always; STUN server-reflexive when
  configured. TURN relay is declared in config but not yet wired (see
  [`COVERAGE.md`](COVERAGE.md)).
- **GCM SRTP profiles** (`AEAD_AES_128_GCM` / `AEAD_AES_256_GCM`), negotiated
  through DTLS-SRTP.
- **Opus audio, H.264 video.** Video access units are always delivered as
  Annex-B so callers can decode them without the `apple-media` feature.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.

[`AudioSource`]: https://docs.rs/rtc
[`VideoSource`]: https://docs.rs/rtc
[`SilenceAudioSource`]: https://docs.rs/rtc
