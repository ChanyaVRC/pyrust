//! Streaming RFC 1951 (DEFLATE) + RFC 1950 (zlib) + RFC 1952 (gzip) compressor.
//!
//! Supports:
//! - Levels `-1` (default), `0` (stored), and `1..=9` (LZ77 hash-chain + dynamic/fixed Huffman).
//! - Strategies: `Z_DEFAULT_STRATEGY`, `Z_FILTERED`, `Z_HUFFMAN_ONLY`, `Z_RLE`, `Z_FIXED`.
//! - Wrappers: raw (`-15..=-9`), zlib (`8..=15`), gzip (`25..=31`).
//! - Preset dictionary (`zdict`) for both raw deflate and zlib (`FDICT`).
//! - Flush modes: `Z_NO_FLUSH`, `Z_PARTIAL_FLUSH`, `Z_SYNC_FLUSH`, `Z_FULL_FLUSH`, `Z_FINISH`.
//! - Full `Clone` support for `compressobj.copy()`.

use super::checksum::{adler32, crc32};
use super::tables::{
    CODE_LEN_ORDER, FIXED_DIST_LENS, FIXED_LITLEN_LENS, build_canonical_encode_codes,
    build_length_limited_huffman, dist_to_code, length_to_code,
};

const WINDOW_SIZE: usize = 32768;
const MAX_MATCH: usize = 258;
const MIN_MATCH: usize = 3;
const BLOCK_FLUSH_SIZE: usize = 32768;
const HASH_BITS: usize = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: usize = HASH_SIZE - 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WrapperKind {
    Raw,
    Zlib,
    Gzip,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Strategy {
    Default,
    Filtered,
    HuffmanOnly,
    Rle,
    Fixed,
}

#[derive(Clone)]
struct BitWriter {
    bit_buf: u64,
    bit_count: u32,
    out: Vec<u8>,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bit_buf: 0,
            bit_count: 0,
            out: Vec::new(),
        }
    }

    #[inline]
    fn write_bits(&mut self, val: u32, bits: u32) {
        debug_assert!(bits <= 32);
        self.bit_buf |= u64::from(val) << self.bit_count;
        self.bit_count += bits;
        while self.bit_count >= 8 {
            self.out.push((self.bit_buf & 0xff) as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
    }

    fn align_byte(&mut self) {
        if self.bit_count > 0 {
            self.out.push((self.bit_buf & 0xff) as u8);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }

    fn write_bytes_aligned(&mut self, bytes: &[u8]) {
        self.align_byte();
        self.out.extend_from_slice(bytes);
    }

    fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }
}

#[derive(Clone, Copy)]
enum Token {
    Lit(u8),
    Match { len: u16, dist: u16 },
}

#[derive(Clone)]
pub(super) struct Deflater {
    wrapper: WrapperKind,
    window_bits: u8,
    level: u8,
    strategy: Strategy,
    zdict: Vec<u8>,
    header_written: bool,
    finished: bool,

    // Rolling sliding-window history (up to WINDOW_SIZE bytes preceding `pending`).
    history: Vec<u8>,
    // Uncompressed input waiting to be encoded into the next DEFLATE block.
    pending: Vec<u8>,

    // Running checksums & total uncompressed length.
    adler: u32,
    crc: u32,
    total_in: u64,

    // Persistent bit writer across streaming calls.
    bw: BitWriter,
}

impl Deflater {
    pub(super) fn new(
        level: i32,
        method: i32,
        wbits: i32,
        mem_level: i32,
        strategy: i32,
        zdict: Vec<u8>,
    ) -> Result<Self, &'static str> {
        if method != 8 {
            return Err("Invalid initialization option");
        }
        let level_u8 = match level {
            -1 => 6,
            0..=9 => level as u8,
            _ => return Err("Bad compression level"),
        };
        if !(1..=9).contains(&mem_level) {
            return Err("Invalid initialization option");
        }
        let (wrapper, window_bits) = match wbits {
            -15..=-9 => (WrapperKind::Raw, (-wbits) as u8),
            8..=15 => (WrapperKind::Zlib, (wbits.max(9)) as u8),
            25..=31 => (WrapperKind::Gzip, (wbits - 16) as u8),
            _ => return Err("Invalid initialization option"),
        };
        let strat = match strategy {
            0 => Strategy::Default,
            1 => Strategy::Filtered,
            2 => Strategy::HuffmanOnly,
            3 => Strategy::Rle,
            4 => Strategy::Fixed,
            _ => return Err("Invalid initialization option"),
        };

        let mut history = Vec::new();
        if !zdict.is_empty() && wrapper != WrapperKind::Gzip {
            let start = zdict.len().saturating_sub(WINDOW_SIZE);
            history.extend_from_slice(&zdict[start..]);
        }

        Ok(Self {
            wrapper,
            window_bits,
            level: level_u8,
            strategy: strat,
            zdict,
            header_written: false,
            finished: false,
            history,
            pending: Vec::new(),
            adler: 1,
            crc: 0,
            total_in: 0,
            bw: BitWriter::new(),
        })
    }

    pub(super) fn is_finished(&self) -> bool {
        self.finished
    }

    fn ensure_header(&mut self) {
        if self.header_written {
            return;
        }
        self.header_written = true;
        match self.wrapper {
            WrapperKind::Raw => {}
            WrapperKind::Zlib => {
                let cinfo = self.window_bits.saturating_sub(8).min(7);
                let cmf = (cinfo << 4) | 8;
                let flevel: u8 = match self.level {
                    0..=1 => 0,
                    2..=5 => 1,
                    6 => 2,
                    _ => 3,
                };
                let fdict: u8 = if !self.zdict.is_empty() { 0x20 } else { 0 };
                let mut flg = (flevel << 6) | fdict;
                let rem = ((u16::from(cmf) << 8) | u16::from(flg)) % 31;
                if rem != 0 {
                    flg += (31 - rem) as u8;
                }
                self.bw.write_bytes_aligned(&[cmf, flg]);
                if !self.zdict.is_empty() {
                    let dict_adler = adler32(&self.zdict, 1);
                    self.bw.write_bytes_aligned(&dict_adler.to_be_bytes());
                }
            }
            WrapperKind::Gzip => {
                let xfl: u8 = if self.level >= 9 {
                    2
                } else if self.level <= 1 {
                    4
                } else {
                    0
                };
                // RFC 1952 minimal 10-byte gzip header (mtime=0, OS=255 unknown).
                self.bw
                    .write_bytes_aligned(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, xfl, 0xff]);
            }
        }
    }

    pub(super) fn compress(&mut self, data: &[u8]) -> Result<Vec<u8>, &'static str> {
        if self.finished {
            return Err("Error -2 while compressing data: inconsistent stream state");
        }
        self.ensure_header();
        if data.is_empty() {
            return Ok(self.bw.take_output());
        }

        match self.wrapper {
            WrapperKind::Zlib => self.adler = adler32(data, self.adler),
            WrapperKind::Gzip => self.crc = crc32(data, self.crc),
            WrapperKind::Raw => {}
        }
        self.total_in = self.total_in.wrapping_add(data.len() as u64);
        self.pending.extend_from_slice(data);

        while self.pending.len() >= BLOCK_FLUSH_SIZE {
            let block: Vec<u8> = self.pending.drain(..BLOCK_FLUSH_SIZE).collect();
            self.emit_block(&block, false);
        }

        Ok(self.bw.take_output())
    }

    pub(super) fn flush(&mut self, mode: i32) -> Result<Vec<u8>, &'static str> {
        if self.finished {
            return Err("Error -2 while flushing: inconsistent stream state");
        }
        match mode {
            // Z_NO_FLUSH
            0 => Ok(self.bw.take_output()),
            // Z_PARTIAL_FLUSH (1), Z_SYNC_FLUSH (2), Z_FULL_FLUSH (3), Z_BLOCK (5)
            1..=3 | 5 => {
                self.ensure_header();
                if !self.pending.is_empty() {
                    let block = std::mem::take(&mut self.pending);
                    self.emit_block(&block, false);
                }
                if mode != 5 {
                    // Emit an empty non-final stored block to reach a byte boundary.
                    self.bw.write_bits(0, 3);
                    self.bw.write_bytes_aligned(&[0x00, 0x00, 0xff, 0xff]);
                }
                if mode == 3 {
                    // Z_FULL_FLUSH resets dictionary history so subsequent blocks are independent.
                    self.history.clear();
                }
                Ok(self.bw.take_output())
            }
            // Z_FINISH
            4 => {
                self.ensure_header();
                let block = std::mem::take(&mut self.pending);
                self.emit_block(&block, true);
                self.bw.align_byte();
                match self.wrapper {
                    WrapperKind::Raw => {}
                    WrapperKind::Zlib => {
                        self.bw.write_bytes_aligned(&self.adler.to_be_bytes());
                    }
                    WrapperKind::Gzip => {
                        self.bw.write_bytes_aligned(&self.crc.to_le_bytes());
                        self.bw
                            .write_bytes_aligned(&(self.total_in as u32).to_le_bytes());
                    }
                }
                self.finished = true;
                Ok(self.bw.take_output())
            }
            _ => Err("Error -2 while flushing: inconsistent stream state"),
        }
    }

    fn emit_block(&mut self, block: &[u8], is_final: bool) {
        if self.level == 0 {
            self.emit_stored_blocks(block, is_final);
            self.update_history(block);
            return;
        }
        if block.is_empty() {
            // Empty fixed Huffman block (3 header bits + 7-bit EOB symbol 256).
            self.bw
                .write_bits(if is_final { 1 } else { 0 } | (1 << 1), 3);
            self.bw.write_bits(0, 7);
            return;
        }

        let tokens = self.lz77_tokenize(block);
        self.emit_optimal_huffman_block(block, &tokens, is_final);
        self.update_history(block);
    }

    fn update_history(&mut self, block: &[u8]) {
        let max_win = 1usize << self.window_bits;
        if block.len() >= max_win {
            self.history.clear();
            self.history
                .extend_from_slice(&block[block.len() - max_win..]);
        } else {
            let total = self.history.len() + block.len();
            if total > max_win {
                let drop = total - max_win;
                self.history.drain(..drop);
            }
            self.history.extend_from_slice(block);
        }
    }

    fn emit_stored_blocks(&mut self, data: &[u8], is_final: bool) {
        if data.is_empty() {
            self.bw.write_bits(u32::from(is_final), 3);
            self.bw.write_bytes_aligned(&[0x00, 0x00, 0xff, 0xff]);
            return;
        }
        let mut offset = 0usize;
        while offset < data.len() {
            let end = (offset + 65535).min(data.len());
            let chunk = &data[offset..end];
            let final_bit = u32::from(is_final && end == data.len());
            self.bw.write_bits(final_bit, 3);
            let len = chunk.len() as u16;
            let nlen = !len;
            self.bw.write_bytes_aligned(&len.to_le_bytes());
            self.bw.write_bytes_aligned(&nlen.to_le_bytes());
            self.bw.write_bytes_aligned(chunk);
            offset = end;
        }
    }

    fn lz77_tokenize(&self, block: &[u8]) -> Vec<Token> {
        if self.strategy == Strategy::HuffmanOnly {
            return block.iter().copied().map(Token::Lit).collect();
        }

        let max_dist = (1usize << self.window_bits).min(WINDOW_SIZE);
        let hist_len = self.history.len().min(max_dist);
        let hist_start = self.history.len() - hist_len;

        // Concatenate history + current block for uniform, bounds-checked match scanning.
        let mut buf = Vec::with_capacity(hist_len + block.len());
        buf.extend_from_slice(&self.history[hist_start..]);
        buf.extend_from_slice(block);
        let total = buf.len();

        let mut tokens = Vec::with_capacity(block.len() / 2 + 16);

        if self.strategy == Strategy::Rle {
            let mut pos = hist_len;
            while pos < total {
                if pos > 0 && buf[pos] == buf[pos - 1] {
                    let target = buf[pos];
                    let mut run = 1usize;
                    while run < MAX_MATCH && pos + run < total && buf[pos + run] == target {
                        run += 1;
                    }
                    if run >= MIN_MATCH {
                        tokens.push(Token::Match {
                            len: run as u16,
                            dist: 1,
                        });
                        pos += run;
                        continue;
                    }
                }
                tokens.push(Token::Lit(buf[pos]));
                pos += 1;
            }
            return tokens;
        }

        let (max_chain, nice_len, use_lazy): (usize, usize, bool) = match self.level {
            1 => (4, 16, false),
            2 => (8, 24, false),
            3 => (16, 32, false),
            4 => (16, 48, true),
            5 => (32, 64, true),
            6 => (48, 128, true),
            7 => (64, 160, true),
            8 => (128, 258, true),
            _ => (256, 258, true),
        };

        let mut head = vec![u32::MAX; HASH_SIZE];
        let mut prev = vec![u32::MAX; total];

        #[inline]
        fn hash3(b: &[u8], i: usize) -> usize {
            ((usize::from(b[i]) << 10) ^ (usize::from(b[i + 1]) << 5) ^ usize::from(b[i + 2]))
                & HASH_MASK
        }

        // Seed hash table with history bytes.
        if hist_len >= MIN_MATCH {
            let seed_end = (hist_len + block.len()).saturating_sub(2).min(hist_len);
            for (i, slot) in prev.iter_mut().enumerate().take(seed_end) {
                let h = hash3(&buf, i);
                *slot = head[h];
                head[h] = i as u32;
            }
        }

        let find_best = |pos: usize, head: &[u32], prev: &[u32]| -> (usize, usize) {
            if pos + MIN_MATCH > total {
                return (0, 0);
            }
            let h = hash3(&buf, pos);
            let mut cur = head[h];
            let mut best_len = 0usize;
            let mut best_dist = 0usize;
            let mut chain = max_chain;
            let max_len = MAX_MATCH.min(total - pos);

            while cur != u32::MAX && chain > 0 {
                let c = cur as usize;
                let dist = pos - c;
                if dist > max_dist {
                    break;
                }
                if buf[c + best_len] == buf[pos + best_len] && buf[c] == buf[pos] {
                    let mut l = 0usize;
                    while l < max_len && buf[c + l] == buf[pos + l] {
                        l += 1;
                    }
                    if l > best_len {
                        best_len = l;
                        best_dist = dist;
                        if best_len >= nice_len || best_len == max_len {
                            break;
                        }
                    }
                }
                cur = prev[c];
                chain -= 1;
            }
            (best_len, best_dist)
        };

        let mut pos = hist_len;
        while pos < total {
            let (mut best_len, mut best_dist) = find_best(pos, &head, &prev);

            if pos + MIN_MATCH <= total {
                let h = hash3(&buf, pos);
                prev[pos] = head[h];
                head[h] = pos as u32;
            }

            if best_len >= MIN_MATCH {
                // Optional lazy match check at pos + 1.
                if use_lazy && best_len < nice_len && pos + 1 + MIN_MATCH <= total {
                    let (next_len, next_dist) = find_best(pos + 1, &head, &prev);
                    if next_len > best_len + 1 {
                        tokens.push(Token::Lit(buf[pos]));
                        pos += 1;
                        let h = hash3(&buf, pos);
                        prev[pos] = head[h];
                        head[h] = pos as u32;
                        best_len = next_len;
                        best_dist = next_dist;
                    }
                }

                tokens.push(Token::Match {
                    len: best_len as u16,
                    dist: best_dist as u16,
                });

                // Insert skipped positions into the hash chain.
                let end = (pos + best_len).min(total.saturating_sub(2));
                for (p, slot) in prev.iter_mut().enumerate().take(end).skip(pos + 1) {
                    let h = hash3(&buf, p);
                    *slot = head[h];
                    head[h] = p as u32;
                }
                pos += best_len;
            } else {
                tokens.push(Token::Lit(buf[pos]));
                pos += 1;
            }
        }

        tokens
    }

    fn emit_optimal_huffman_block(&mut self, raw: &[u8], tokens: &[Token], is_final: bool) {
        // Count symbol frequencies.
        let mut lit_freqs = [0u32; 286];
        let mut dist_freqs = [0u32; 30];
        let mut extra_bits_total: usize = 0;

        for &tok in tokens {
            match tok {
                Token::Lit(b) => lit_freqs[usize::from(b)] += 1,
                Token::Match { len, dist } => {
                    let (lsym, lextra, _) = length_to_code(len);
                    let (dsym, dextra, _) = dist_to_code(dist);
                    lit_freqs[lsym] += 1;
                    dist_freqs[dsym] += 1;
                    extra_bits_total += usize::from(lextra) + usize::from(dextra);
                }
            }
        }
        lit_freqs[256] = 1; // End-of-block symbol.

        // Compute Fixed Huffman bit cost.
        let fixed_bits: usize = 3
            + extra_bits_total
            + lit_freqs
                .iter()
                .enumerate()
                .map(|(s, &f)| f as usize * usize::from(FIXED_LITLEN_LENS[s]))
                .sum::<usize>()
            + dist_freqs
                .iter()
                .enumerate()
                .map(|(s, &f)| f as usize * usize::from(FIXED_DIST_LENS[s]))
                .sum::<usize>();

        // Compute Stored block bit cost (only if raw fits in 1 stored block and strategy != Fixed).
        let stored_bits: usize = if raw.len() <= 65535 && self.strategy != Strategy::Fixed {
            let align_pad = (8 - ((self.bw.bit_count + 3) & 7)) & 7;
            3 + align_pad as usize + 32 + raw.len() * 8
        } else {
            usize::MAX
        };

        // Try Dynamic Huffman if strategy != Fixed and block is large enough to benefit.
        let dyn_plan = if self.strategy != Strategy::Fixed && tokens.len() >= 32 {
            Some(build_dynamic_plan(
                &lit_freqs,
                &dist_freqs,
                extra_bits_total,
            ))
        } else {
            None
        };
        let dyn_bits = dyn_plan.as_ref().map_or(usize::MAX, |p| p.total_bits);

        if stored_bits < fixed_bits && stored_bits < dyn_bits {
            self.emit_stored_blocks(raw, is_final);
        } else if let Some(plan) = dyn_plan
            && dyn_bits < fixed_bits
        {
            self.emit_dynamic_block(tokens, &plan, is_final);
        } else {
            self.emit_fixed_block(tokens, is_final);
        }
    }

    fn emit_fixed_block(&mut self, tokens: &[Token], is_final: bool) {
        let lit_codes = build_canonical_encode_codes(&FIXED_LITLEN_LENS);
        let dist_codes = build_canonical_encode_codes(&FIXED_DIST_LENS);

        self.bw.write_bits(u32::from(is_final) | (1 << 1), 3);
        self.write_tokens(
            tokens,
            &lit_codes,
            &FIXED_LITLEN_LENS,
            &dist_codes,
            &FIXED_DIST_LENS,
        );
    }

    fn emit_dynamic_block(&mut self, tokens: &[Token], plan: &DynamicPlan, is_final: bool) {
        let lit_codes = build_canonical_encode_codes(&plan.lit_lens);
        let dist_codes = build_canonical_encode_codes(&plan.dist_lens);
        let cl_codes = build_canonical_encode_codes(&plan.cl_lens);

        self.bw.write_bits(u32::from(is_final) | (2 << 1), 3);
        self.bw.write_bits((plan.hlit - 257) as u32, 5);
        self.bw.write_bits((plan.hdist - 1) as u32, 5);
        self.bw.write_bits((plan.hclen - 4) as u32, 4);

        for &sym in &CODE_LEN_ORDER[..plan.hclen] {
            self.bw.write_bits(u32::from(plan.cl_lens[sym]), 3);
        }

        for &(sym, extra_bits, extra_val) in &plan.rle_ops {
            let s = sym as usize;
            self.bw
                .write_bits(u32::from(cl_codes[s]), u32::from(plan.cl_lens[s]));
            if extra_bits > 0 {
                self.bw
                    .write_bits(u32::from(extra_val), u32::from(extra_bits));
            }
        }

        self.write_tokens(
            tokens,
            &lit_codes,
            &plan.lit_lens,
            &dist_codes,
            &plan.dist_lens,
        );
    }

    fn write_tokens(
        &mut self,
        tokens: &[Token],
        lit_codes: &[u16],
        lit_lens: &[u8],
        dist_codes: &[u16],
        dist_lens: &[u8],
    ) {
        for &tok in tokens {
            match tok {
                Token::Lit(b) => {
                    let s = usize::from(b);
                    self.bw
                        .write_bits(u32::from(lit_codes[s]), u32::from(lit_lens[s]));
                }
                Token::Match { len, dist } => {
                    let (lsym, lextra, lval) = length_to_code(len);
                    self.bw
                        .write_bits(u32::from(lit_codes[lsym]), u32::from(lit_lens[lsym]));
                    if lextra > 0 {
                        self.bw.write_bits(u32::from(lval), u32::from(lextra));
                    }
                    let (dsym, dextra, dval) = dist_to_code(dist);
                    self.bw
                        .write_bits(u32::from(dist_codes[dsym]), u32::from(dist_lens[dsym]));
                    if dextra > 0 {
                        self.bw.write_bits(u32::from(dval), u32::from(dextra));
                    }
                }
            }
        }
        // EOB symbol 256.
        self.bw
            .write_bits(u32::from(lit_codes[256]), u32::from(lit_lens[256]));
    }
}

