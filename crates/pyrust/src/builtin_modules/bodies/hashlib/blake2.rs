//! Pure-Rust BLAKE2b and BLAKE2s implementations (RFC 7693 + BLAKE2 tree
//! hashing parameters matching CPython 3.12's `hashlib.blake2b`/`blake2s`).

const SIGMA: [[usize; 16]; 10] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
];

// ── BLAKE2b ──────────────────────────────────────────────────────────────────

const BLAKE2B_IV: [u64; 8] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];

#[derive(Clone, Copy)]
pub(super) struct Blake2Params<'a> {
    pub digest_size: u8,
    pub key: &'a [u8],
    pub salt: &'a [u8],
    pub person: &'a [u8],
    pub fanout: u8,
    pub depth: u8,
    pub leaf_size: u32,
    pub node_offset: u64,
    pub node_depth: u8,
    pub inner_size: u8,
    pub last_node: bool,
}

#[derive(Clone)]
pub(super) struct Blake2b {
    h: [u64; 8],
    t: [u64; 2],
    buf: [u8; 128],
    buf_len: usize,
    digest_size: usize,
    last_node: bool,
}

impl Blake2b {
    pub(super) fn new_default() -> Self {
        Self::new(Blake2Params {
            digest_size: 64,
            key: &[],
            salt: &[],
            person: &[],
            fanout: 1,
            depth: 1,
            leaf_size: 0,
            node_offset: 0,
            node_depth: 0,
            inner_size: 0,
            last_node: false,
        })
    }

    pub(super) fn new(params: Blake2Params<'_>) -> Self {
        // 64-byte parameter block (RFC 7693 Section 2.5 + BLAKE2 spec Section 2.8).
        let mut p = [0u8; 64];
        p[0] = params.digest_size;
        p[1] = params.key.len() as u8;
        p[2] = params.fanout;
        p[3] = params.depth;
        p[4..8].copy_from_slice(&params.leaf_size.to_le_bytes());
        p[8..16].copy_from_slice(&params.node_offset.to_le_bytes());
        p[16] = params.node_depth;
        p[17] = params.inner_size;
        // p[18..32] reserved (zero)
        p[32..32 + params.salt.len()].copy_from_slice(params.salt);
        p[48..48 + params.person.len()].copy_from_slice(params.person);

        let mut h = BLAKE2B_IV;
        for (i, chunk) in p.as_chunks::<8>().0.iter().enumerate() {
            h[i] ^= u64::from_le_bytes(*chunk);
        }

        let mut state = Self {
            h,
            t: [0, 0],
            buf: [0; 128],
            buf_len: 0,
            digest_size: usize::from(params.digest_size),
            last_node: params.last_node,
        };

        if !params.key.is_empty() {
            let mut key_block = [0u8; 128];
            key_block[..params.key.len()].copy_from_slice(params.key);
            state.update(&key_block);
        }
        state
    }

    pub(super) fn digest_size(&self) -> usize {
        self.digest_size
    }

    fn increment_counter(&mut self, inc: u64) {
        let (new_lo, carry) = self.t[0].overflowing_add(inc);
        self.t[0] = new_lo;
        if carry {
            self.t[1] = self.t[1].wrapping_add(1);
        }
    }

    pub(super) fn update(&mut self, mut data: &[u8]) {
        if data.is_empty() {
            return;
        }
        // Keep at least one byte buffered if possible so finalize knows the final block.
        if self.buf_len > 0 {
            let fill = 128 - self.buf_len;
            if data.len() > fill {
                self.buf[self.buf_len..128].copy_from_slice(&data[..fill]);
                self.increment_counter(128);
                let block = self.buf;
                self.compress(&block, false);
                self.buf_len = 0;
                data = &data[fill..];
            } else {
                self.buf[self.buf_len..self.buf_len + data.len()].copy_from_slice(data);
                self.buf_len += data.len();
                return;
            }
        }
        while data.len() > 128 {
            self.increment_counter(128);
            let block: [u8; 128] = data[..128].try_into().unwrap();
            self.compress(&block, false);
            data = &data[128..];
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.buf_len = data.len();
    }

    pub(super) fn finalize(&self) -> Vec<u8> {
        let mut clone = self.clone();
        clone.increment_counter(clone.buf_len as u64);
        clone.buf[clone.buf_len..128].fill(0);
        let block = clone.buf;
        clone.compress(&block, true);

        let mut full = [0u8; 64];
        for (i, word) in clone.h.iter().enumerate() {
            full[i * 8..(i + 1) * 8].copy_from_slice(&word.to_le_bytes());
        }
        full[..self.digest_size].to_vec()
    }

    fn compress(&mut self, block: &[u8; 128], is_last: bool) {
        let mut m = [0u64; 16];
        for (i, chunk) in block.as_chunks::<8>().0.iter().enumerate() {
            m[i] = u64::from_le_bytes(*chunk);
        }

        let mut v = [0u64; 16];
        v[..8].copy_from_slice(&self.h);
        v[8..16].copy_from_slice(&BLAKE2B_IV);
        v[12] ^= self.t[0];
        v[13] ^= self.t[1];
        if is_last {
            v[14] = !v[14];
            if self.last_node {
                v[15] = !v[15];
            }
        }

        #[inline(always)]
        fn g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
            v[d] = (v[d] ^ v[a]).rotate_right(32);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(24);
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
            v[d] = (v[d] ^ v[a]).rotate_right(16);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(63);
        }

        for round in 0..12 {
            let s = &SIGMA[round % 10];
            g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
            g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
        }

        for i in 0..8 {
            self.h[i] ^= v[i] ^ v[i + 8];
        }
    }
}

