// ─────────────────────────────────────────────────────────────────────────────
// NaN-boxing constants
// ─────────────────────────────────────────────────────────────────────────────

const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
const INT_SIGN_BIT: u64 = 1 << 47;
const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

const TAG_NONE_BITS: u64 = 0xFFF9_0000_0000_0000;
/// Internal-only sentinel for "uninitialised register slot". Positive NaN
/// bit pattern outside the negative-NaN range used by the tag system; not
/// observable from Python code. See `Value::unset()`.
const UNSET_BITS: u64 = 0x7FF8_0000_0000_BAD0;
/// The `NotImplemented` singleton.  Stored as a reserved NaN-box pattern
/// so identity comparison is a cheap u64-eq, and the value doesn't take an
/// `Opaque` variant.  Pattern is a positive NaN in the same family as
/// `UNSET_BITS` — not classified as a float by `top16()`-based checks
/// because we test the exact bit pattern explicitly. See [`Value::not_implemented`].
const NOT_IMPLEMENTED_BITS: u64 = 0x7FF8_0000_0000_BAD2;
/// The `Ellipsis` singleton (`...`). Stored as a reserved NaN-box pattern
/// in the same positive-NaN family as `UNSET_BITS` and `NOT_IMPLEMENTED_BITS`.
/// Must be tested explicitly before the float arm in `kind()` / `is_float()`.
const ELLIPSIS_BITS: u64 = 0x7FF8_0000_0000_BAD4;
/// Bit 47 of the NaN payload.  Always set on a NaN minted by [`Value::float`].
///
/// The reserved sentinels [`UNSET_BITS`], [`NOT_IMPLEMENTED_BITS`] and
/// [`ELLIPSIS_BITS`] occupy payloads `0xBAD0` / `0xBAD2` / `0xBAD4` in the very
/// same positive-NaN family, so a naive counter starting at zero would collide
/// with them after ~48k NaNs.  That is not a cosmetic clash: a `0xBAD0`-payload
/// NaN makes `is_unset()` return `true`, and release builds only
/// `debug_assert!` against reading an unset slot, so the value would silently
/// behave as an uninitialised register.  Forcing bit 47 keeps every minted
/// payload at or above 2^47 and therefore permanently clear of that region.
const NAN_IDENTITY_BIT: u64 = 1 << 47;
/// Counter bits available below [`NAN_IDENTITY_BIT`] (2^47 distinct identities).
const NAN_IDENTITY_MASK: u64 = NAN_IDENTITY_BIT - 1;

thread_local! {
    /// Per-thread source of NaN object identities (#2911).  Values are
    /// thread-confined, so a non-atomic `Cell` matches the ownership model.
    static NAN_IDENTITY_COUNTER: Cell<u64> = const { Cell::new(0) };
}

/// Mint a fresh NaN bit pattern carrying a distinct object identity.
///
/// CPython gives every `float('nan')` its own object, and that is observable:
/// two distinct NaNs occupy two dict slots, `b in {a}` is `False`, and
/// `[a] == [b]` is `False` while `[a] == [a]` is `True`.  pyrust's floats are
/// NaN-boxed immediates with no heap allocation to hang an identity off, so the
/// identity is carried in the otherwise-unused NaN payload.  "Same object" then
/// becomes exactly "same bits" — which is precisely what `is_identical_nan` and
/// `values_are_identical` already test, so those call sites gain CPython
/// semantics without changing.
///
/// The counter wraps after 2^47 mintings and identities are then reused; that
/// is the same practical caveat CPython carries when a freed object's address
/// is recycled.
#[cold]
#[inline(never)]
fn mint_nan_identity() -> u64 {
    // A `Value` may be constructed while thread-locals are being torn down
    // (drop glue running after this key's destructor).  Fall back to the bare
    // identity pattern rather than panicking; it stays a valid, in-family NaN.
    let payload = NAN_IDENTITY_COUNTER
        .try_with(|counter| {
            let next = counter.get().wrapping_add(1) & NAN_IDENTITY_MASK;
            counter.set(next);
            next
        })
        .unwrap_or(0);
    CANONICAL_NAN | NAN_IDENTITY_BIT | payload
}

