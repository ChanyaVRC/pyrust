// `unicodedata` module — body for the `unicodedata @inject` entry in
// `pyrust_builtin_modules!` (issue #3203).
//
// Exposes the Unicode 15.0.0 character database matching CPython 3.12:
// `unidata_version`, `category`, `bidirectional`, `combining`,
// `east_asian_width`, `mirrored`, `decomposition`, `decimal`, `digit`,
// `numeric`, `name`, `lookup`, `normalize`, `is_normalized`, plus `UCD` and
// `ucd_3_2_0` injected from `unicodedata_py.py`.
//
// Reference: <https://docs.python.org/3/library/unicodedata.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
    value_type_name_str,
};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_core::cesu8_codepoints;
use pyrust_derive::pyrust_module;

#[path = "unicodedata/normalize.rs"]
mod normalize;
#[path = "unicodedata/tables.rs"]
mod tables;

use self::normalize::NormForm;
use self::tables::{
    BIDI_NAMES, CATEGORY_NAMES, COMPAT_TAGS, DECOMP_DATA, DECOMP_INDEX, EAW_NAMES, NAME_ALIASES,
    NUMERIC_VALUES, PROP_RECORDS, PROP_RUNS, PropRecord, UNIDATA_VERSION,
};

const UNICODEDATA_PY_SOURCE: &str = include_str!("unicodedata_py.py");

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    // Expose the native module functions into the exec namespace so `UCD` can
    // delegate cleanly without keeping private helpers on the module.
    {
        let module_ref = module.borrow();
        for key in [
            "category",
            "bidirectional",
            "combining",
            "east_asian_width",
            "mirrored",
            "decomposition",
            "decimal",
            "digit",
            "numeric",
            "name",
            "lookup",
            "normalize",
            "is_normalized",
            "unidata_version",
        ] {
            if let Some(val) = module_ref.attrs.get(key) {
                ns.dict_insert(PyKey::str_from(key), val.clone())?;
            }
        }
    }
    interp.exec_source(UNICODEDATA_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("unicodedata: exec namespace not a dict".into()))?;
    for name in ["UCD", "ucd_3_2_0"] {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("unicodedata"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Lookup & argument helpers ────────────────────────────────────────────────

#[inline]
fn lookup_record(cp: u32) -> PropRecord {
    let pos = PROP_RUNS.partition_point(|&entry| (entry >> 11) <= cp);
    let idx = if pos == 0 { 0 } else { pos - 1 };
    let rec_idx = (PROP_RUNS[idx] & 0x7FF) as usize;
    PROP_RECORDS[rec_idx]
}

fn extract_str(val: &Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        return Some(s.to_owned());
    }
    if let Some(backing) = builtin_data_backing(val)
        && let Some(s) = backing.as_str()
    {
        return Some(s.to_owned());
    }
    None
}

fn extract_single_codepoint(fn_short: &str, val: &Value) -> Result<u32> {
    let Some(s) = extract_str(val) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_short}() argument must be a unicode character, not {}",
                value_type_name_str(val)
            ),
        ));
    };
    let mut cps = cesu8_codepoints(&s);
    match (cps.next(), cps.next()) {
        (Some(cp), None) => Ok(cp),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_short}() argument must be a unicode character, not str"),
        )),
    }
}

fn check_arity_1(fn_short: &str, args: &[ExpandedCallArg]) -> Result<()> {
    reject_keyword_args_expanded(fn_short, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "unicodedata.{fn_short}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    Ok(())
}

fn check_arity_1_or_2(fn_short: &str, args: &[ExpandedCallArg]) -> Result<()> {
    reject_keyword_args_expanded(fn_short, args)?;
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_short}() takes at least 1 argument (0 given)"),
        ));
    }
    if args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_short}() takes at most 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    Ok(())
}

