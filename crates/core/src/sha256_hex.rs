//! Lowercase-hex SHA-256 — the encoding every persisted digest key in this workspace uses
//! (cassette keys, ontology content hashes, dedup and judge cache keys, eval plan hashes).
//!
//! These used to be written as `format!("{:x}", Sha256::digest(..))`. sha2 0.11's digest output
//! (a `hybrid_array::Array`) no longer implements `LowerHex`, so the encoding lives here instead,
//! and must stay byte-identical to what 0.10 produced: 64 lowercase hex digits, no separators.
//! Changing it would silently orphan every key already on disk.

use sha2::{Digest, Sha256};

/// Lowercase hex of `bytes`, two digits per byte.
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Lowercase-hex SHA-256 of `data`.
pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    to_hex(&Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS 180-2 known answers. Pins the encoding persisted keys were written with.
    #[test]
    fn matches_known_answers() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn to_hex_pads_and_lowercases() {
        assert_eq!(to_hex(&[0x00, 0x0a, 0xff]), "000aff");
    }
}
