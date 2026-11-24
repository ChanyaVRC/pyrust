//! Pure-Rust Keccak-f[1600] sponge implementation (FIPS 202):
//! SHA3-224, SHA3-256, SHA3-384, SHA3-512, SHAKE128, SHAKE256.

const RC: [u64; 24] = [
    0x0000_0000_0000_0001,
    0x0000_0000_0000_8082,
    0x8000_0000_0000_808a,
    0x8000_0000_8000_8000,
    0x0000_0000_0000_808b,
    0x0000_0000_8000_0001,
    0x8000_0000_8000_8081,
    0x8000_0000_0000_8009,
    0x0000_0000_0000_008a,
    0x0000_0000_0000_0088,
    0x0000_0000_8000_8009,
    0x0000_0000_8000_000a,
    0x0000_0000_8000_808b,
    0x8000_0000_0000_008b,
    0x8000_0000_0000_8089,
    0x8000_0000_0000_8003,
    0x8000_0000_0000_8002,
    0x8000_0000_0000_0080,
    0x0000_0000_0000_800a,
    0x8000_0000_8000_000a,
    0x8000_0000_8000_8081,
    0x8000_0000_0000_8080,
    0x0000_0000_8000_0001,
    0x8000_0000_8000_8008,
];

const RHO: [u32; 24] = [
    1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44,
];

const PI: [usize; 24] = [
    10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1,
];

fn keccak_f1600(state: &mut [u64; 25]) {
    for &rc in &RC {
        // Theta
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = state[x] ^ state[x + 5] ^ state[x + 10] ^ state[x + 15] ^ state[x + 20];
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                state[x + 5 * y] ^= d;
            }
        }

        // Rho and Pi
        let mut current = state[1];
        for i in 0..24 {
            let target = PI[i];
            let temp = state[target];
            state[target] = current.rotate_left(RHO[i]);
            current = temp;
        }

        // Chi
        for y in 0..5 {
            let base = 5 * y;
            let t = [
                state[base],
                state[base + 1],
                state[base + 2],
                state[base + 3],
                state[base + 4],
            ];
            for x in 0..5 {
                state[base + x] = t[x] ^ (!t[(x + 1) % 5] & t[(x + 2) % 5]);
            }
        }

        // Iota
        state[0] ^= rc;
    }
}

#[derive(Clone, Copy)]
pub(super) enum Sha3Kind {
    Sha3_224,
    Sha3_256,
    Sha3_384,
    Sha3_512,
    Shake128,
    Shake256,
}

impl Sha3Kind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Sha3_224 => "sha3_224",
            Self::Sha3_256 => "sha3_256",
            Self::Sha3_384 => "sha3_384",
            Self::Sha3_512 => "sha3_512",
            Self::Shake128 => "shake_128",
            Self::Shake256 => "shake_256",
        }
    }

    pub(super) fn rate(self) -> usize {
        match self {
            Self::Sha3_224 => 144,
            Self::Sha3_256 => 136,
            Self::Sha3_384 => 104,
            Self::Sha3_512 => 72,
            Self::Shake128 => 168,
            Self::Shake256 => 136,
        }
    }

    pub(super) fn digest_size(self) -> usize {
        match self {
            Self::Sha3_224 => 28,
            Self::Sha3_256 => 32,
            Self::Sha3_384 => 48,
            Self::Sha3_512 => 64,
            Self::Shake128 | Self::Shake256 => 0,
        }
    }

    fn domain_sep(self) -> u8 {
        match self {
            Self::Shake128 | Self::Shake256 => 0x1f,
            _ => 0x06,
        }
    }

    pub(super) fn is_shake(self) -> bool {
        matches!(self, Self::Shake128 | Self::Shake256)
    }
}

#[derive(Clone)]
pub(super) struct KeccakSponge {
    state: [u64; 25],
    pos: usize,
    kind: Sha3Kind,
}

impl KeccakSponge {
    pub(super) fn new(kind: Sha3Kind) -> Self {
        Self {
            state: [0u64; 25],
            pos: 0,
            kind,
        }
    }

    pub(super) fn kind(&self) -> Sha3Kind {
        self.kind
    }

    #[inline]
    fn xor_byte(&mut self, idx: usize, byte: u8) {
        let lane = idx / 8;
        let shift = (idx % 8) * 8;
        self.state[lane] ^= u64::from(byte) << shift;
    }

    pub(super) fn update(&mut self, data: &[u8]) {
        let rate = self.kind.rate();
        let mut offset = 0;

        if self.pos > 0 {
            let take = (rate - self.pos).min(data.len());
            for &b in &data[..take] {
                self.xor_byte(self.pos, b);
                self.pos += 1;
            }
            offset += take;
            if self.pos == rate {
                keccak_f1600(&mut self.state);
                self.pos = 0;
            }
        }

        while offset + rate <= data.len() {
            let block = &data[offset..offset + rate];
            for (lane, chunk) in block.as_chunks::<8>().0.iter().enumerate() {
                self.state[lane] ^= u64::from_le_bytes(*chunk);
            }
            keccak_f1600(&mut self.state);
            offset += rate;
        }

        for &b in &data[offset..] {
            self.xor_byte(self.pos, b);
            self.pos += 1;
        }
    }

    pub(super) fn finalize_fixed(&self) -> Vec<u8> {
        self.finalize_xof(self.kind.digest_size())
    }

    pub(super) fn finalize_xof(&self, output_len: usize) -> Vec<u8> {
        let mut clone = self.clone();
        let rate = clone.kind.rate();
        clone.xor_byte(clone.pos, clone.kind.domain_sep());
        clone.xor_byte(rate - 1, 0x80);
        keccak_f1600(&mut clone.state);

        let mut out = Vec::with_capacity(output_len);
        let mut remaining = output_len;
        while remaining > 0 {
            let take = remaining.min(rate);
            let mut produced = 0;
            while produced < take {
                let bytes = clone.state[produced / 8].to_le_bytes();
                let chunk_len = (take - produced).min(8);
                out.extend_from_slice(&bytes[..chunk_len]);
                produced += chunk_len;
            }
            remaining -= take;
            if remaining > 0 {
                keccak_f1600(&mut clone.state);
            }
        }
        out
    }
}