fn check_arity_2(fn_short: &str, args: &[ExpandedCallArg]) -> Result<()> {
    reject_keyword_args_expanded(fn_short, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "unicodedata.{fn_short}() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    Ok(())
}

/// Normalize `s` using Unicode NFKC (shared with `encodings.idna`).
pub(crate) fn normalize_nfkc(s: &str) -> String {
    normalize::normalize(NormForm::Nfkc, s)
}

/// Return the official Unicode 15.0.0 character name for `cp`, if any.
pub(crate) fn unicode15_name(cp: u32) -> Option<String> {
    let rec = lookup_record(cp);
    // Cc (25), Cs (27), Co (28), Cn (29) never have character names in Unicode 15.0.
    if matches!(rec.cat, 25 | 27 | 28 | 29) {
        return None;
    }
    let ch = char::from_u32(cp)?;
    unicode_names2::name(ch).map(|n| n.to_string())
}

/// Look up a Unicode 15.0.0 character by exact name or Name Alias
/// (case-insensitive on ASCII letters, matching CPython 3.12).
pub(crate) fn unicode15_lookup(query: &str) -> Option<char> {
    if query.is_empty() || !query.is_ascii() {
        return None;
    }
    let upper = query.to_ascii_uppercase();

    // 1. Official Unicode 15.0 character name (verify exact match to reject
    //    `unicode_names2`'s UAX44-LM2 loose matching and Unicode 16.0 additions).
    //    Match CPython 3.12: algorithmic CJK Unified Ideograph and Hangul
    //    Syllable names require exact uppercase, whereas named table entries
    //    are ASCII case-insensitive.
    if let Some(ch) = unicode_names2::character(&upper)
        && let Some(official) = unicode15_name(ch as u32)
    {
        let is_algorithmic = official.starts_with("CJK UNIFIED IDEOGRAPH-")
            || official.starts_with("HANGUL SYLLABLE ");
        if is_algorithmic {
            if query == official {
                return Some(ch);
            }
        } else if upper == official {
            return Some(ch);
        }
    }

    // 2. Unicode 15.0 Name Aliases (control, correction, alternate, figment, abbreviation).
    if let Ok(idx) = NAME_ALIASES.binary_search_by_key(&upper.as_str(), |&(alias, _)| alias) {
        return char::from_u32(NAME_ALIASES[idx].1);
    }

    None
}

fn format_decomposition(cp: u32) -> String {
    let Ok(idx) = DECOMP_INDEX.binary_search_by_key(&cp, |&entry| (entry >> 24) as u32) else {
        return String::new();
    };
    let entry = DECOMP_INDEX[idx];
    let tag = ((entry >> 19) & 0x1F) as usize;
    let len = ((entry >> 14) & 0x1F) as usize;
    let offset = (entry & 0x3FFF) as usize;
    let mut out = String::new();
    if tag != 0 {
        out.push_str(COMPAT_TAGS[tag]);
    }
    for &c in &DECOMP_DATA[offset..offset + len] {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&format!("{c:04X}"));
    }
    out
}

