//! Pure-Rust CRC-32 (IEEE 802.3) and Adler-32 (RFC 1950) checksums.

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut c = i;
        let mut j = 0;
        while j < 8 {
            if (c & 1) != 0 {
                c = 0xedb8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            j += 1;
        }
        table[i as usize] = c;
        i += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

/// Compute incremental IEEE 802.3 CRC-32 (matching `zlib.crc32`).
pub(super) fn crc32(data: &[u8], init: u32) -> u32 {
    let mut crc = !init;
    for &b in data {
        crc = CRC32_TABLE[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Compute incremental RFC 1950 Adler-32 (matching `zlib.adler32`).
pub(super) fn adler32(data: &[u8], init: u32) -> u32 {
    const MOD_ADLER: u32 = 65521;
    // Largest n such that 255*n*(n+1)/2 + (n+1)*(MOD_ADLER-1) <= 2^32 - 1.
    const NMAX: usize = 5552;

    let mut s1 = init & 0xffff;
    let mut s2 = (init >> 16) & 0xffff;

    for chunk in data.chunks(NMAX) {
        for &b in chunk {
            s1 += u32::from(b);
            s2 += s1;
        }
        s1 %= MOD_ADLER;
        s2 %= MOD_ADLER;
    }

    (s2 << 16) | s1
}