/// Whether `bits` is a NaN pattern minted by [`mint_nan_identity`].
///
/// Used to decide when a raw bit pattern may be restored verbatim.  Anything
/// else — in particular a *negative* NaN, whose top 16 bits overlap `TAG_STR`
/// through `TAG_OPAQUE` — must be normalised through `Value::float` instead, or
/// it would be decoded as a heap pointer.
#[inline]
fn is_minted_nan(bits: u64) -> bool {
    bits & !NAN_IDENTITY_MASK == CANONICAL_NAN | NAN_IDENTITY_BIT
}

const TAG_BOOL_BITS: u64 = 0xFFFA_0000_0000_0000;
const TAG_INT_BITS: u64 = 0xFFFB_0000_0000_0000;
const TAG_STR_BITS: u64 = 0xFFFC_0000_0000_0000;
const TAG_TUPLE_BITS: u64 = 0xFFFD_0000_0000_0000;
const TAG_LIST_BITS: u64 = 0xFFFE_0000_0000_0000;
const TAG_OPAQUE_BITS: u64 = 0xFFFF_0000_0000_0000;

// top16() tag values used in match arms and comparisons
const TAG_FLOAT_MAX: u16 = 0xFFF8; // all top16 values ≤ this are floats
const TAG_NONE: u16 = 0xFFF9;
const TAG_BOOL: u16 = 0xFFFA;
const TAG_INT: u16 = 0xFFFB;
const TAG_STR: u16 = 0xFFFC;
const TAG_TUPLE: u16 = 0xFFFD;
const TAG_LIST: u16 = 0xFFFE;
const TAG_OPAQUE: u16 = 0xFFFF;

// String header `rc_type` (offset 0, u32) bit layout:
//   bit 0      — layout discriminant: 0 = Layout A (owned), 1 = Layout B (slice)
//   bit 1      — `is_ascii`: cached ASCII-ness (valid only when bit 2 is set)
//   bit 2      — `ascii_computed`: 1 once the ASCII flag has been determined
//   bits 31:3  — reference count (one ref == `STR_RC_ONE`)
//
// Strings are immutable, so a computed ASCII flag never invalidates.  The flag
// is set eagerly in `Value::string` (it already touches every byte) and
// propagated cheaply for ASCII parents in `string_slice`; otherwise
// `str_is_ascii` computes it lazily on first query and caches it in-place.
const STR_TYPE_B: u32 = 0b001; // bit 0 — Layout B (slice)
const STR_IS_ASCII: u32 = 0b010; // bit 1 — cached ASCII-ness
const STR_ASCII_COMPUTED: u32 = 0b100; // bit 2 — flag has been computed
const STR_RC_ONE: u32 = 0b1000; // rc in bits 31:3; one reference
const STR_RC_MAX: u32 = u32::MAX >> 3;

/// Increment a string header's packed reference count without modifying its
/// layout/ASCII flags. Once the 29-bit counter is saturated the allocation is
/// intentionally immortal, matching the opaque-value saturation policy.
#[inline(always)]
unsafe fn str_refcount_increment(header: *mut u32) {
    let current = unsafe { *header };
    if current >> 3 != STR_RC_MAX {
        unsafe { *header = current + STR_RC_ONE };
    }
}

/// Decrement a packed string reference count.
///
/// Returns `true` only when the count reached zero. A saturated header is an
/// immortal sentinel and is left bit-identical so subsequent drops cannot
/// reinterpret corrupted low flag bits or eventually free an over-shared
/// allocation.
#[inline(always)]
unsafe fn str_refcount_decrement(header: *mut u32) -> bool {
    let current = unsafe { *header };
    if current >> 3 == STR_RC_MAX {
        return false;
    }
    debug_assert!(current >> 3 > 0, "dropping a released string allocation");
    let next = current - STR_RC_ONE;
    unsafe { *header = next };
    next >> 3 == 0
}

