// Unicode UAX #15 normalization (NFC, NFD, NFKC, NFKD) backed by the Unicode
// 15.0.0 tables in `tables.rs`.

use pyrust_core::{cesu8_codepoints, cesu8_encode_codepoint};

use super::tables::{CANON_COMPOSE, DECOMP_DATA, DECOMP_INDEX, PROP_RECORDS, PROP_RUNS};

const HANGUL_S_BASE: u32 = 0xAC00;
const HANGUL_L_BASE: u32 = 0x1100;
const HANGUL_V_BASE: u32 = 0x1161;
const HANGUL_T_BASE: u32 = 0x11A7;
const HANGUL_L_COUNT: u32 = 19;
const HANGUL_V_COUNT: u32 = 21;
const HANGUL_T_COUNT: u32 = 28;
const HANGUL_N_COUNT: u32 = HANGUL_V_COUNT * HANGUL_T_COUNT; // 588
const HANGUL_S_COUNT: u32 = HANGUL_L_COUNT * HANGUL_N_COUNT; // 11172

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NormForm {
    Nfc,
    Nfd,
    Nfkc,
    Nfkd,
}

impl NormForm {
    pub(super) fn parse(s: &str) -> Option<Self> {
        match s {
            "NFC" => Some(Self::Nfc),
            "NFD" => Some(Self::Nfd),
            "NFKC" => Some(Self::Nfkc),
            "NFKD" => Some(Self::Nfkd),
            _ => None,
        }
    }

    fn is_compat(self) -> bool {
        matches!(self, Self::Nfkc | Self::Nfkd)
    }

    fn is_composed(self) -> bool {
        matches!(self, Self::Nfc | Self::Nfkc)
    }
}

#[inline]
fn combining_class(cp: u32) -> u8 {
    let pos = PROP_RUNS.partition_point(|&entry| (entry >> 11) <= cp);
    let idx = if pos == 0 { 0 } else { pos - 1 };
    let rec_idx = (PROP_RUNS[idx] & 0x7FF) as usize;
    PROP_RECORDS[rec_idx].comb
}

#[inline]
fn lookup_decomp(cp: u32) -> Option<(u8, &'static [u32])> {
    let idx = DECOMP_INDEX
        .binary_search_by_key(&cp, |&entry| (entry >> 24) as u32)
        .ok()?;
    let entry = DECOMP_INDEX[idx];
    let tag = ((entry >> 19) & 0x1F) as u8;
    let len = ((entry >> 14) & 0x1F) as usize;
    let offset = (entry & 0x3FFF) as usize;
    Some((tag, &DECOMP_DATA[offset..offset + len]))
}

fn decompose_cp(cp: u32, compat: bool, out: &mut Vec<u32>) {
    // Algorithmic Hangul syllable decomposition (UAX #15 / Unicode §3.12).
    if (HANGUL_S_BASE..HANGUL_S_BASE + HANGUL_S_COUNT).contains(&cp) {
        let s_index = cp - HANGUL_S_BASE;
        let l = HANGUL_L_BASE + s_index / HANGUL_N_COUNT;
        let v = HANGUL_V_BASE + (s_index % HANGUL_N_COUNT) / HANGUL_T_COUNT;
        let t = HANGUL_T_BASE + s_index % HANGUL_T_COUNT;
        out.push(l);
        out.push(v);
        if t != HANGUL_T_BASE {
            out.push(t);
        }
        return;
    }

    if let Some((tag, slice)) = lookup_decomp(cp)
        && (compat || tag == 0)
    {
        for &sub in slice {
            decompose_cp(sub, compat, out);
        }
        return;
    }

    out.push(cp);
}

fn canonical_order(buf: &mut [u32]) {
    let mut i = 0;
    while i < buf.len() {
        if combining_class(buf[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < buf.len() && combining_class(buf[i]) != 0 {
            i += 1;
        }
        // Insertion sort (stable) on the contiguous non-starter run.
        for j in (start + 1)..i {
            let key = buf[j];
            let key_ccc = combining_class(key);
            let mut k = j;
            while k > start && combining_class(buf[k - 1]) > key_ccc {
                buf[k] = buf[k - 1];
                k -= 1;
            }
            buf[k] = key;
        }
    }
}

#[inline]
fn compose_pair(a: u32, b: u32) -> Option<u32> {
    // Algorithmic Hangul composition: L + V -> LV
    if (HANGUL_L_BASE..HANGUL_L_BASE + HANGUL_L_COUNT).contains(&a)
        && (HANGUL_V_BASE..HANGUL_V_BASE + HANGUL_V_COUNT).contains(&b)
    {
        let l_idx = a - HANGUL_L_BASE;
        let v_idx = b - HANGUL_V_BASE;
        return Some(HANGUL_S_BASE + (l_idx * HANGUL_V_COUNT + v_idx) * HANGUL_T_COUNT);
    }
    // Algorithmic Hangul composition: LV + T -> LVT
    if (HANGUL_S_BASE..HANGUL_S_BASE + HANGUL_S_COUNT).contains(&a)
        && (a - HANGUL_S_BASE).is_multiple_of(HANGUL_T_COUNT)
        && (HANGUL_T_BASE + 1..HANGUL_T_BASE + HANGUL_T_COUNT).contains(&b)
    {
        return Some(a + (b - HANGUL_T_BASE));
    }

    CANON_COMPOSE
        .binary_search_by_key(&(a, b), |&(x, y, _)| (x, y))
        .ok()
        .map(|idx| CANON_COMPOSE[idx].2)
}

fn canonical_compose(buf: &mut Vec<u32>) {
    if buf.len() < 2 {
        return;
    }
    let mut starter_idx: Option<usize> = None;
    let mut last_ccc: i16 = -1;
    let mut out_len = 0usize;

    for i in 0..buf.len() {
        let ch = buf[i];
        let ccc = combining_class(ch);
        if let Some(s_idx) = starter_idx
            && (last_ccc < i16::from(ccc) || last_ccc == -1)
            && let Some(composed) = compose_pair(buf[s_idx], ch)
        {
            buf[s_idx] = composed;
            continue;
        }
        if ccc == 0 {
            starter_idx = Some(out_len);
            last_ccc = -1;
        } else {
            last_ccc = i16::from(ccc);
        }
        buf[out_len] = ch;
        out_len += 1;
    }
    buf.truncate(out_len);
}

pub(super) fn normalize(form: NormForm, s: &str) -> String {
    if s.is_ascii() {
        return s.to_owned();
    }
    let compat = form.is_compat();
    let mut buf: Vec<u32> = Vec::with_capacity(s.len());
    for cp in cesu8_codepoints(s) {
        decompose_cp(cp, compat, &mut buf);
    }
    canonical_order(&mut buf);
    if form.is_composed() {
        canonical_compose(&mut buf);
    }
    let mut out = String::with_capacity(s.len());
    for cp in buf {
        if let Some(ch) = char::from_u32(cp) {
            out.push(ch);
        } else {
            out.push_str(&cesu8_encode_codepoint(cp));
        }
    }
    out
}

pub(super) fn is_normalized(form: NormForm, s: &str) -> bool {
    if s.is_ascii() {
        return true;
    }
    normalize(form, s) == s
}
