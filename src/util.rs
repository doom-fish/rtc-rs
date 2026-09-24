//! Randomness for ICE credentials, SSRCs and SDP session ids, drawn from the
//! system CSPRNG (`SecRandomCopyBytes`).

use crate::error::{Result, RtcError};
use zeroize::Zeroizing;

const ICE_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) const ICE_UFRAG_LEN: usize = 22;
pub(crate) const ICE_PWD_LEN: usize = 32;

const _: () = assert!(ICE_UFRAG_LEN * 6 >= 128 && ICE_PWD_LEN * 6 >= 128);

const SSRC_ATTEMPTS: usize = 64;

/// Generate a random ICE `ufrag`/`pwd`-style token of `len` characters.
pub(crate) fn random_ice_string(len: usize) -> Result<Zeroizing<String>> {
    let mut bytes = Zeroizing::new(vec![0u8; len]);
    stun::rng::fill(&mut bytes).map_err(|_| RtcError::Rng)?;
    let mut out = Zeroizing::new(String::with_capacity(len));
    out.extend(
        bytes
            .iter()
            .map(|b| char::from(ICE_CHARS[usize::from(b & 0x3f)])),
    );
    Ok(out)
}

pub(crate) fn random_u64() -> Result<u64> {
    stun::rng::random_array::<8>()
        .map(u64::from_be_bytes)
        .map_err(|_| RtcError::Rng)
}

pub(crate) fn random_ssrcs(count: usize) -> Result<Vec<u32>> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..SSRC_ATTEMPTS {
        if out.len() == count {
            break;
        }
        let ssrc = stun::rng::random_array::<4>()
            .map(u32::from_be_bytes)
            .map_err(|_| RtcError::Rng)?;
        if ssrc != 0 && !out.contains(&ssrc) {
            out.push(ssrc);
        }
    }
    if out.len() == count {
        Ok(out)
    } else {
        Err(RtcError::Rng)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn credentials_have_the_expected_length_and_alphabet() {
        let ufrag = random_ice_string(ICE_UFRAG_LEN).unwrap();
        let pwd = random_ice_string(ICE_PWD_LEN).unwrap();
        assert_eq!(ufrag.len(), ICE_UFRAG_LEN);
        assert_eq!(pwd.len(), ICE_PWD_LEN);
        assert!(ufrag
            .bytes()
            .chain(pwd.bytes())
            .all(|b| ICE_CHARS.contains(&b)));
    }

    #[test]
    fn credentials_differ_between_draws() {
        let draws: HashSet<String> = (0..64)
            .map(|_| random_ice_string(ICE_PWD_LEN).unwrap().to_string())
            .collect();
        assert_eq!(draws.len(), 64);
    }

    #[test]
    fn every_ice_character_is_reachable() {
        let mut seen = HashSet::new();
        for _ in 0..64 {
            seen.extend(random_ice_string(ICE_PWD_LEN).unwrap().bytes());
        }
        assert_eq!(seen.len(), ICE_CHARS.len());
    }

    #[test]
    fn ssrcs_are_distinct_and_non_zero() {
        for _ in 0..256 {
            let ssrcs = random_ssrcs(3).unwrap();
            assert_eq!(ssrcs.len(), 3);
            assert!(ssrcs.iter().all(|&s| s != 0));
            assert_ne!(ssrcs[0], ssrcs[1]);
            assert_ne!(ssrcs[0], ssrcs[2]);
            assert_ne!(ssrcs[1], ssrcs[2]);
        }
    }

    #[test]
    fn session_ids_differ() {
        let a = random_u64().unwrap();
        let b = random_u64().unwrap();
        assert_ne!(a, b);
    }
}