// Non-ASCII strings lazily cache their codepoint length as a tagged usize in the
// header. Repeated indexed access upgrades that word to a pointer to a sparse
// codepoint→byte-offset index; a one-shot access scans without allocating.
// Keeping one checkpoint per 32 codepoints bounds reused lookup to 31 short
// UTF-8 steps while using at most 1/32 of the memory of a full offset table.
//
// `unicode_state` encoding (all cache allocations are 8-byte aligned):
//   0             — no metadata yet
//   low bit set   — `(codepoint_len << 2) | length_tag | index_seen`
//   low bit clear — `*mut StrUnicodeCache`
const STR_OFFSET_STRIDE: usize = 32;
const STR_OWNED_HEADER_SIZE: usize = 16;
const STR_MAX_BYTE_LEN: usize = u32::MAX as usize;
const STR_SLICE_LAYOUT_SIZE: usize = 32;
const STR_SLICE_CACHE_OFFSET: usize = 24;
const STR_CODEPOINT_LEN_TAG: usize = 1;
const STR_CODEPOINT_INDEX_SEEN: usize = 2;
const STR_CODEPOINT_LEN_SHIFT: usize = 2;

struct StrUnicodeCache {
    codepoint_len: u32,
    checkpoints: Box<[u32]>,
}

impl StrUnicodeCache {
    fn build(s: &str) -> Box<Self> {
        let mut checkpoints = Vec::new();
        let mut codepoint_len = 0usize;
        let bytes = s.as_bytes();
        let mut byte_offset = 0usize;
        while byte_offset < bytes.len() {
            if codepoint_len.is_multiple_of(STR_OFFSET_STRIDE) {
                checkpoints.push(byte_offset as u32);
            }
            byte_offset += utf8_codepoint_width(bytes[byte_offset]);
            codepoint_len += 1;
        }
        Box::new(Self {
            codepoint_len: codepoint_len as u32,
            checkpoints: checkpoints.into_boxed_slice(),
        })
    }

    #[inline]
    fn byte_offset(&self, s: &str, index: usize) -> usize {
        let len = self.codepoint_len as usize;
        debug_assert!(index <= len);
        if index == len {
            return s.len();
        }

        let checkpoint = index / STR_OFFSET_STRIDE;
        let mut codepoint = checkpoint * STR_OFFSET_STRIDE;
        let mut byte_offset = self.checkpoints[checkpoint] as usize;
        let bytes = s.as_bytes();
        while codepoint < index {
            byte_offset += utf8_codepoint_width(bytes[byte_offset]);
            codepoint += 1;
        }
        byte_offset
    }
}

