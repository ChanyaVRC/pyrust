//! Resumable bit-level RFC 1951 (DEFLATE) + RFC 1950 (zlib) + RFC 1952 (gzip) decompressor.

use super::checksum::{adler32, crc32};
use super::tables::{
    CODE_LEN_ORDER, DIST_BASE, DIST_EXTRA, HuffmanDecoder, LENGTH_BASE, LENGTH_EXTRA,
};

const WINDOW_SIZE: usize = 32768;
const WINDOW_MASK: usize = WINDOW_SIZE - 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum WrapperKind {
    Raw,
    Zlib,
    Gzip,
    Auto,
}

#[derive(Clone)]
enum State {
    AutoDetect,
    ZlibHeader {
        buf: [u8; 2],
        have: usize,
    },
    ZlibDict {
        buf: [u8; 4],
        have: usize,
    },
    GzipFixedHeader {
        buf: [u8; 10],
        have: usize,
    },
    GzipExtraLen {
        flags: u8,
        buf: [u8; 2],
        have: usize,
    },
    GzipExtraData {
        flags: u8,
        rem: usize,
    },
    GzipName {
        flags: u8,
    },
    GzipComment {
        flags: u8,
    },
    GzipHcrc {
        buf: [u8; 2],
        have: usize,
    },
    BlockHeader,
    StoredLen {
        buf: [u8; 4],
        have: usize,
    },
    StoredCopy {
        rem: usize,
    },
    DynHeader,
    DynCodeLens {
        hlit: usize,
        hdist: usize,
        hclen: usize,
        idx: usize,
        cl_lens: [u8; 19],
    },
    DynTrees {
        hlit: usize,
        hdist: usize,
        cl_dec: Box<HuffmanDecoder>,
        lens: Vec<u8>,
        pending_repeat: Option<u8>,
    },
    DecodeSym,
    DecodeLenExtra {
        base: u16,
        extra: u8,
    },
    DecodeDist {
        len: u16,
    },
    DecodeDistExtra {
        len: u16,
        base: u16,
        extra: u8,
    },
    CopyMatch {
        dist: usize,
        rem: u16,
    },
    ZlibTrailer {
        buf: [u8; 4],
        have: usize,
    },
    GzipTrailer {
        buf: [u8; 8],
        have: usize,
    },
    Done,
}

#[derive(Clone)]
pub(super) struct Inflater {
    active_wrapper: WrapperKind,
    max_window_bits: u8,
    state: State,
    bfinal: bool,

    // Bit buffer: LSB-first.
    bit_buf: u64,
    bit_count: u32,

    // Active block Huffman decoders.
    litlen_dec: HuffmanDecoder,
    dist_dec: HuffmanDecoder,

    // 32 KiB circular sliding window.
    window: Vec<u8>,
    win_pos: usize,
    win_filled: usize,

    // Running checksums / counters.
    adler: u32,
    crc: u32,
    header_crc: u32,
    total_out: u64,

    // Preset dictionary (if supplied).
    zdict: Vec<u8>,

    // CPython decompressobj public state.
    pub(super) eof: bool,
    pub(super) unused_data: Vec<u8>,
    pub(super) unconsumed_tail: Vec<u8>,
}

impl Inflater {
    /// Validate `wbits` against CPython 3.12's accepted range and create a new inflater.
    pub(super) fn new(wbits: i32, zdict: Vec<u8>) -> Result<Self, &'static str> {
        let (wrapper, max_window_bits) = match wbits {
            -15..=-8 => (WrapperKind::Raw, (-wbits) as u8),
            0 => (WrapperKind::Zlib, 15),
            8..=15 => (WrapperKind::Zlib, wbits as u8),
            16 => (WrapperKind::Gzip, 15),
            24..=31 => (WrapperKind::Gzip, (wbits - 16) as u8),
            32 => (WrapperKind::Auto, 15),
            40..=47 => (WrapperKind::Auto, (wbits - 32) as u8),
            _ => return Err("Invalid initialization option"),
        };

