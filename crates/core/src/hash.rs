//! SplitMix64-based hashing of integer words.
//!
//! The finalizer is the one from SplitMix64 (Steele, Lea & Flood 2014). Words are absorbed
//! one at a time, so the result depends on order and length; it is not cryptographic.

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

/// The SplitMix64 output finalizer: a bijective avalanche of 64 bits.
#[inline]
pub const fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Hashes a sequence of 64-bit words under `seed`.
pub fn hash64(seed: u64, words: &[u64]) -> u64 {
    let mut h = mix64(seed.wrapping_add(GOLDEN));
    for &w in words {
        h = mix64((h ^ w).wrapping_add(GOLDEN));
    }
    mix64(h ^ words.len() as u64)
}

/// Hashes a byte string under `seed`. Bytes are packed little-endian into words; the
/// trailing partial word is zero-padded and the byte length is mixed in, so `[0]` and
/// `[0, 0]` hash differently.
pub fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = mix64(seed.wrapping_add(GOLDEN));
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        h = mix64((h ^ w).wrapping_add(GOLDEN));
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut buf = [0u8; 8];
        buf[..rest.len()].copy_from_slice(rest);
        h = mix64((h ^ u64::from_le_bytes(buf)).wrapping_add(GOLDEN));
    }
    mix64(h ^ bytes.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix64_matches_splitmix64_reference() {
        // First three outputs of the reference SplitMix64 generator seeded with 0:
        // state += GOLDEN; output = mix64(state).
        assert_eq!(mix64(GOLDEN), 0xE220_A839_7B1D_CDAF);
        assert_eq!(mix64(GOLDEN.wrapping_mul(2)), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(mix64(GOLDEN.wrapping_mul(3)), 0x06C4_5D18_8009_454F);
    }

    #[test]
    fn hash64_fixed_vectors() {
        assert_eq!(hash64(0, &[]), 0x4821_8226_FF3C_D4BF);
        assert_eq!(hash64(1, &[]), 0xDCE4_23FC_82C0_D5B8);
        assert_eq!(hash64(0, &[1, 2, 3]), 0xFD73_5931_0CC4_7E9E);
        assert_eq!(hash64(42, &[0xDEAD_BEEF]), 0xE610_9325_E401_7660);
    }

    #[test]
    fn hash64_depends_on_order_length_and_seed() {
        assert_ne!(hash64(0, &[1, 2]), hash64(0, &[2, 1]));
        assert_ne!(hash64(0, &[0]), hash64(0, &[0, 0]));
        assert_ne!(hash64(0, &[7]), hash64(1, &[7]));
    }

    #[test]
    fn hash_bytes_matches_word_packing_and_length() {
        assert_ne!(hash_bytes(0, &[0]), hash_bytes(0, &[0, 0]));
        assert_ne!(hash_bytes(0, b"skywave"), hash_bytes(0, b"skywavf"));
        assert_eq!(hash_bytes(9, b"skywave!"), hash_bytes(9, b"skywave!"));
        assert_eq!(hash_bytes(0, b""), 0x4821_8226_FF3C_D4BF);
        assert_eq!(hash_bytes(0, b"skywave"), 0xFE14_B152_0A34_2DDC);
    }
}