struct DynamicPlan {
    hlit: usize,
    hdist: usize,
    hclen: usize,
    lit_lens: Vec<u8>,
    dist_lens: Vec<u8>,
    cl_lens: Vec<u8>,
    rle_ops: Vec<(u8, u8, u8)>,
    total_bits: usize,
}

fn build_dynamic_plan(
    lit_freqs: &[u32; 286],
    dist_freqs: &[u32; 30],
    extra_bits_total: usize,
) -> DynamicPlan {
    let lit_lens = build_length_limited_huffman(lit_freqs, 15);
    let mut dist_lens = build_length_limited_huffman(dist_freqs, 15);
    if dist_lens.iter().all(|&l| l == 0) {
        // RFC 1951 requires at least 1 distance code entry.
        dist_lens[0] = 1;
    }

    let mut hlit = 286usize;
    while hlit > 257 && lit_lens[hlit - 1] == 0 {
        hlit -= 1;
    }
    let mut hdist = 30usize;
    while hdist > 1 && dist_lens[hdist - 1] == 0 {
        hdist -= 1;
    }

    let mut combined = Vec::with_capacity(hlit + hdist);
    combined.extend_from_slice(&lit_lens[..hlit]);
    combined.extend_from_slice(&dist_lens[..hdist]);

    // RLE-encode combined code lengths.
    let mut rle_ops: Vec<(u8, u8, u8)> = Vec::new();
    let mut cl_freqs = [0u32; 19];
    let mut i = 0usize;
    while i < combined.len() {
        let v = combined[i];
        let mut run = 1usize;
        while i + run < combined.len() && combined[i + run] == v {
            run += 1;
        }
        if v == 0 {
            let mut rem = run;
            while rem >= 11 {
                let take = rem.min(138);
                rle_ops.push((18, 7, (take - 11) as u8));
                cl_freqs[18] += 1;
                rem -= take;
            }
            while rem >= 3 {
                let take = rem.min(10);
                rle_ops.push((17, 3, (take - 3) as u8));
                cl_freqs[17] += 1;
                rem -= take;
            }
            while rem > 0 {
                rle_ops.push((0, 0, 0));
                cl_freqs[0] += 1;
                rem -= 1;
            }
        } else {
            rle_ops.push((v, 0, 0));
            cl_freqs[usize::from(v)] += 1;
            let mut rem = run - 1;
            while rem >= 3 {
                let take = rem.min(6);
                rle_ops.push((16, 2, (take - 3) as u8));
                cl_freqs[16] += 1;
                rem -= take;
            }
            while rem > 0 {
                rle_ops.push((v, 0, 0));
                cl_freqs[usize::from(v)] += 1;
                rem -= 1;
            }
        }
        i += run;
    }

    let cl_lens = build_length_limited_huffman(&cl_freqs, 7);
    let mut hclen = 19usize;
    while hclen > 4 && cl_lens[CODE_LEN_ORDER[hclen - 1]] == 0 {
        hclen -= 1;
    }

    let mut total_bits = 3 + 5 + 5 + 4 + hclen * 3;
    for &(sym, extra_bits, _) in &rle_ops {
        total_bits += usize::from(cl_lens[usize::from(sym)]) + usize::from(extra_bits);
    }
    total_bits += extra_bits_total;
    for (s, &f) in lit_freqs.iter().enumerate() {
        total_bits += f as usize * usize::from(lit_lens[s]);
    }
    for (s, &f) in dist_freqs.iter().enumerate() {
        total_bits += f as usize * usize::from(dist_lens[s]);
    }

    DynamicPlan {
        hlit,
        hdist,
        hclen,
        lit_lens,
        dist_lens,
        cl_lens,
        rle_ops,
        total_bits,
    }
}
