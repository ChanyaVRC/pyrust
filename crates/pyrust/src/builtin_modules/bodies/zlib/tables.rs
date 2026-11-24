//! RFC 1951 DEFLATE static tables and canonical Huffman coding primitives.

/// Base lengths for literal/length symbols 257..=285.
pub(super) const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];

/// Extra bits for literal/length symbols 257..=285.
pub(super) const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// Base distances for distance symbols 0..=29.
pub(super) const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

/// Extra bits for distance symbols 0..=29.
pub(super) const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Order in which dynamic-Huffman code-length code lengths are stored in the header.
pub(super) const CODE_LEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Fixed Huffman literal/length bit lengths (RFC 1951 section 3.2.6).
pub(super) const fn fixed_litlen_lengths() -> [u8; 288] {
    let mut lens = [0u8; 288];
    let mut i = 0;
    while i <= 143 {
        lens[i] = 8;
        i += 1;
    }
    while i <= 255 {
        lens[i] = 9;
        i += 1;
    }
    while i <= 279 {
        lens[i] = 7;
        i += 1;
    }
    while i <= 287 {
        lens[i] = 8;
        i += 1;
    }
    lens
}

/// Fixed Huffman distance bit lengths (RFC 1951 section 3.2.6).
pub(super) const fn fixed_dist_lengths() -> [u8; 32] {
    [5u8; 32]
}

pub(super) const FIXED_LITLEN_LENS: [u8; 288] = fixed_litlen_lengths();
pub(super) const FIXED_DIST_LENS: [u8; 32] = fixed_dist_lengths();

/// Reverse the lowest `bits` bits of `code`.
#[inline]
pub(super) fn reverse_bits(mut code: u16, bits: u8) -> u16 {
    let mut out = 0u16;
    for _ in 0..bits {
        out = (out << 1) | (code & 1);
        code >>= 1;
    }
    out
}

/// Map match length (3..=258) to `(symbol 257..=285, extra_bits, extra_value)`.
#[inline]
pub(super) fn length_to_code(len: u16) -> (usize, u8, u16) {
    debug_assert!((3..=258).contains(&len));
    if len == 258 {
        return (285, 0, 0);
    }
    // Linear/binary scan over 28 entries — tiny and branch-friendly.
    let mut lo = 0usize;
    let mut hi = 27usize;
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if LENGTH_BASE[mid] <= len {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    (257 + lo, LENGTH_EXTRA[lo], len - LENGTH_BASE[lo])
}

/// Map match distance (1..=32768) to `(symbol 0..=29, extra_bits, extra_value)`.
#[inline]
pub(super) fn dist_to_code(dist: u16) -> (usize, u8, u16) {
    debug_assert!((1..=32768).contains(&dist));
    let mut lo = 0usize;
    let mut hi = 29usize;
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if DIST_BASE[mid] <= dist {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    (lo, DIST_EXTRA[lo], dist - DIST_BASE[lo])
}

/// Canonical Huffman decoder supporting bit lengths up to 15.
///
/// Uses a fast 9-bit lookup table when enough bits are buffered, and falls back
/// to exact bit-by-bit canonical decoding otherwise (guaranteeing we never read
/// past the end of a DEFLATE stream).
#[derive(Clone)]
pub(super) struct HuffmanDecoder {
    /// Count of codes of each bit length `1..=15`.
    counts: [u16; 16],
    /// Symbols sorted by `(code_length, symbol_value)`.
    symbols: Vec<u16>,
    /// Maximum bit length present (0 if empty).
    max_bits: u8,
    /// Fast lookup table for codes up to 9 bits: `(symbol << 4) | bit_len`, or `0` if invalid / > 9 bits.
    fast: [u16; 512],
}

impl HuffmanDecoder {
    pub(super) fn from_lengths(lengths: &[u8], max_allowed_bits: u8) -> Result<Self, &'static str> {
        let mut counts = [0u16; 16];
        let mut max_bits = 0u8;
        let mut total_syms = 0usize;

        for &len in lengths {
            if len > max_allowed_bits || len > 15 {
                return Err("invalid code lengths set");
            }
            if len > 0 {
                counts[len as usize] += 1;
                if len > max_bits {
                    max_bits = len;
                }
                total_syms += 1;
            }
        }

        if total_syms == 0 {
            return Ok(Self {
                counts,
                symbols: Vec::new(),
                max_bits: 0,
                fast: [0u16; 512],
            });
        }

        // Check for oversubscribed tree.
        let mut left: i32 = 1;
        for &cnt in &counts[1..=max_bits as usize] {
            left = (left << 1) - i32::from(cnt);
            if left < 0 {
                return Err("invalid code lengths set");
            }
        }

        let mut offsets = [0usize; 16];
        let mut sum = 0usize;
        for bits in 1..=15 {
            offsets[bits] = sum;
            sum += counts[bits] as usize;
        }

        let mut symbols = vec![0u16; total_syms];
        for (sym, &len) in lengths.iter().enumerate() {
            if len > 0 {
                let idx = offsets[len as usize];
                symbols[idx] = sym as u16;
                offsets[len as usize] = idx + 1;
            }
        }

        // Populate 9-bit fast lookup table.
        let mut fast = [0u16; 512];
        let mut code = 0u32;
        let mut sym_idx = 0usize;
        for bits in 1..=max_bits.min(9) {
            let cnt = counts[bits as usize] as usize;
            let step = 1usize << bits;
            for _ in 0..cnt {
                let sym = symbols[sym_idx];
                sym_idx += 1;
                let rev = reverse_bits(code as u16, bits) as usize;
                let entry = (sym << 4) | u16::from(bits);
                let mut idx = rev;
                while idx < 512 {
                    fast[idx] = entry;
                    idx += step;
                }
                code += 1;
            }
            code <<= 1;
        }

        Ok(Self {
            counts,
            symbols,
            max_bits,
            fast,
        })
    }

    pub(super) fn fixed_litlen() -> Self {
        Self::from_lengths(&FIXED_LITLEN_LENS, 15).expect("fixed litlen table is valid")
    }

    pub(super) fn fixed_dist() -> Self {
        Self::from_lengths(&FIXED_DIST_LENS, 15).expect("fixed dist table is valid")
    }

    /// Try fast lookup from the lowest 9 bits of `bit_buf`.
    /// Returns `Some((symbol, bit_len))` if the lowest bits match a code of length <= `min(9, bit_count)`.
    #[inline]
    pub(super) fn fast_decode(&self, bit_buf: u64, bit_count: u32) -> Option<(u16, u8)> {
        if bit_count == 0 {
            return None;
        }
        let entry = self.fast[(bit_buf & 0x1ff) as usize];
        let len = (entry & 0x0f) as u8;
        if len != 0 && u32::from(len) <= bit_count {
            Some((entry >> 4, len))
        } else {
            None
        }
    }

    /// Exact canonical decode using bits already in `bit_buf` (`bit_count` bits available).
    /// Returns `Ok(Some((symbol, bit_len)))` on match, `Ok(None)` if more bits are needed,
    /// or `Err("invalid literal/lengths set")` if the bit pattern is definitely invalid.
    pub(super) fn slow_decode(
        &self,
        bit_buf: u64,
        bit_count: u32,
    ) -> Result<Option<(u16, u8)>, &'static str> {
        if self.max_bits == 0 {
            return Err("invalid code");
        }
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: usize = 0;

        for len in 1..=self.max_bits {
            if u32::from(len) > bit_count {
                return Ok(None);
            }
            let bit = ((bit_buf >> (len - 1)) & 1) as i32;
            code |= bit;
            let count = i32::from(self.counts[len as usize]);
            if code - count < first {
                let sym = self.symbols[index + (code - first) as usize];
                return Ok(Some((sym, len)));
            }
            index += count as usize;
            first = (first + count) << 1;
            code <<= 1;
        }

        Err("invalid code")
    }
}