#[inline(always)]
fn utf8_codepoint_width(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

#[inline(always)]
fn utf8_codepoint_boundary(bytes: &[u8], index: usize) -> bool {
    index == 0
        || index == bytes.len()
        || (index < bytes.len() && bytes[index] & 0b1100_0000 != 0b1000_0000)
}

#[inline]
fn utf8_codepoint_count(bytes: &[u8]) -> usize {
    // A codepoint starts at every byte except a UTF-8 continuation byte
    // (`10xxxxxx`). This mirrors the lane-accumulation strategy used by Rust's
    // optimized `str::chars().count()`, but operates on raw bytes so pyrust's
    // CESU-8 surrogate representation is safe too.
    const CHUNK_WORDS: usize = 192;

    let (head, body, tail) = unsafe { bytes.align_to::<u64>() };
    let mut total = utf8_codepoint_count_scalar(head) + utf8_codepoint_count_scalar(tail);
    for chunk in body.chunks(CHUNK_WORDS) {
        let mut lane_counts = 0u64;
        for &word in chunk {
            lane_counts += utf8_start_markers(word);
        }
        total += sum_u8_lanes(lane_counts);
    }
    total
}

#[inline(always)]
fn utf8_start_markers(word: u64) -> u64 {
    const LOW_BITS: u64 = 0x0101_0101_0101_0101;
    ((!word >> 7) | (word >> 6)) & LOW_BITS
}

#[inline(always)]
fn utf8_starts_in_word(word: u64) -> usize {
    utf8_start_markers(word).count_ones() as usize
}

#[inline(always)]
fn sum_u8_lanes(values: u64) -> usize {
    const LOW_U16_LANES: u64 = 0x0001_0001_0001_0001;
    const LOW_BYTES: u64 = 0x00ff_00ff_00ff_00ff;
    let pair_sum = (values & LOW_BYTES) + ((values >> 8) & LOW_BYTES);
    (pair_sum.wrapping_mul(LOW_U16_LANES) >> 48) as usize
}

#[inline]
fn utf8_codepoint_count_scalar(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .filter(|byte| **byte & 0b1100_0000 != 0b1000_0000)
        .count()
}

#[inline]
fn uncached_utf8_byte_offset(bytes: &[u8], index: usize, codepoint_len: usize) -> usize {
    debug_assert!(index <= codepoint_len);
    if index <= codepoint_len - index {
        let mut byte_offset = 0usize;
        let mut remaining = index;
        while byte_offset + 8 <= bytes.len() {
            let word = u64::from_ne_bytes(
                bytes[byte_offset..byte_offset + 8]
                    .try_into()
                    .expect("exact 8-byte chunk"),
            );
            let starts = utf8_starts_in_word(word);
            if remaining < starts {
                break;
            }
            remaining -= starts;
            byte_offset += 8;
        }
        while byte_offset < bytes.len() {
            if bytes[byte_offset] & 0b1100_0000 != 0b1000_0000 {
                if remaining == 0 {
                    return byte_offset;
                }
                remaining -= 1;
            }
            byte_offset += 1;
        }
        debug_assert_eq!(remaining, 0);
        byte_offset
    } else {
        let mut byte_offset = bytes.len();
        let mut remaining = codepoint_len - index;
        while byte_offset >= 8 {
            let word = u64::from_ne_bytes(
                bytes[byte_offset - 8..byte_offset]
                    .try_into()
                    .expect("exact 8-byte chunk"),
            );
            let starts = utf8_starts_in_word(word);
            if remaining <= starts {
                break;
            }
            remaining -= starts;
            byte_offset -= 8;
        }
        while remaining > 0 {
            byte_offset -= 1;
            if bytes[byte_offset] & 0b1100_0000 != 0b1000_0000 {
                remaining -= 1;
            }
        }
        byte_offset
    }
}

// ── Small-string optimisation (SSO), issue #2832 ─────────────────────────────
//
// A TAG_STR value whose payload low bit is set stores its bytes *inline* in the
// NaN-box payload instead of pointing at a heap buffer.  Heap string pointers
// come from `alloc(…, align 8)` / `pool_b_alloc` (also align 8), so their low 3
// bits are always 0 — bit 0 is therefore a free, unambiguous inline marker.
//
// Inline payload layout (48 payload bits, little-endian):
//   bit 0      — 1 = inline (0 = heap pointer)
//   bits 1:3   — byte length (0..=STR_INLINE_MAX)
//   bytes 1..6 — up to 5 bytes of UTF-8 data (payload bytes 1..5 = bits 8..47)
//
// Inline strings never touch the heap: `clone` is a bit-copy, `drop` is a
// no-op, and identical content always yields identical bits (so short strings
// are implicitly interned and `is` / bit-equality stay consistent — every
// constructor routes the ≤ MAX case through `make_inline_str`).
const STR_INLINE_MARK: u64 = 1; // payload bit 0
pub(crate) const STR_INLINE_MAX: usize = 5; // bytes storable inline

// The inline byte layout reads bytes directly out of the payload's little-endian
// image; a big-endian target would need the offsets flipped.  All supported
// targets (x86-64 / aarch64 on linux, macos, windows) are little-endian.
#[cfg(target_endian = "big")]
compile_error!("small-string optimisation assumes a little-endian target");

/// True when a TAG_STR value stores its bytes inline (see [`STR_INLINE_MARK`]).
/// Only meaningful once `top16(bits) == TAG_STR` has been established.
#[inline(always)]
fn str_is_inline_bits(bits: u64) -> bool {
    bits & STR_INLINE_MARK != 0
}

/// Encode `s` (which must be `≤ STR_INLINE_MAX` bytes) as an inline TAG_STR
/// value.  Zero allocations.
#[inline]
pub(crate) fn make_inline_str(s: &str) -> Value {
    let bytes = s.as_bytes();
    let len = bytes.len();
    debug_assert!(len <= STR_INLINE_MAX);
    let mut bits: u64 = TAG_STR_BITS | STR_INLINE_MARK | ((len as u64) << 1);
    for (i, &b) in bytes.iter().enumerate() {
        bits |= (b as u64) << (8 + i * 8);
    }
    Value::from_bits(bits)
}

/// Convert a ryu decimal string like `"0.00009999"` (or `"-0.00001"`) to
/// CPython-style scientific notation like `"9.999e-05"`.  Only called when
/// the value's absolute magnitude is known to be in (0, 1e-4), so the string
/// always starts with `"0.000"` (possibly with a leading `"-"`).
fn decimal_to_python_sci(s: &str) -> String {
    // Handle sign.
    let (neg, digits_str) = if let Some(d) = s.strip_prefix('-') {
        (true, d)
    } else {
        (false, s)
    };
    let sign_prefix = if neg { "-" } else { "" };

    // digits_str is like "0.00009999".  Split on '.'.
    // We know it's "0.<zeros><sig_digits>".
    let after_dot = digits_str
        .split_once('.')
        .map(|(_, frac)| frac)
        .unwrap_or(digits_str);

    // Count leading zeros to determine the exponent.
    let leading_zeros = after_dot.chars().take_while(|&c| c == '0').count();
    let exp = -(leading_zeros as i32 + 1);
    let sig = &after_dot[leading_zeros..]; // significant digits, no leading zeros

    // Build mantissa string: first digit, then '.' + rest (if any).
    let mantissa = if sig.len() <= 1 {
        sig.to_string()
    } else {
        format!("{}.{}", &sig[..1], &sig[1..])
    };

    let exp_abs = exp.unsigned_abs();
    let exp_sign = if exp < 0 { "-" } else { "+" };
    if exp_abs < 10 {
        format!("{sign_prefix}{mantissa}e{exp_sign}0{exp_abs}")
    } else {
        format!("{sign_prefix}{mantissa}e{exp_sign}{exp_abs}")
    }
}

fn format_float(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    // Use ryu for the shortest round-trip decimal representation.
    let raw = ryu::Buffer::new().format(v).to_string();
    // ryu omits the '+' in positive exponents and may use a single digit for the
    // exponent, e.g. "1e20" or "1e-5".  CPython always writes "1e+20" and "1e-05"
    // (sign present, exponent at least two digits).  Normalise to match.
    if let Some(e_pos) = raw.find('e') {
        let mantissa = &raw[..e_pos];
        let exp_str = &raw[e_pos + 1..]; // everything after 'e'
        let (sign, digits) = if let Some(d) = exp_str.strip_prefix('-') {
            ("-", d)
        } else if let Some(d) = exp_str.strip_prefix('+') {
            ("+", d)
        } else {
            ("+", exp_str)
        };
        // Pad the exponent to at least two digits.
        return if digits.len() < 2 {
            format!("{mantissa}e{sign}0{digits}")
        } else {
            format!("{mantissa}e{sign}{digits}")
        };
    }
    // No scientific notation in ryu output (e.g. "0.0001", "1.1", "1.0").
    // CPython uses scientific notation when 0 < abs(v) < 1e-4, but ryu may
    // produce a plain decimal string for those values (e.g. "0.00001" for 1e-5).
    // Detect this case and reformat as scientific to match CPython.
    let abs_v = v.abs();
    if abs_v != 0.0 && abs_v < 1e-4 {
        // ryu produced a decimal string (e.g. "0.00009999") for a value that
        // CPython would render in scientific notation (e.g. "9.999e-05").
        // Parse ryu's decimal string to extract the significant digits and
        // exponent, then re-emit in CPython's exponent format.
        return decimal_to_python_sci(&raw);
    }
    // ryu guarantees a decimal point for non-integer floats, and always includes
    // ".0" for integer-valued floats, so no further fixup is needed.
    raw
}

/// Returns the top 16 bits of a Value's u64 encoding — the tag.
///
/// **Caveat:** the sentinels [`UNSET_BITS`] and [`NOT_IMPLEMENTED_BITS`] are
/// positive-NaN bit patterns whose `top16` is `0x7FF8` (≤ [`TAG_FLOAT_MAX`]).
/// They will be classified as `Float` by a raw `top16`-based check.  Always
/// route through [`Value::kind`] (which checks the exact bit pattern first),
/// [`Value::is_unset`], or [`Value::is_not_implemented`] when distinguishing
/// these from real floats.
#[inline(always)]
fn top16(bits: u64) -> u16 {
    (bits >> 48) as u16
}
