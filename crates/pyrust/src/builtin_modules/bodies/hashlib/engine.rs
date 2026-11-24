//! Unified hash engine, algorithm resolution, HMAC, and PBKDF2-HMAC.

use super::blake2::{Blake2Params, Blake2b, Blake2s};
use super::md5::Md5;
use super::sha1::Sha1;
use super::sha2::{Sha256, Sha512};
use super::sha3::{KeccakSponge, Sha3Kind};

#[derive(Clone)]
pub(super) enum HashState {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Sha512(Sha512),
    Sha3(KeccakSponge),
    Blake2b(Blake2b),
    Blake2s(Blake2s),
}

impl HashState {
    /// Resolve an algorithm name matching CPython 3.12's `hashlib.new()`.
    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "md5" | "MD5" => Some(Self::Md5(Md5::new())),
            "sha1" | "SHA1" | "sha-1" | "SHA-1" => Some(Self::Sha1(Sha1::new())),
            "sha224" | "SHA224" | "sha-224" | "SHA-224" => Some(Self::Sha256(Sha256::new_224())),
            "sha256" | "SHA256" | "sha-256" | "SHA-256" => Some(Self::Sha256(Sha256::new_256())),
            "sha384" | "SHA384" | "sha-384" | "SHA-384" => Some(Self::Sha512(Sha512::new_384())),
            "sha512" | "SHA512" | "sha-512" | "SHA-512" => Some(Self::Sha512(Sha512::new_512())),
            "sha512_224" | "SHA512_224" | "sha512-224" | "SHA512-224" | "sha-512/224"
            | "SHA-512/224" => Some(Self::Sha512(Sha512::new_512_224())),
            "sha512_256" | "SHA512_256" | "sha512-256" | "SHA512-256" | "sha-512/256"
            | "SHA-512/256" => Some(Self::Sha512(Sha512::new_512_256())),
            "sha3_224" | "sha3-224" => Some(Self::Sha3(KeccakSponge::new(Sha3Kind::Sha3_224))),
            "sha3_256" | "sha3-256" => Some(Self::Sha3(KeccakSponge::new(Sha3Kind::Sha3_256))),
            "sha3_384" | "sha3-384" => Some(Self::Sha3(KeccakSponge::new(Sha3Kind::Sha3_384))),
            "sha3_512" | "sha3-512" => Some(Self::Sha3(KeccakSponge::new(Sha3Kind::Sha3_512))),
            "shake_128" | "shake-128" => Some(Self::Sha3(KeccakSponge::new(Sha3Kind::Shake128))),
            "shake_256" | "shake-256" => Some(Self::Sha3(KeccakSponge::new(Sha3Kind::Shake256))),
            "blake2b" => Some(Self::Blake2b(Blake2b::new_default())),
            "blake2s" => Some(Self::Blake2s(Blake2s::new_default())),
            _ => None,
        }
    }

    pub(super) fn new_blake2b(params: Blake2Params<'_>) -> Self {
        Self::Blake2b(Blake2b::new(params))
    }

    pub(super) fn new_blake2s(params: Blake2Params<'_>) -> Self {
        Self::Blake2s(Blake2s::new(params))
    }

    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Md5(_) => "md5",
            Self::Sha1(_) => "sha1",
            Self::Sha256(s) => {
                if s.digest_size() == 28 {
                    "sha224"
                } else {
                    "sha256"
                }
            }
            Self::Sha512(s) => s.name(),
            Self::Sha3(s) => s.kind().name(),
            Self::Blake2b(_) => "blake2b",
            Self::Blake2s(_) => "blake2s",
        }
    }

    pub(super) fn digest_size(&self) -> usize {
        match self {
            Self::Md5(_) => 16,
            Self::Sha1(_) => 20,
            Self::Sha256(s) => s.digest_size(),
            Self::Sha512(s) => s.digest_size(),
            Self::Sha3(s) => s.kind().digest_size(),
            Self::Blake2b(s) => s.digest_size(),
            Self::Blake2s(s) => s.digest_size(),
        }
    }

    pub(super) fn block_size(&self) -> usize {
        match self {
            Self::Md5(_) | Self::Sha1(_) | Self::Sha256(_) | Self::Blake2s(_) => 64,
            Self::Sha512(_) | Self::Blake2b(_) => 128,
            Self::Sha3(s) => s.kind().rate(),
        }
    }

    pub(super) fn is_shake(&self) -> bool {
        matches!(self, Self::Sha3(s) if s.kind().is_shake())
    }

    pub(super) fn update(&mut self, data: &[u8]) {
        match self {
            Self::Md5(s) => s.update(data),
            Self::Sha1(s) => s.update(data),
            Self::Sha256(s) => s.update(data),
            Self::Sha512(s) => s.update(data),
            Self::Sha3(s) => s.update(data),
            Self::Blake2b(s) => s.update(data),
            Self::Blake2s(s) => s.update(data),
        }
    }

    pub(super) fn digest(&self) -> Vec<u8> {
        match self {
            Self::Md5(s) => s.finalize(),
            Self::Sha1(s) => s.finalize(),
            Self::Sha256(s) => s.finalize(),
            Self::Sha512(s) => s.finalize(),
            Self::Sha3(s) => s.finalize_fixed(),
            Self::Blake2b(s) => s.finalize(),
            Self::Blake2s(s) => s.finalize(),
        }
    }

    pub(super) fn digest_xof(&self, length: usize) -> Vec<u8> {
        match self {
            Self::Sha3(s) => s.finalize_xof(length),
            _ => self.digest(),
        }
    }
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Precomputed inner/outer HMAC contexts for fast repeated evaluation (PBKDF2).
struct PreparedHmac {
    inner: HashState,
    outer: HashState,
    digest_size: usize,
}