/// Build bit-reversed canonical Huffman codes from bit lengths (ready to write LSB-first).
pub(super) fn build_canonical_encode_codes(lengths: &[u8]) -> Vec<u16> {
    let mut bl_count = [0u16; 16];
    for &len in lengths {
        if len > 0 {
            bl_count[len as usize] += 1;
        }
    }
    let mut next_code = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..=15 {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }

    let mut codes = vec![0u16; lengths.len()];
    for (sym, &len) in lengths.iter().enumerate() {
        if len > 0 {
            let c = next_code[len as usize];
            next_code[len as usize] = c + 1;
            codes[sym] = reverse_bits(c, len);
        }
    }
    codes
}

/// Package-Merge algorithm: compute optimal length-limited Huffman bit lengths
/// in `1..=max_bits` for all symbols with `freq > 0` (0 for symbols with `freq == 0`).
pub(super) fn build_length_limited_huffman(freqs: &[u32], max_bits: usize) -> Vec<u8> {
    let mut lengths = vec![0u8; freqs.len()];
    let mut active: Vec<(u64, usize)> = freqs
        .iter()
        .enumerate()
        .filter_map(|(sym, &f)| {
            if f > 0 {
                Some((u64::from(f), sym))
            } else {
                None
            }
        })
        .collect();

    if active.is_empty() {
        return lengths;
    }
    if active.len() == 1 {
        lengths[active[0].1] = 1;
        return lengths;
    }

    // Sort active symbols by ascending frequency (tie-break by symbol index).
    active.sort_unstable();

    // Node representation for package-merge:
    // Each package at level `l` is either a Leaf(sym) or a Pair(left_idx, right_idx)
    // referencing indices into `levels[l - 1]`.
    #[derive(Clone, Copy)]
    enum PkgKind {
        Leaf(usize),
        Pair(usize, usize),
    }

    let n = active.len();
    let mut levels: Vec<Vec<(u64, PkgKind)>> = Vec::with_capacity(max_bits);

    for level in 0..max_bits {
        let mut cur: Vec<(u64, PkgKind)> = Vec::with_capacity(n * 2);
        for &(w, sym) in &active {
            cur.push((w, PkgKind::Leaf(sym)));
        }
        if level > 0 {
            let prev = &levels[level - 1];
            let pairs = prev.len() / 2;
            for i in 0..pairs {
                let w = prev[2 * i].0.saturating_add(prev[2 * i + 1].0);
                cur.push((w, PkgKind::Pair(2 * i, 2 * i + 1)));
            }
        }
        // Sort by ascending weight; keep leaves before equal-weight pairs for determinism.
        cur.sort_by_key(|&(w, _)| w);
        levels.push(cur);
    }

    // Select the first `2 * n - 2` packages at the top level and trace down.
    let mut active_indices: Vec<usize> = (0..(2 * n - 2)).collect();
    for level in (0..max_bits).rev() {
        let mut next_indices = Vec::new();
        let cur = &levels[level];
        for idx in active_indices {
            match cur[idx].1 {
                PkgKind::Leaf(sym) => {
                    lengths[sym] += 1;
                }
                PkgKind::Pair(a, b) => {
                    next_indices.push(a);
                    next_indices.push(b);
                }
            }
        }
        active_indices = next_indices;
    }

    lengths
}