        let initial_state = match wrapper {
            WrapperKind::Raw => State::BlockHeader,
            WrapperKind::Zlib => State::ZlibHeader {
                buf: [0; 2],
                have: 0,
            },
            WrapperKind::Gzip => State::GzipFixedHeader {
                buf: [0; 10],
                have: 0,
            },
            WrapperKind::Auto => State::AutoDetect,
        };

        let mut inflater = Self {
            active_wrapper: wrapper,
            max_window_bits,
            state: initial_state,
            bfinal: false,
            bit_buf: 0,
            bit_count: 0,
            litlen_dec: HuffmanDecoder::fixed_litlen(),
            dist_dec: HuffmanDecoder::fixed_dist(),
            window: vec![0u8; WINDOW_SIZE],
            win_pos: 0,
            win_filled: 0,
            adler: 1,
            crc: 0,
            header_crc: 0,
            total_out: 0,
            zdict,
            eof: false,
            unused_data: Vec::new(),
            unconsumed_tail: Vec::new(),
        };

        // Raw deflate seeds the sliding window immediately with `zdict`.
        if wrapper == WrapperKind::Raw && !inflater.zdict.is_empty() {
            let dict = inflater.zdict.clone();
            inflater.seed_window(&dict);
        }

        Ok(inflater)
    }

    fn seed_window(&mut self, dict: &[u8]) {
        let start = dict.len().saturating_sub(WINDOW_SIZE);
        for &b in &dict[start..] {
            self.window[self.win_pos] = b;
            self.win_pos = (self.win_pos + 1) & WINDOW_MASK;
            if self.win_filled < WINDOW_SIZE {
                self.win_filled += 1;
            }
        }
    }

    #[inline]
    fn emit_byte(&mut self, b: u8, out: &mut Vec<u8>) {
        out.push(b);
        self.window[self.win_pos] = b;
        self.win_pos = (self.win_pos + 1) & WINDOW_MASK;
        if self.win_filled < WINDOW_SIZE {
            self.win_filled += 1;
        }
        self.total_out = self.total_out.wrapping_add(1);
    }

    #[inline]
    fn fill_bits(&mut self, input: &[u8], pos: &mut usize, needed: u32) -> bool {
        while self.bit_count < needed && *pos < input.len() {
            self.bit_buf |= u64::from(input[*pos]) << self.bit_count;
            self.bit_count += 8;
            *pos += 1;
        }
        self.bit_count >= needed
    }

    #[inline]
    fn take_bits(&mut self, n: u32) -> u32 {
        let val = (self.bit_buf & ((1u64 << n) - 1)) as u32;
        self.bit_buf >>= n;
        self.bit_count -= n;
        val
    }

    /// Discard partial bits to align to a byte boundary.
    #[inline]
    fn align_byte(&mut self) {
        let drop = self.bit_count & 7;
        if drop > 0 {
            self.bit_buf >>= drop;
            self.bit_count -= drop;
        }
    }

    /// Read one aligned byte (first from any whole bytes buffered in `bit_buf`, then from `input`).
    #[inline]
    fn next_aligned_byte(&mut self, input: &[u8], pos: &mut usize) -> Option<u8> {
        debug_assert_eq!(self.bit_count & 7, 0);
        if self.bit_count >= 8 {
            let b = (self.bit_buf & 0xff) as u8;
            self.bit_buf >>= 8;
            self.bit_count -= 8;
            return Some(b);
        }
        if *pos < input.len() {
            let b = input[*pos];
            *pos += 1;
            Some(b)
        } else {
            None
        }
    }

    /// Drain any whole buffered bytes from `bit_buf` (after byte-aligning) followed by `input[pos..]`.
    fn collect_remaining_bytes(&mut self, input: &[u8], pos: usize) -> Vec<u8> {
        self.align_byte();
        let mut rem = Vec::with_capacity((self.bit_count as usize / 8) + (input.len() - pos));
        while self.bit_count >= 8 {
            rem.push((self.bit_buf & 0xff) as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
        rem.extend_from_slice(&input[pos..]);
        rem
    }

    /// Decode a Huffman symbol without ever pulling bytes past what is strictly needed.
    fn decode_huffman(
        bit_buf: &mut u64,
        bit_count: &mut u32,
        input: &[u8],
        pos: &mut usize,
        dec: &HuffmanDecoder,
    ) -> Result<Option<u16>, &'static str> {
        // Fast path when >= 9 bits are already buffered.
        if *bit_count >= 9
            && let Some((sym, len)) = dec.fast_decode(*bit_buf, *bit_count)
        {
            *bit_buf >>= len;
            *bit_count -= u32::from(len);
            return Ok(Some(sym));
        }

        loop {
            if let Some((sym, len)) = dec.fast_decode(*bit_buf, *bit_count) {
                *bit_buf >>= len;
                *bit_count -= u32::from(len);
                return Ok(Some(sym));
            }
            match dec.slow_decode(*bit_buf, *bit_count)? {
                Some((sym, len)) => {
                    *bit_buf >>= len;
                    *bit_count -= u32::from(len);
                    return Ok(Some(sym));
                }
                None => {
                    if *pos >= input.len() {
                        return Ok(None);
                    }
                    *bit_buf |= u64::from(input[*pos]) << *bit_count;
                    *bit_count += 8;
                    *pos += 1;
                }
            }
        }
    }

    /// Decompress a chunk of bytes. If `max_length > 0`, stop once `out.len() == max_length`.
    pub(super) fn decompress(
        &mut self,
        input: &[u8],
        max_length: usize,
    ) -> Result<Vec<u8>, String> {
        if self.eof {
            self.unused_data.extend_from_slice(input);
            self.unconsumed_tail.clear();
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let mut pos = 0usize;
        let mut out_checksum_start = 0usize;

        let res: Result<(), String> = (|| {
            loop {
                // Respect `max_length` before emitting more data.
                let at_limit = max_length > 0 && out.len() >= max_length;

                match &mut self.state {
                    State::AutoDetect => {
                        if pos >= input.len() {
                            break;
                        }
                        if input[pos] == 0x1f {
                            self.active_wrapper = WrapperKind::Gzip;
                            self.state = State::GzipFixedHeader {
                                buf: [0; 10],
                                have: 0,
                            };
                        } else {
                            self.active_wrapper = WrapperKind::Zlib;
                            self.state = State::ZlibHeader {
                                buf: [0; 2],
                                have: 0,
                            };
                        }
                    }

                    State::ZlibHeader { buf, have } => {
                        while *have < 2 && pos < input.len() {
                            buf[*have] = input[pos];
                            *have += 1;
                            pos += 1;
                        }
                        if *have < 2 {
                            break;
                        }
                        let cmf = buf[0];
                        let flg = buf[1];
                        let cm = cmf & 0x0f;
                        let cinfo = cmf >> 4;
                        // zlib's `inflate.c` validates the header checksum before
                        // the compression method.
                        if !((u16::from(cmf) << 8) | u16::from(flg)).is_multiple_of(31) {
                            return Err(
                                "Error -3 while decompressing data: incorrect header check"
                                    .to_string(),
                            );
                        }
                        if cm != 8 {
                            return Err(
                                "Error -3 while decompressing data: unknown compression method"
                                    .to_string(),
                            );
                        }
                        if cinfo > 7 || cinfo + 8 > self.max_window_bits {
                            return Err("Error -3 while decompressing data: invalid window size"
                                .to_string());
                        }
                        if (flg & 0x20) != 0 {
                            self.state = State::ZlibDict {
                                buf: [0; 4],
                                have: 0,
                            };
                        } else {
                            self.state = State::BlockHeader;
                        }
                    }

                    State::ZlibDict { buf, have } => {
                        while *have < 4 && pos < input.len() {
                            buf[*have] = input[pos];
                            *have += 1;
                            pos += 1;
                        }
                        if *have < 4 {
                            break;
                        }
                        if self.zdict.is_empty() {
                            return Err("Error 2 while decompressing data".to_string());
                        }
                        let expected_dict_id = u32::from_be_bytes(*buf);
                        let actual_dict_id = adler32(&self.zdict, 1);
                        if expected_dict_id != actual_dict_id {
                            return Err(
                                "Error -3 while decompressing data: invalid dictionary".to_string()
                            );
                        }
                        let dict = self.zdict.clone();
                        self.seed_window(&dict);
                        self.state = State::BlockHeader;
                    }

                    State::GzipFixedHeader { buf, have } => {
                        while *have < 10 && pos < input.len() {
                            buf[*have] = input[pos];
                            *have += 1;
                            pos += 1;
                        }
                        // zlib validates the magic once two bytes are available and
                        // the method / flags once four are, without waiting for the
                        // full ten-byte fixed header.
                        if *have >= 2 && (buf[0] != 0x1f || buf[1] != 0x8b) {
                            return Err(
                                "Error -3 while decompressing data: incorrect header check"
                                    .to_string(),
                            );
                        }
                        if *have >= 4 {
                            if buf[2] != 8 {
                                return Err(
                                    "Error -3 while decompressing data: unknown compression method"
                                        .to_string(),
                                );
                            }
                            if (buf[3] & 0xe0) != 0 {
                                return Err(
                                    "Error -3 while decompressing data: unknown header flags set"
                                        .to_string(),
                                );
                            }
                        }
                        if *have < 10 {
                            break;
                        }
                        let flags = buf[3];
                        self.header_crc = crc32(buf, 0);
                        self.state = next_gzip_header_state(flags, 0);
                    }

                    State::GzipExtraLen { flags, buf, have } => {
                        while *have < 2 && pos < input.len() {
                            buf[*have] = input[pos];
                            *have += 1;
                            pos += 1;
                        }
                        if *have < 2 {
                            break;
                        }
                        let f = *flags;
                        let xlen = u16::from_le_bytes(*buf) as usize;
                        self.header_crc = crc32(buf, self.header_crc);
                        if xlen == 0 {
                            self.state = next_gzip_header_state(f, 1);
                        } else {
                            self.state = State::GzipExtraData {
                                flags: f,
                                rem: xlen,
                            };
                        }
                    }

                    State::GzipExtraData { flags, rem } => {
                        let avail = input.len() - pos;
                        if avail == 0 {
                            break;
                        }
                        let take = (*rem).min(avail);
                        self.header_crc = crc32(&input[pos..pos + take], self.header_crc);
                        pos += take;
                        *rem -= take;
                        if *rem == 0 {
                            let f = *flags;
                            self.state = next_gzip_header_state(f, 1);
                        }
                    }

                    State::GzipName { flags } => {
                        let f = *flags;
                        let mut found = false;
                        while pos < input.len() {
                            let b = input[pos];
                            self.header_crc = crc32(&[b], self.header_crc);
                            pos += 1;
                            if b == 0 {
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            break;
                        }
                        self.state = next_gzip_header_state(f, 2);
                    }

                    State::GzipComment { flags } => {
                        let f = *flags;
                        let mut found = false;
                        while pos < input.len() {
                            let b = input[pos];
                            self.header_crc = crc32(&[b], self.header_crc);
                            pos += 1;
                            if b == 0 {
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            break;
                        }
                        self.state = next_gzip_header_state(f, 3);
                    }

                    State::GzipHcrc { buf, have } => {
                        while *have < 2 && pos < input.len() {
                            buf[*have] = input[pos];
                            *have += 1;
                            pos += 1;
                        }
                        if *have < 2 {
                            break;
                        }
                        let expected = u16::from_le_bytes(*buf);
                        if expected != (self.header_crc & 0xffff) as u16 {
                            return Err("Error -3 while decompressing data: header crc mismatch"
                                .to_string());
                        }
                        self.state = State::BlockHeader;
                    }

                    State::BlockHeader => {
                        if !self.fill_bits(input, &mut pos, 3) {
                            break;
                        }
                        self.bfinal = self.take_bits(1) != 0;
                        let btype = self.take_bits(2);
                        match btype {
                            0 => {
                                self.align_byte();
                                self.state = State::StoredLen {
                                    buf: [0; 4],
                                    have: 0,
                                };
                            }
                            1 => {
                                self.litlen_dec = HuffmanDecoder::fixed_litlen();
                                self.dist_dec = HuffmanDecoder::fixed_dist();
                                self.state = State::DecodeSym;
                            }
                            2 => {
                                self.state = State::DynHeader;
                            }
                            _ => {
                                return Err(
                                    "Error -3 while decompressing data: invalid block type"
                                        .to_string(),
                                );
                            }
                        }
                    }

                    State::StoredLen { buf, have } => {
                        let mut h = *have;
                        let mut b_arr = *buf;
                        while h < 4 {
                            let Some(b) = self.next_aligned_byte(input, &mut pos) else {
                                break;
                            };
                            b_arr[h] = b;
                            h += 1;
                        }
                        if h < 4 {
                            self.state = State::StoredLen {
                                buf: b_arr,
                                have: h,
                            };
                            break;
                        }
                        let len = u16::from_le_bytes([b_arr[0], b_arr[1]]);
                        let nlen = u16::from_le_bytes([b_arr[2], b_arr[3]]);
                        if len != !nlen {
                            return Err(
                                "Error -3 while decompressing data: invalid stored block lengths"
                                    .to_string(),
                            );
                        }
                        if len == 0 {
                            self.finish_block();
                        } else {
                            self.state = State::StoredCopy { rem: len as usize };
                        }
                    }

                    State::StoredCopy { rem } => {
                        if at_limit {
                            break;
                        }
                        let mut remaining = *rem;
                        while remaining > 0 {
                            if max_length > 0 && out.len() >= max_length {
                                break;
                            }
                            let Some(b) = self.next_aligned_byte(input, &mut pos) else {
                                break;
                            };
                            self.emit_byte(b, &mut out);
                            remaining -= 1;
                        }
                        if remaining == 0 {
                            self.finish_block();
                        } else {
                            self.state = State::StoredCopy { rem: remaining };
                            break;
                        }
                    }

                    State::DynHeader => {
                        if !self.fill_bits(input, &mut pos, 14) {
                            break;
                        }
                        let hlit = self.take_bits(5) as usize + 257;
                        let hdist = self.take_bits(5) as usize + 1;
                        let hclen = self.take_bits(4) as usize + 4;
                        if hlit > 286 || hdist > 30 {
                            return Err(
                                "Error -3 while decompressing data: too many length or distance symbols"
                                    .to_string(),
                            );
                        }
                        self.state = State::DynCodeLens {
                            hlit,
                            hdist,
                            hclen,
                            idx: 0,
                            cl_lens: [0u8; 19],
                        };
                    }

                    State::DynCodeLens {
                        hlit,
                        hdist,
                        hclen,
                        idx,
                        cl_lens,
                    } => {
                        let (hl, hd, hc) = (*hlit, *hdist, *hclen);
                        let mut i = *idx;
                        let mut cl = *cl_lens;
                        while i < hc {
                            if !self.fill_bits(input, &mut pos, 3) {
                                break;
                            }
                            cl[CODE_LEN_ORDER[i]] = self.take_bits(3) as u8;
                            i += 1;
                        }
                        if i < hc {
                            self.state = State::DynCodeLens {
                                hlit: hl,
                                hdist: hd,
                                hclen: hc,
                                idx: i,
                                cl_lens: cl,
                            };
                            break;
                        }
                        let cl_dec = Box::new(
                            HuffmanDecoder::from_lengths(&cl, 7)
                                .map_err(|e| format!("Error -3 while decompressing data: {e}"))?,
                        );
                        self.state = State::DynTrees {
                            hlit: hl,
                            hdist: hd,
                            cl_dec,
                            lens: Vec::with_capacity(hl + hd),
                            pending_repeat: None,
                        };
                    }

                    State::DynTrees {
                        hlit,
                        hdist,
                        cl_dec,
                        lens,
                        pending_repeat,
                    } => {
                        let total = *hlit + *hdist;
                        let mut need_more = false;
                        while lens.len() < total {
                            if let Some(sym) = *pending_repeat {
                                let (extra_bits, base_rep, val) = match sym {
                                    16 => {
                                        let Some(&prev) = lens.last() else {
                                            return Err(
                                                "Error -3 while decompressing data: invalid bit length repeat"
                                                    .to_string(),
                                            );
                                        };
                                        (2u32, 3usize, prev)
                                    }
                                    17 => (3u32, 3usize, 0u8),
                                    18 => (7u32, 11usize, 0u8),
                                    _ => unreachable!(),
                                };
                                while self.bit_count < extra_bits && pos < input.len() {
                                    self.bit_buf |= u64::from(input[pos]) << self.bit_count;
                                    self.bit_count += 8;
                                    pos += 1;
                                }
                                if self.bit_count < extra_bits {
                                    need_more = true;
                                    break;
                                }
                                let extra = (self.bit_buf & ((1u64 << extra_bits) - 1)) as usize;
                                self.bit_buf >>= extra_bits;
                                self.bit_count -= extra_bits;
                                let count = base_rep + extra;
                                if lens.len() + count > total {
                                    return Err(
                                        "Error -3 while decompressing data: invalid bit length repeat"
                                            .to_string(),
                                    );
                                }
                                for _ in 0..count {
                                    lens.push(val);
                                }
                                *pending_repeat = None;
                                continue;
                            }

                            let sym = match Self::decode_huffman(
                                &mut self.bit_buf,
                                &mut self.bit_count,
                                input,
                                &mut pos,
                                cl_dec,
                            ) {
                                Ok(Some(s)) => s as u8,
                                Ok(None) => {
                                    need_more = true;
                                    break;
                                }
                                Err(e) => {
                                    return Err(format!("Error -3 while decompressing data: {e}"));
                                }
                            };

                            if sym < 16 {
                                lens.push(sym);
                            } else if sym <= 18 {
                                *pending_repeat = Some(sym);
                            } else {
                                return Err(
                                    "Error -3 while decompressing data: invalid code lengths set"
                                        .to_string(),
                                );
                            }
                        }

                        if need_more {
                            break;
                        }

                        let hl = *hlit;
                        if lens.get(256).copied().unwrap_or(0) == 0 {
                            return Err(
                                "Error -3 while decompressing data: invalid literal/lengths set"
                                    .to_string(),
                            );
                        }
                        let litlen_dec = HuffmanDecoder::from_lengths(&lens[..hl], 15)
                            .map_err(|e| format!("Error -3 while decompressing data: {e}"))?;
                        let dist_dec = HuffmanDecoder::from_lengths(&lens[hl..], 15)
                            .map_err(|e| format!("Error -3 while decompressing data: {e}"))?;
                        self.litlen_dec = litlen_dec;
                        self.dist_dec = dist_dec;
                        self.state = State::DecodeSym;
                    }

                    State::DecodeSym => {
                        if at_limit {
                            break;
                        }
                        let sym = match Self::decode_huffman(
                            &mut self.bit_buf,
                            &mut self.bit_count,
                            input,
                            &mut pos,
                            &self.litlen_dec,
                        ) {
                            Ok(Some(s)) => s,
                            Ok(None) => break,
                            Err(e) => {
                                return Err(format!("Error -3 while decompressing data: {e}"));
                            }
                        };

                        if sym < 256 {
                            self.emit_byte(sym as u8, &mut out);
                        } else if sym == 256 {
                            self.finish_block();
                        } else if sym <= 285 {
                            let idx = (sym - 257) as usize;
                            let base = LENGTH_BASE[idx];
                            let extra = LENGTH_EXTRA[idx];
                            if extra == 0 {
                                self.state = State::DecodeDist { len: base };
                            } else {
                                self.state = State::DecodeLenExtra { base, extra };
                            }
                        } else {
                            return Err(
                                "Error -3 while decompressing data: invalid literal/length code"
                                    .to_string(),
                            );
                        }
                    }

                    State::DecodeLenExtra { base, extra } => {
                        let (b, ex) = (*base, *extra);
                        if !self.fill_bits(input, &mut pos, u32::from(ex)) {
                            break;
                        }
                        let add = self.take_bits(u32::from(ex)) as u16;
                        self.state = State::DecodeDist { len: b + add };
                    }

                    State::DecodeDist { len } => {
                        let match_len = *len;
                        let dsym = match Self::decode_huffman(
                            &mut self.bit_buf,
                            &mut self.bit_count,
                            input,
                            &mut pos,
                            &self.dist_dec,
                        ) {
                            Ok(Some(s)) => s,
                            Ok(None) => break,
                            Err(e) => {
                                return Err(format!("Error -3 while decompressing data: {e}"));
                            }
                        };
                        if dsym >= 30 {
                            return Err("Error -3 while decompressing data: invalid distance code"
                                .to_string());
                        }
                        let base = DIST_BASE[dsym as usize];
                        let extra = DIST_EXTRA[dsym as usize];
                        if extra == 0 {
                            let dist = base as usize;
                            if dist > self.win_filled {
                                return Err(
                                    "Error -3 while decompressing data: invalid distance too far back"
                                        .to_string(),
                                );
                            }
                            self.state = State::CopyMatch {
                                dist,
                                rem: match_len,
                            };
                        } else {
                            self.state = State::DecodeDistExtra {
                                len: match_len,
                                base,
                                extra,
                            };
                        }
                    }

                    State::DecodeDistExtra { len, base, extra } => {
                        let (match_len, b, ex) = (*len, *base, *extra);
                        if !self.fill_bits(input, &mut pos, u32::from(ex)) {
                            break;
                        }
                        let add = self.take_bits(u32::from(ex)) as u16;
                        let dist = (b + add) as usize;
                        if dist > self.win_filled {
                            return Err(
                                "Error -3 while decompressing data: invalid distance too far back"
                                    .to_string(),
                            );
                        }
                        self.state = State::CopyMatch {
                            dist,
                            rem: match_len,
                        };
                    }

                    State::CopyMatch { dist, rem } => {
                        if at_limit {
                            break;
                        }
                        let d = *dist;
                        let mut r = *rem;
                        while r > 0 {
                            if max_length > 0 && out.len() >= max_length {
                                break;
                            }
                            let src_idx = self.win_pos.wrapping_sub(d) & WINDOW_MASK;
                            let byte = self.window[src_idx];
                            self.emit_byte(byte, &mut out);
                            r -= 1;
                        }
                        if r == 0 {
                            self.state = State::DecodeSym;
                        } else {
                            self.state = State::CopyMatch { dist: d, rem: r };
                            break;
                        }
                    }

                    State::ZlibTrailer { buf, have } => {
                        // Update running checksum with all bytes emitted so far before checking trailer.
                        if out_checksum_start < out.len() {
                            self.adler = adler32(&out[out_checksum_start..], self.adler);
                            out_checksum_start = out.len();
                        }
                        let mut h = *have;
                        let mut b_arr = *buf;
                        while h < 4 {
                            let Some(b) = self.next_aligned_byte(input, &mut pos) else {
                                break;
                            };
                            b_arr[h] = b;
                            h += 1;
                        }
                        if h < 4 {
                            self.state = State::ZlibTrailer {
                                buf: b_arr,
                                have: h,
                            };
                            break;
                        }
                        let expected = u32::from_be_bytes(b_arr);
                        if expected != self.adler {
                            return Err("Error -3 while decompressing data: incorrect data check"
                                .to_string());
                        }
                        self.state = State::Done;
                        self.eof = true;
                        break;
                    }

                    State::GzipTrailer { buf, have } => {
                        if out_checksum_start < out.len() {
                            self.crc = crc32(&out[out_checksum_start..], self.crc);
                            out_checksum_start = out.len();
                        }
                        let mut h = *have;
                        let mut b_arr = *buf;
                        while h < 8 {
                            let Some(b) = self.next_aligned_byte(input, &mut pos) else {
                                break;
                            };
                            b_arr[h] = b;
                            h += 1;
                        }
                        if h < 8 {
                            self.state = State::GzipTrailer {
                                buf: b_arr,
                                have: h,
                            };
                            break;
                        }
                        let expected_crc =
                            u32::from_le_bytes([b_arr[0], b_arr[1], b_arr[2], b_arr[3]]);
                        let expected_isize =
                            u32::from_le_bytes([b_arr[4], b_arr[5], b_arr[6], b_arr[7]]);
                        if expected_crc != self.crc {
                            return Err("Error -3 while decompressing data: incorrect data check"
                                .to_string());
                        }
                        if expected_isize != (self.total_out as u32) {
                            return Err(
                                "Error -3 while decompressing data: incorrect length check"
                                    .to_string(),
                            );
                        }
                        self.state = State::Done;
                        self.eof = true;
                        break;
                    }

                    State::Done => {
                        self.eof = true;
                        break;
                    }
                }
            }
            Ok(())
        })();

        // Update running checksums for any newly produced output.
        if out_checksum_start < out.len() {
            match self.active_wrapper {
                WrapperKind::Zlib => {
                    self.adler = adler32(&out[out_checksum_start..], self.adler);
                }
                WrapperKind::Gzip => {
                    self.crc = crc32(&out[out_checksum_start..], self.crc);
                }
                WrapperKind::Raw | WrapperKind::Auto => {}
            }
        }

        res?;

        if self.eof {
            self.unconsumed_tail.clear();
            let rem = self.collect_remaining_bytes(input, pos);
            self.unused_data.extend_from_slice(&rem);
        } else if max_length > 0 && out.len() >= max_length {
            self.unconsumed_tail = input[pos..].to_vec();
        } else {
            self.unconsumed_tail.clear();
        }

        Ok(out)
    }

    /// Flush remaining buffered output (for `decompressobj.flush()`).
    pub(super) fn flush(&mut self) -> Result<Vec<u8>, String> {
        let tail = std::mem::take(&mut self.unconsumed_tail);
        self.decompress(&tail, 0)
    }

    fn finish_block(&mut self) {
        if self.bfinal {
            self.align_byte();
            match self.active_wrapper {
                WrapperKind::Raw => {
                    self.state = State::Done;
                    self.eof = true;
                }
                WrapperKind::Zlib => {
                    self.state = State::ZlibTrailer {
                        buf: [0; 4],
                        have: 0,
                    };
                }
                WrapperKind::Gzip => {
                    self.state = State::GzipTrailer {
                        buf: [0; 8],
                        have: 0,
                    };
                }
                WrapperKind::Auto => unreachable!(),
            }
        } else {
            self.state = State::BlockHeader;
        }
    }
}

fn next_gzip_header_state(flags: u8, after_stage: u8) -> State {
    // Stage 0: after fixed 10-byte header -> check FEXTRA (0x04)
    if after_stage < 1 && (flags & 0x04) != 0 {
        return State::GzipExtraLen {
            flags,
            buf: [0; 2],
            have: 0,
        };
    }
    // Stage 1: after FEXTRA -> check FNAME (0x08)
    if after_stage < 2 && (flags & 0x08) != 0 {
        return State::GzipName { flags };
    }
    // Stage 2: after FNAME -> check FCOMMENT (0x10)
    if after_stage < 3 && (flags & 0x10) != 0 {
        return State::GzipComment { flags };
    }
    // Stage 3: after FCOMMENT -> check FHCRC (0x02)
    if after_stage < 4 && (flags & 0x02) != 0 {
        return State::GzipHcrc {
            buf: [0; 2],
            have: 0,
        };
    }
    State::BlockHeader
}