impl PreparedHmac {
    fn new(base: &HashState, key: &[u8]) -> Self {
        let block_size = base.block_size();
        let mut key_block = vec![0u8; block_size];
        if key.len() > block_size {
            let mut h = base.clone();
            h.update(key);
            let hashed = h.digest();
            key_block[..hashed.len()].copy_from_slice(&hashed);
        } else {
            key_block[..key.len()].copy_from_slice(key);
        }

        let mut ipad = vec![0x36u8; block_size];
        let mut opad = vec![0x5cu8; block_size];
        for i in 0..block_size {
            ipad[i] ^= key_block[i];
            opad[i] ^= key_block[i];
        }

        let mut inner = base.clone();
        inner.update(&ipad);
        let mut outer = base.clone();
        outer.update(&opad);

        Self {
            inner,
            outer,
            digest_size: base.digest_size(),
        }
    }

    fn mac(&self, data: &[u8]) -> Vec<u8> {
        let mut i = self.inner.clone();
        i.update(data);
        let inner_digest = i.digest();
        let mut o = self.outer.clone();
        o.update(&inner_digest);
        o.digest()
    }

    fn mac_two(&self, part1: &[u8], part2: &[u8]) -> Vec<u8> {
        let mut i = self.inner.clone();
        i.update(part1);
        i.update(part2);
        let inner_digest = i.digest();
        let mut o = self.outer.clone();
        o.update(&inner_digest);
        o.digest()
    }
}

/// RFC 2898 / RFC 8018 PBKDF2-HMAC.
pub(super) fn pbkdf2_hmac(
    base: &HashState,
    password: &[u8],
    salt: &[u8],
    iterations: u64,
    dklen: usize,
) -> Vec<u8> {
    let prf = PreparedHmac::new(base, password);
    let hlen = prf.digest_size;
    let blocks = dklen.div_ceil(hlen);
    let mut out = Vec::with_capacity(blocks * hlen);

    for block_idx in 1..=(blocks as u32) {
        let mut u = prf.mac_two(salt, &block_idx.to_be_bytes());
        let mut t = u.clone();
        for _ in 1..iterations {
            u = prf.mac(&u);
            for (ti, &ui) in t.iter_mut().zip(u.iter()) {
                *ti ^= ui;
            }
        }
        out.extend_from_slice(&t);
    }

    out.truncate(dklen);
    out
}

/// One-shot RFC 2104 HMAC over a built-in hash algorithm.
pub(super) fn hmac_oneshot(base: &HashState, key: &[u8], msg: &[u8]) -> Vec<u8> {
    PreparedHmac::new(base, key).mac(msg)
}