pyrust_module! {
    constants {
        "unidata_version" => Value::string(UNIDATA_VERSION),
    }

    fn category(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1("category", args)?;
        let cp = extract_single_codepoint("category", &args[0].value)?;
        let rec = lookup_record(cp);
        Ok(Value::string(CATEGORY_NAMES[rec.cat as usize]))
    }

    fn bidirectional(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1("bidirectional", args)?;
        let cp = extract_single_codepoint("bidirectional", &args[0].value)?;
        let rec = lookup_record(cp);
        Ok(Value::string(BIDI_NAMES[rec.bidi as usize]))
    }

    fn combining(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1("combining", args)?;
        let cp = extract_single_codepoint("combining", &args[0].value)?;
        let rec = lookup_record(cp);
        Ok(Value::int(i64::from(rec.comb)))
    }

    fn east_asian_width(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1("east_asian_width", args)?;
        let cp = extract_single_codepoint("east_asian_width", &args[0].value)?;
        let rec = lookup_record(cp);
        Ok(Value::string(EAW_NAMES[rec.eaw as usize]))
    }

    fn mirrored(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1("mirrored", args)?;
        let cp = extract_single_codepoint("mirrored", &args[0].value)?;
        let rec = lookup_record(cp);
        Ok(Value::int(i64::from(rec.mir)))
    }

    fn decomposition(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1("decomposition", args)?;
        let cp = extract_single_codepoint("decomposition", &args[0].value)?;
        Ok(Value::string(format_decomposition(cp)))
    }

    fn decimal(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1_or_2("decimal", args)?;
        let cp = extract_single_codepoint("decimal", &args[0].value)?;
        let rec = lookup_record(cp);
        if rec.dec >= 0 {
            Ok(Value::int(i64::from(rec.dec)))
        } else if args.len() == 2 {
            Ok(args[1].value.clone())
        } else {
            Err(PyError::named("ValueError", "not a decimal".to_string()))
        }
    }

    fn digit(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1_or_2("digit", args)?;
        let cp = extract_single_codepoint("digit", &args[0].value)?;
        let rec = lookup_record(cp);
        if rec.dig >= 0 {
            Ok(Value::int(i64::from(rec.dig)))
        } else if args.len() == 2 {
            Ok(args[1].value.clone())
        } else {
            Err(PyError::named("ValueError", "not a digit".to_string()))
        }
    }

    fn numeric(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1_or_2("numeric", args)?;
        let cp = extract_single_codepoint("numeric", &args[0].value)?;
        let rec = lookup_record(cp);
        if rec.num_idx > 0 {
            Ok(Value::float(NUMERIC_VALUES[(rec.num_idx - 1) as usize]))
        } else if args.len() == 2 {
            Ok(args[1].value.clone())
        } else {
            Err(PyError::named(
                "ValueError",
                "not a numeric character".to_string(),
            ))
        }
    }

    fn name(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1_or_2("name", args)?;
        let cp = extract_single_codepoint("name", &args[0].value)?;
        if let Some(n) = unicode15_name(cp) {
            Ok(Value::string(n))
        } else if args.len() == 2 {
            Ok(args[1].value.clone())
        } else {
            Err(PyError::named("ValueError", "no such name".to_string()))
        }
    }

    fn lookup(args) -> Result<Value> {
        let _ = _interp;
        check_arity_1("lookup", args)?;
        let query = if let Some(s) = extract_str(&args[0].value) {
            s
        } else if let ValueKind::Bytes(rc) = args[0].value.kind() {
            rc.iter().map(|&b| char::from(b)).collect()
        } else if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value)
        {
            snap.into_iter().map(char::from).collect()
        } else if pyrust_builtins::memoryview::is_memoryview(&args[0].value) {
            let bytes = pyrust_builtins::memoryview::to_bytes_checked(&args[0].value)?;
            bytes.into_iter().map(char::from).collect()
        } else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    value_type_name_str(&args[0].value)
                ),
            ));
        };
        if let Some(ch) = unicode15_lookup(&query) {
            Ok(Value::string(ch.to_string()))
        } else {
            Err(PyError::named(
                "KeyError",
                format!("undefined character name '{query}'"),
            ))
        }
    }

    fn normalize(args) -> Result<Value> {
        let _ = _interp;
        check_arity_2("normalize", args)?;
        let Some(form_str) = extract_str(&args[0].value) else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "normalize() argument 1 must be str, not {}",
                    value_type_name_str(&args[0].value)
                ),
            ));
        };
        let Some(unistr) = extract_str(&args[1].value) else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "normalize() argument 2 must be str, not {}",
                    value_type_name_str(&args[1].value)
                ),
            ));
        };
        let Some(form) = NormForm::parse(&form_str) else {
            return Err(PyError::named(
                "ValueError",
                "invalid normalization form".to_string(),
            ));
        };
        Ok(Value::string(normalize::normalize(form, &unistr)))
    }

    fn is_normalized(args) -> Result<Value> {
        let _ = _interp;
        check_arity_2("is_normalized", args)?;
        let Some(form_str) = extract_str(&args[0].value) else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "is_normalized() argument 1 must be str, not {}",
                    value_type_name_str(&args[0].value)
                ),
            ));
        };
        let Some(unistr) = extract_str(&args[1].value) else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "is_normalized() argument 2 must be str, not {}",
                    value_type_name_str(&args[1].value)
                ),
            ));
        };
        let Some(form) = NormForm::parse(&form_str) else {
            return Err(PyError::named(
                "ValueError",
                "invalid normalization form".to_string(),
            ));
        };
        Ok(Value::bool_(normalize::is_normalized(form, &unistr)))
    }
}