// ── BLAKE2s ──────────────────────────────────────────────────────────────────

const BLAKE2S_IV: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

#[derive(Clone)]
pub(super) struct Blake2s {
    h: [u32; 8],
    t: [u32; 2],
    buf: [u8; 64],
    buf_len: usize,
    digest_size: usize,
    last_node: bool,
}

impl Blake2s {
    pub(super) fn new_default() -> Self {
        Self::new(Blake2Params {
            digest_size: 32,
            key: &[],
            salt: &[],
            person: &[],
            fanout: 1,
            depth: 1,
            leaf_size: 0,
            node_offset: 0,
            node_depth: 0,
            inner_size: 0,
            last_node: false,
        })
    }

    pub(super) fn new(params: Blake2Params<'_>) -> Self {
        // 32-byte parameter block (RFC 7693 Section 2.5 + BLAKE2 spec Section 2.8).
        let mut p = [0u8; 32];
        p[0] = params.digest_size;
        p[1] = params.key.len() as u8;
        p[2] = params.fanout;
        p[3] = params.depth;
        p[4..8].copy_from_slice(&params.leaf_size.to_le_bytes());
        // 48-bit little-endian node_offset in bytes 8..14
        let no = params.node_offset.to_le_bytes();
        p[8..14].copy_from_slice(&no[..6]);
        p[14] = params.node_depth;
        p[15] = params.inner_size;
        p[16..16 + params.salt.len()].copy_from_slice(params.salt);
        p[24..24 + params.person.len()].copy_from_slice(params.person);

        let mut h = BLAKE2S_IV;
        for (i, chunk) in p.as_chunks::<4>().0.iter().enumerate() {
            h[i] ^= u32::from_le_bytes(*chunk);
        }

        let mut state = Self {
            h,
            t: [0, 0],
            buf: [0; 64],
            buf_len: 0,
            digest_size: usize::from(params.digest_size),
            last_node: params.last_node,
        };

        if !params.key.is_empty() {
            let mut key_block = [0u8; 64];
            key_block[..params.key.len()].copy_from_slice(params.key);
            state.update(&key_block);
        }
        state
    }

    pub(super) fn digest_size(&self) -> usize {
        self.digest_size
    }

    fn increment_counter(&mut self, inc: u32) {
        let (new_lo, carry) = self.t[0].overflowing_add(inc);
        self.t[0] = new_lo;
        if carry {
            self.t[1] = self.t[1].wrapping_add(1);
        }
    }

    pub(super) fn update(&mut self, mut data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if self.buf_len > 0 {
            let fill = 64 - self.buf_len;
            if data.len() > fill {
                self.buf[self.buf_len..64].copy_from_slice(&data[..fill]);
                self.increment_counter(64);
                let block = self.buf;
                self.compress(&block, false);
                self.buf_len = 0;
                data = &data[fill..];
            } else {
                self.buf[self.buf_len..self.buf_len + data.len()].copy_from_slice(data);
                self.buf_len += data.len();
                return;
            }
        }
        while data.len() > 64 {
            self.increment_counter(64);
            let block: [u8; 64] = data[..64].try_into().unwrap();
            self.compress(&block, false);
            data = &data[64..];
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.buf_len = data.len();
    }

    pub(super) fn finalize(&self) -> Vec<u8> {
        let mut clone = self.clone();
        clone.increment_counter(clone.buf_len as u32);
        clone.buf[clone.buf_len..64].fill(0);
        let block = clone.buf;
        clone.compress(&block, true);

        let mut full = [0u8; 32];
        for (i, word) in clone.h.iter().enumerate() {
            full[i * 4..(i + 1) * 4].copy_from_slice(&word.to_le_bytes());
        }
        full[..self.digest_size].to_vec()
    }

    fn compress(&mut self, block: &[u8; 64], is_last: bool) {
        let mut m = [0u32; 16];
        for (i, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            m[i] = u32::from_le_bytes(*chunk);
        }

        let mut v = [0u32; 16];
        v[..8].copy_from_slice(&self.h);
        v[8..16].copy_from_slice(&BLAKE2S_IV);
        v[12] ^= self.t[0];
        v[13] ^= self.t[1];
        if is_last {
            v[14] = !v[14];
            if self.last_node {
                v[15] = !v[15];
            }
        }

        #[inline(always)]
        fn g(v: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, x: u32, y: u32) {
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
            v[d] = (v[d] ^ v[a]).rotate_right(16);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(12);
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
            v[d] = (v[d] ^ v[a]).rotate_right(8);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(7);
        }

        for s in &SIGMA {
            g(&mut v, 0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(&mut v, 1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(&mut v, 2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(&mut v, 3, 7, 11, 15, m[s[6]], m[s[7]]);
            g(&mut v, 0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(&mut v, 1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(&mut v, 2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(&mut v, 3, 4, 9, 14, m[s[14]], m[s[15]]);
        }

        for i in 0..8 {
            self.h[i] ^= v[i] ^ v[i + 8];
        }
    }
}
