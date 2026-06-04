//! Small internal helpers with no external dependencies.

use std::time::{SystemTime, UNIX_EPOCH};

/// A tiny `xorshift64*` PRNG — enough for ICE credentials and SSRCs, not for
/// cryptography (the crypto lives in `dtls`/`srtp`/`cryptokit`).
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Seed from the clock and a caller-supplied salt.
    #[must_use]
    pub fn new(salt: u64) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        let mut state = nanos ^ salt.wrapping_mul(0x2545_F491_4F6C_DD1D);
        if state == 0 {
            state = 0x9E37_79B9_7F4A_7C15;
        }
        Self { state }
    }

    /// Next 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Next 32-bit value.
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }
}

const ICE_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Generate a random ICE `ufrag`/`pwd`-style token of `len` characters.
#[must_use]
pub fn random_ice_string(rng: &mut Rng, len: usize) -> String {
    let mut s = String::with_capacity(len);
    for _ in 0..len {
        let idx = (rng.next_u32() as usize) % ICE_CHARS.len();
        s.push(ICE_CHARS[idx] as char);
    }
    s
}
