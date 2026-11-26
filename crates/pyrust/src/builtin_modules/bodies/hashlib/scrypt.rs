//! scrypt (RFC 7914) built on the in-crate PBKDF2-HMAC-SHA256.
//!
//! Parameter validation mirrors OpenSSL's `EVP_PBE_scrypt` (the backend of
//! CPython's `hashlib.scrypt`): [`validate`] reproduces the order of its
//! sanity checks and distinguishes "invalid parameter combination" (no
//! OpenSSL error queued) from "memory limit exceeded" (queued).

use super::engine::{HashState, pbkdf2_hmac};

/// OpenSSL's default `maxmem` when the caller passes `0` (32 MiB).
const SCRYPT_MAX_MEM: u64 = 32 * 1024 * 1024;
/// `SCRYPT_PR_MAX` in OpenSSL: the bound on `p * r`.
const SCRYPT_PR_MAX: u64 = (1 << 30) - 1;

/// Why OpenSSL rejected a parameter set.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ScryptParamError {
    /// Rejected without an OpenSSL error being queued; CPython falls back to
    /// its own "Invalid parameter combination" text.
    Invalid,
    /// `EVP_R_MEMORY_LIMIT_EXCEEDED`.
    MemoryLimit,
}

/// Reproduce `scrypt_alg()`'s parameter checks for a `NULL` output buffer.
///
/// `r` and `p` arrive as `u64` exactly like OpenSSL's prototype, so a negative
/// C `int` has already been sign-extended by the caller.
pub(super) fn validate(n: u64, r: u64, p: u64, maxmem: u64) -> Result<(), ScryptParamError> {
    if r == 0 || p == 0 || n < 2 || n & (n - 1) != 0 {
        return Err(ScryptParamError::Invalid);
    }
    if p > SCRYPT_PR_MAX / r {
        return Err(ScryptParamError::MemoryLimit);
    }
    if 16 * u128::from(r) <= 63 && u128::from(n) >= (1u128 << (16 * r)) {
        return Err(ScryptParamError::MemoryLimit);
    }
    let blen = u128::from(p) * 128 * u128::from(r);
    if blen > i32::MAX as u128 {
        return Err(ScryptParamError::MemoryLimit);
    }
    let vlen = 32u128 * u128::from(r) * (u128::from(n) + 2) * 4;
    if vlen > u128::from(u64::MAX) {
        return Err(ScryptParamError::MemoryLimit);
    }
    let limit = if maxmem == 0 { SCRYPT_MAX_MEM } else { maxmem };
    if blen + vlen > u128::from(limit) {
        return Err(ScryptParamError::MemoryLimit);
    }
    Ok(())
}

/// Derive `dklen` bytes. The parameters must already have passed [`validate`].
pub(super) fn scrypt(
    password: &[u8],
    salt: &[u8],
    n: u64,
    r: u64,
    p: u64,
    dklen: usize,
) -> Vec<u8> {
    let sha256 = HashState::from_name("sha256").expect("sha256 hash algorithm must exist");
    let (n, r, p) = (n as usize, r as usize, p as usize);
    let block_len = 128 * r;
    let mut blocks = pbkdf2_hmac(&sha256, password, salt, 1, block_len * p);
    for chunk in blocks.chunks_mut(block_len) {
        ro_mix(chunk, n, r);
    }
    pbkdf2_hmac(&sha256, password, &blocks, 1, dklen)
}

fn ro_mix(block: &mut [u8], n: usize, r: usize) {
    let words = 32 * r;
    let mut x: Vec<u32> = block
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let mut v = vec![0u32; n * words];
    let mut scratch = vec![0u32; words];
    for i in 0..n {
        v[i * words..(i + 1) * words].copy_from_slice(&x);
        block_mix(&mut x, &mut scratch, r);
    }
    for _ in 0..n {
        let j = (x[(2 * r - 1) * 16] as usize) & (n - 1);
        for (xk, vk) in x.iter_mut().zip(&v[j * words..(j + 1) * words]) {
            *xk ^= *vk;
        }
        block_mix(&mut x, &mut scratch, r);
    }
    for (chunk, word) in block.as_chunks_mut::<4>().0.iter_mut().zip(&x) {
        *chunk = word.to_le_bytes();
    }
}

fn block_mix(b: &mut [u32], y: &mut [u32], r: usize) {
    let mut x = [0u32; 16];
    x.copy_from_slice(&b[(2 * r - 1) * 16..2 * r * 16]);
    for i in 0..2 * r {
        for (xk, bk) in x.iter_mut().zip(&b[i * 16..(i + 1) * 16]) {
            *xk ^= *bk;
        }
        salsa20_8(&mut x);
        y[i * 16..(i + 1) * 16].copy_from_slice(&x);
    }
    for i in 0..r {
        b[i * 16..(i + 1) * 16].copy_from_slice(&y[2 * i * 16..(2 * i + 1) * 16]);
        b[(r + i) * 16..(r + i + 1) * 16].copy_from_slice(&y[(2 * i + 1) * 16..(2 * i + 2) * 16]);
    }
}

fn salsa20_8(b: &mut [u32; 16]) {
    let mut x = *b;
    macro_rules! qr {
        ($a:expr, $b:expr, $c:expr, $d:expr) => {
            x[$b] ^= x[$a].wrapping_add(x[$d]).rotate_left(7);
            x[$c] ^= x[$b].wrapping_add(x[$a]).rotate_left(9);
            x[$d] ^= x[$c].wrapping_add(x[$b]).rotate_left(13);
            x[$a] ^= x[$d].wrapping_add(x[$c]).rotate_left(18);
        };
    }
    for _ in 0..4 {
        // Column round.
        qr!(0, 4, 8, 12);
        qr!(5, 9, 13, 1);
        qr!(10, 14, 2, 6);
        qr!(15, 3, 7, 11);
        // Row round.
        qr!(0, 1, 2, 3);
        qr!(5, 6, 7, 4);
        qr!(10, 11, 8, 9);
        qr!(15, 12, 13, 14);
    }
    for (bk, xk) in b.iter_mut().zip(x) {
        *bk = bk.wrapping_add(xk);
    }
}

#[cfg(test)]
mod tests {
    use super::super::engine::hex_encode;
    use super::{ScryptParamError, scrypt, validate};

    #[test]
    fn rfc7914_vectors() {
        assert_eq!(
            hex_encode(&scrypt(b"", b"", 16, 1, 1, 64)),
            "77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906"
        );
        assert_eq!(
            hex_encode(&scrypt(b"password", b"NaCl", 1024, 8, 16, 64)),
            "fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b3731622eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640"
        );
    }

    #[test]
    fn parameter_validation_matches_openssl() {
        assert_eq!(validate(16, 1, 1, 0), Ok(()));
        assert_eq!(validate(16, 0, 1, 0), Err(ScryptParamError::Invalid));
        assert_eq!(validate(16, 1, 0, 0), Err(ScryptParamError::Invalid));
        assert_eq!(validate(15, 1, 1, 0), Err(ScryptParamError::Invalid));
        assert_eq!(validate(16, 1, 1, 1), Err(ScryptParamError::MemoryLimit));
        assert_eq!(
            validate(1 << 20, 8, 1, 0),
            Err(ScryptParamError::MemoryLimit)
        );
        // N >= 2^(16 * r) is rejected for r == 1.
        assert_eq!(
            validate(1 << 16, 1, 1, 1 << 30),
            Err(ScryptParamError::MemoryLimit)
        );
    }
}
