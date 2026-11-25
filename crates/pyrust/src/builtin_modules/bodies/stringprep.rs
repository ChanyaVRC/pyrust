// `stringprep` module — 100% native-Rust implementation of Python 3.12's
// RFC 3454 StringPrep tables (issue #3250).
//
// Exposes all 19 CPython 3.12 RFC 3454 predicates and mapping functions
// (`in_table_a1`, `in_table_b1`, `map_table_b2`, `map_table_b3`,
// `in_table_c11`, `in_table_c12`, `in_table_c11_c12`, `in_table_c21`,
// `in_table_c22`, `in_table_c21_c22`, `in_table_c3`, `in_table_c4`,
// `in_table_c5`, `in_table_c6`, `in_table_c7`, `in_table_c8`, `in_table_c9`,
// `in_table_d1`, `in_table_d2`), plus the underlying table constants
// (`b1_set`, `b3_exceptions`, `c22_specials`, `c6_set`, `c7_set`, `c8_set`,
// `c9_set`).
//
// Reference: <https://docs.python.org/3.12/library/stringprep.html>

use pyrust_core::{cesu8_codepoints, cesu8_encode_codepoint};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{ExpandedCallArg, builtin_data_backing, value_type_name_str};
use crate::value::{PyDict, PyKey, PySet, Value};

#[path = "stringprep/tables.rs"]
mod tables;

use self::tables::{
    B2_OVERRIDES, B3_EXCEPTIONS, C22_SPECIALS, TABLE_A1, TABLE_B1, TABLE_C3, TABLE_C4, TABLE_C6,
    TABLE_C7, TABLE_C8, TABLE_C9, TABLE_C12, TABLE_C21, TABLE_C22, TABLE_D1, TABLE_D2,
};

#[inline]
fn in_ranges(ranges: &[(u32, u32)], cp: u32) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if cp < lo {
                std::cmp::Ordering::Greater
            } else if cp > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn extract_str(val: &Value) -> Option<&str> {
    if let Some(s) = val.as_str() {
        return Some(s);
    }
    None
}

fn extract_single_cp_ord(val: &Value) -> Result<u32> {
    let owned: String;
    let s = if let Some(s) = extract_str(val) {
        s
    } else if let Some(backing) = builtin_data_backing(val)
        && let Some(s) = backing.as_str()
    {
        owned = s.to_owned();
        &owned
    } else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "ord() expected string of length 1, but {} found",
                value_type_name_str(val)
            ),
        ));
    };
    let mut cps = cesu8_codepoints(s);
    match (cps.next(), cps.next()) {
        (Some(cp), None) => Ok(cp),
        _ => {
            let count = cesu8_codepoints(s).count();
            Err(PyError::named(
                "TypeError",
                format!("ord() expected a character, but string of length {count} found"),
            ))
        }
    }
}

fn extract_single_cp_ucd(fn_name: &str, val: &Value) -> Result<u32> {
    let owned: String;
    let s = if let Some(s) = extract_str(val) {
        s
    } else if let Some(backing) = builtin_data_backing(val)
        && let Some(s) = backing.as_str()
    {
        owned = s.to_owned();
        &owned
    } else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() argument must be a unicode character, not {}",
                value_type_name_str(val)
            ),
        ));
    };
    let mut cps = cesu8_codepoints(s);
    match (cps.next(), cps.next()) {
        (Some(cp), None) => Ok(cp),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() argument must be a unicode character, not str"),
        )),
    }
}

fn set_from_ranges(ranges: &[(u32, u32)]) -> Value {
    let mut set = PySet::default();
    for &(lo, hi) in ranges {
        for cp in lo..=hi {
            set.insert(PyKey::Int(i64::from(cp)));
        }
    }
    Value::set(set)
}

fn set_from_slice(items: &[u32]) -> Value {
    let mut set = PySet::default();
    for &cp in items {
        set.insert(PyKey::Int(i64::from(cp)));
    }
    Value::set(set)
}

fn b3_exceptions_dict() -> Value {
    let mut dict = PyDict::default();
    for &(cp, mapped) in B3_EXCEPTIONS {
        dict.insert(PyKey::Int(i64::from(cp)), Value::string(mapped));
    }
    Value::dict(dict)
}

// ── Pure-Rust RFC 3454 helpers (shared with `encodings.idna`) ────────────────

#[inline]
pub(crate) fn in_table_a1_cp(cp: u32) -> bool {
    in_ranges(TABLE_A1, cp)
}

#[inline]
pub(crate) fn in_table_b1_cp(cp: u32) -> bool {
    in_ranges(TABLE_B1, cp)
}

#[inline]
pub(crate) fn map_table_b3_cp(cp: u32) -> String {
    if let Ok(idx) = B3_EXCEPTIONS.binary_search_by_key(&cp, |&(k, _)| k) {
        return B3_EXCEPTIONS[idx].1.to_string();
    }
    if let Some(ch) = char::from_u32(cp) {
        ch.to_lowercase().collect()
    } else {
        cesu8_encode_codepoint(cp)
    }
}

#[inline]
pub(crate) fn map_table_b2_cp(cp: u32) -> String {
    if let Ok(idx) = B2_OVERRIDES.binary_search_by_key(&cp, |&(k, _)| k) {
        return B2_OVERRIDES[idx].1.to_string();
    }
    map_table_b3_cp(cp)
}

#[inline]
pub(crate) fn in_table_c12_cp(cp: u32) -> bool {
    in_ranges(TABLE_C12, cp)
}

#[inline]
pub(crate) fn in_table_c21_cp(cp: u32) -> bool {
    in_ranges(TABLE_C21, cp)
}

#[inline]
pub(crate) fn in_table_c22_cp(cp: u32) -> bool {
    in_ranges(TABLE_C22, cp)
}

#[inline]
pub(crate) fn in_table_c3_cp(cp: u32) -> bool {
    in_ranges(TABLE_C3, cp)
}

#[inline]
pub(crate) fn in_table_c4_cp(cp: u32) -> bool {
    in_ranges(TABLE_C4, cp)
}

#[inline]
pub(crate) fn in_table_c5_cp(cp: u32) -> bool {
    (0xD800..=0xDFFF).contains(&cp)
}

#[inline]
pub(crate) fn in_table_c6_cp(cp: u32) -> bool {
    in_ranges(TABLE_C6, cp)
}

#[inline]
pub(crate) fn in_table_c7_cp(cp: u32) -> bool {
    in_ranges(TABLE_C7, cp)
}

#[inline]
pub(crate) fn in_table_c8_cp(cp: u32) -> bool {
    in_ranges(TABLE_C8, cp)
}

#[inline]
pub(crate) fn in_table_c9_cp(cp: u32) -> bool {
    in_ranges(TABLE_C9, cp)
}

#[inline]
pub(crate) fn in_table_d1_cp(cp: u32) -> bool {
    in_ranges(TABLE_D1, cp)
}

#[inline]
pub(crate) fn in_table_d2_cp(cp: u32) -> bool {
    in_ranges(TABLE_D2, cp)
}

pyrust_module! {
    constants {
        "b1_set" => set_from_ranges(TABLE_B1),
        "b3_exceptions" => b3_exceptions_dict(),
        "c22_specials" => set_from_slice(C22_SPECIALS),
        "c6_set" => set_from_ranges(TABLE_C6),
        "c7_set" => set_from_ranges(TABLE_C7),
        "c8_set" => set_from_ranges(TABLE_C8),
        "c9_set" => set_from_ranges(TABLE_C9),
    }

    fn in_table_a1(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ucd("category", code.0)?;
        Ok(Value::bool_(in_table_a1_cp(cp)))
    }

    fn in_table_b1(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::bool_(in_table_b1_cp(cp)))
    }

    fn map_table_b3(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::string(map_table_b3_cp(cp)))
    }

    fn map_table_b2(a: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(a.0)?;
        Ok(Value::string(map_table_b2_cp(cp)))
    }

    fn in_table_c11(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let is_space = if let Some(s) = code.0.as_str() {
            s == " "
        } else if let Some(backing) = builtin_data_backing(code.0) {
            backing.as_str() == Some(" ")
        } else {
            false
        };
        Ok(Value::bool_(is_space))
    }

    fn in_table_c12(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ucd("category", code.0)?;
        Ok(Value::bool_(in_table_c12_cp(cp)))
    }

    fn in_table_c11_c12(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ucd("category", code.0)?;
        Ok(Value::bool_(cp == 0x20 || in_table_c12_cp(cp)))
    }

    fn in_table_c21(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::bool_(in_table_c21_cp(cp)))
    }

    fn in_table_c22(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::bool_(in_table_c22_cp(cp)))
    }

    fn in_table_c21_c22(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ucd("category", code.0)?;
        Ok(Value::bool_(in_table_c21_cp(cp) || in_table_c22_cp(cp)))
    }

    fn in_table_c3(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ucd("category", code.0)?;
        Ok(Value::bool_(in_table_c3_cp(cp)))
    }

    fn in_table_c4(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::bool_(in_table_c4_cp(cp)))
    }

    fn in_table_c5(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ucd("category", code.0)?;
        Ok(Value::bool_(in_table_c5_cp(cp)))
    }

    fn in_table_c6(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::bool_(in_table_c6_cp(cp)))
    }

    fn in_table_c7(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::bool_(in_table_c7_cp(cp)))
    }

    fn in_table_c8(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::bool_(in_table_c8_cp(cp)))
    }

    fn in_table_c9(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ord(code.0)?;
        Ok(Value::bool_(in_table_c9_cp(cp)))
    }

    fn in_table_d1(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ucd("bidirectional", code.0)?;
        Ok(Value::bool_(in_table_d1_cp(cp)))
    }

    fn in_table_d2(code: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let cp = extract_single_cp_ucd("bidirectional", code.0)?;
        Ok(Value::bool_(in_table_d2_cp(cp)))
    }
}
