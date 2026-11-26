// `unicodedata` module — 100% native Rust (issues #3203, #3323).
//
// Exposes the Unicode 15.0.0 character database matching CPython 3.12:
// `unidata_version`, `category`, `bidirectional`, `combining`,
// `east_asian_width`, `mirrored`, `decomposition`, `decimal`, `digit`,
// `numeric`, `name`, `lookup`, `normalize`, `is_normalized`, plus the `UCD`
// class and its `ucd_3_2_0` instance.  There is no Python-source injection:
// `UCD` is a native-state class declared in the `pyrust_module!` block whose
// methods share the module functions' implementations.
//
// Argument handling mirrors CPython's `Modules/unicodedata.c` (Argument
// Clinic): positional-only, no keywords, and the exact C-level wording for
// arity and type errors, with the `UCD.` qualifier on the method forms.
//
// Reference: <https://docs.python.org/3/library/unicodedata.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::builtin_modules::macros::native_state::{has_native_state, set_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, error_type_name_str, value_type_name_str,
};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};
use pyrust_core::{PyErrorKind, cesu8_codepoints};
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

/// Database version reported by `unicodedata.ucd_3_2_0` (CPython keeps a
/// delta-filtered view of the 3.2.0 database; PyRust reuses the 15.0.0 tables
/// and only reproduces the object identity and `unidata_version`).
const UCD_3_2_0_VERSION: &str = "3.2.0";

/// Qualifier prefix of arity errors raised by the module-level functions.
const QUAL_MODULE: &str = "unicodedata.";
/// Qualifier prefix of arity errors raised by `UCD` methods.
const QUAL_UCD: &str = "UCD.";

/// Native payload of a `UCD` instance.
struct UcdState {
    version: String,
}

// ── Lookup & argument helpers ────────────────────────────────────────────────

#[inline]
fn lookup_record(cp: u32) -> PropRecord {
    let pos = PROP_RUNS.partition_point(|&entry| (entry >> 11) <= cp);
    let idx = if pos == 0 { 0 } else { pos - 1 };
    let rec_idx = (PROP_RUNS[idx] & 0x7FF) as usize;
    PROP_RECORDS[rec_idx]
}

/// Run `f` on the text of a `str` (or `str` subclass) value.
fn with_str<R>(val: &Value, f: impl FnOnce(&str) -> R) -> Option<R> {
    if let Some(s) = val.as_str() {
        return Some(f(s));
    }
    let backing = builtin_data_backing(val)?;
    backing.as_str().map(f)
}

fn extract_str(val: &Value) -> Option<String> {
    with_str(val, str::to_owned)
}

/// Type name as CPython's `_PyArg_BadArgument` prints it: `None` is spelled
/// `None` and static stdlib types are module-qualified (`array.array`).
fn arg_type_name(val: &Value) -> String {
    if matches!(val.kind(), ValueKind::None) {
        "None".to_string()
    } else {
        error_type_name_str(val).into_owned()
    }
}

/// Positional-only single-character argument (`Py_UCS4` converter).
/// `label` is `"argument"` for the one-parameter functions and `"argument 1"`
/// for the ones that also take a `default`.
fn extract_single_codepoint(fn_short: &str, label: &str, val: &Value) -> Result<u32> {
    let single = with_str(val, |s| {
        let mut cps = cesu8_codepoints(s);
        match (cps.next(), cps.next()) {
            (Some(cp), None) => Some(cp),
            _ => None,
        }
    });
    match single {
        Some(Some(cp)) => Ok(cp),
        Some(None) => Err(not_a_character(fn_short, label, "str")),
        None => Err(not_a_character(fn_short, label, &arg_type_name(val))),
    }
}

fn not_a_character(fn_short: &str, label: &str, type_name: &str) -> PyError {
    PyError::named(
        "TypeError",
        format!("{fn_short}() {label} must be a unicode character, not {type_name}"),
    )
}

fn reject_keywords(qual: &str, fn_short: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if args.iter().any(|a| a.name.is_some()) {
        return Err(PyError::named(
            "TypeError",
            format!("{qual}{fn_short}() takes no keyword arguments"),
        ));
    }
    Ok(())
}

/// `METH_O`-style arity: exactly one positional argument.
fn check_arity_1(qual: &str, fn_short: &str, args: &[ExpandedCallArg]) -> Result<()> {
    reject_keywords(qual, fn_short, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{qual}{fn_short}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    Ok(())
}

/// Clinic arity for `(chr, default=<unrepresentable>, /)`.
fn check_arity_1_or_2(qual: &str, fn_short: &str, args: &[ExpandedCallArg]) -> Result<()> {
    reject_keywords(qual, fn_short, args)?;
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_short} expected at least 1 argument, got 0"),
        ));
    }
    if args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_short} expected at most 2 arguments, got {}",
                args.len()
            ),
        ));
    }
    Ok(())
}

/// Clinic arity for `(form, unistr, /)`.
fn check_arity_2(qual: &str, fn_short: &str, args: &[ExpandedCallArg]) -> Result<()> {
    reject_keywords(qual, fn_short, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_short} expected 2 arguments, got {}", args.len()),
        ));
    }
    Ok(())
}

/// Normalize `s` using Unicode NFKC (shared with `encodings.idna`).
pub(crate) fn normalize_nfkc(s: &str) -> String {
    normalize::normalize(NormForm::Nfkc, s)
}

/// Unicode 15.0.0 decimal digit value of `cp` (`Py_UNICODE_TODECIMAL`), or
/// `None` when it is not a decimal digit (shared with `_string`).
pub(crate) fn decimal_value(cp: u32) -> Option<u32> {
    u32::try_from(lookup_record(cp).dec).ok()
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

/// Single-character argument of the one-parameter property functions.
fn one_codepoint(qual: &str, fn_short: &str, args: &[ExpandedCallArg]) -> Result<u32> {
    check_arity_1(qual, fn_short, args)?;
    extract_single_codepoint(fn_short, "argument", &args[0].value)
}

/// `(chr, default=<unrepresentable>, /)` argument shape.
fn codepoint_and_default<'a>(
    qual: &str,
    fn_short: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(u32, Option<&'a Value>)> {
    check_arity_1_or_2(qual, fn_short, args)?;
    let cp = extract_single_codepoint(fn_short, "argument 1", &args[0].value)?;
    Ok((cp, args.get(1).map(|a| &a.value)))
}

/// Parse `(form, unistr, /)` shared by `normalize` / `is_normalized`.
fn form_and_text(
    qual: &str,
    fn_short: &str,
    args: &[ExpandedCallArg],
) -> Result<(NormForm, String)> {
    check_arity_2(qual, fn_short, args)?;
    let Some(form_str) = extract_str(&args[0].value) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_short}() argument 1 must be str, not {}",
                arg_type_name(&args[0].value)
            ),
        ));
    };
    let Some(unistr) = extract_str(&args[1].value) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_short}() argument 2 must be str, not {}",
                arg_type_name(&args[1].value)
            ),
        ));
    };
    let Some(form) = NormForm::parse(&form_str) else {
        return Err(PyError::named(
            "ValueError",
            "invalid normalization form".to_string(),
        ));
    };
    Ok((form, unistr))
}

fn category_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let cp = one_codepoint(qual, "category", args)?;
    Ok(Value::string(
        CATEGORY_NAMES[lookup_record(cp).cat as usize],
    ))
}

fn bidirectional_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let cp = one_codepoint(qual, "bidirectional", args)?;
    Ok(Value::string(BIDI_NAMES[lookup_record(cp).bidi as usize]))
}

fn combining_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let cp = one_codepoint(qual, "combining", args)?;
    Ok(Value::int(i64::from(lookup_record(cp).comb)))
}

fn east_asian_width_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let cp = one_codepoint(qual, "east_asian_width", args)?;
    Ok(Value::string(EAW_NAMES[lookup_record(cp).eaw as usize]))
}

fn mirrored_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let cp = one_codepoint(qual, "mirrored", args)?;
    Ok(Value::int(i64::from(lookup_record(cp).mir)))
}

fn decomposition_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let cp = one_codepoint(qual, "decomposition", args)?;
    Ok(Value::string(format_decomposition(cp)))
}

fn decimal_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let (cp, default) = codepoint_and_default(qual, "decimal", args)?;
    let rec = lookup_record(cp);
    match default {
        _ if rec.dec >= 0 => Ok(Value::int(i64::from(rec.dec))),
        Some(d) => Ok(d.clone()),
        None => Err(PyError::named("ValueError", "not a decimal".to_string())),
    }
}

fn digit_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let (cp, default) = codepoint_and_default(qual, "digit", args)?;
    let rec = lookup_record(cp);
    match default {
        _ if rec.dig >= 0 => Ok(Value::int(i64::from(rec.dig))),
        Some(d) => Ok(d.clone()),
        None => Err(PyError::named("ValueError", "not a digit".to_string())),
    }
}

fn numeric_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let (cp, default) = codepoint_and_default(qual, "numeric", args)?;
    let rec = lookup_record(cp);
    match default {
        _ if rec.num_idx > 0 => Ok(Value::float(NUMERIC_VALUES[(rec.num_idx - 1) as usize])),
        Some(d) => Ok(d.clone()),
        None => Err(PyError::named(
            "ValueError",
            "not a numeric character".to_string(),
        )),
    }
}

fn name_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let (cp, default) = codepoint_and_default(qual, "name", args)?;
    match (unicode15_name(cp), default) {
        (Some(n), _) => Ok(Value::string(n)),
        (None, Some(d)) => Ok(d.clone()),
        (None, None) => Err(PyError::named("ValueError", "no such name".to_string())),
    }
}

/// The `s#` converter of `unicodedata.lookup`: `str` (UTF-8) and read-only
/// `bytes` are accepted.  Returns the search key and the C-string spelling
/// CPython embeds in the `KeyError` message (cut at the first NUL).
fn lookup_query(val: &Value) -> Result<(String, String)> {
    let message_text = |text: &str| text.split('\0').next().unwrap_or("").to_string();
    if let Some(text) = extract_str(val) {
        // `PyUnicode_AsUTF8AndSize` rejects lone surrogates.
        let is_surrogate = |cp: u32| (0xD800..0xE000).contains(&cp);
        let surrogates = {
            let mut cps = cesu8_codepoints(&text).enumerate();
            cps.find(|&(_, cp)| is_surrogate(cp)).map(|(start, _)| {
                (
                    start,
                    start + 1 + cps.take_while(|&(_, cp)| is_surrogate(cp)).count(),
                )
            })
        };
        if let Some((start, end)) = surrogates {
            return Err(PyError::new(PyErrorKind::UnicodeEncodeError {
                encoding: "utf-8".to_string(),
                object: text,
                start,
                end,
                reason: "surrogates not allowed".to_string(),
            }));
        }
        let shown = message_text(&text);
        return Ok((text, shown));
    }
    let bytes = match val.kind() {
        ValueKind::Bytes(rc) => Some(rc.to_vec()),
        _ => builtin_data_backing(val).and_then(|b| match b.kind() {
            ValueKind::Bytes(rc) => Some(rc.to_vec()),
            _ => None,
        }),
    };
    if let Some(bytes) = bytes {
        let key = bytes.iter().map(|&b| char::from(b)).collect();
        let shown = message_text(&String::from_utf8_lossy(&bytes));
        return Ok((key, shown));
    }
    if pyrust_builtins::bytearray::as_bytearray_snapshot(val).is_some()
        || pyrust_builtins::memoryview::is_memoryview(val)
        || error_type_name_str(val) == "array.array"
    {
        return Err(PyError::named(
            "TypeError",
            format!(
                "lookup() argument must be read-only bytes-like object, not {}",
                arg_type_name(val)
            ),
        ));
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "a bytes-like object is required, not '{}'",
            value_type_name_str(val)
        ),
    ))
}

fn lookup_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    check_arity_1(qual, "lookup", args)?;
    let (key, shown) = lookup_query(&args[0].value)?;
    match unicode15_lookup(&key) {
        Some(ch) => Ok(Value::string(ch.to_string())),
        None => Err(PyError::named(
            "KeyError",
            format!("undefined character name '{shown}'"),
        )),
    }
}

fn normalize_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let (form, unistr) = form_and_text(qual, "normalize", args)?;
    Ok(Value::string(normalize::normalize(form, &unistr)))
}

fn is_normalized_impl(qual: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let (form, unistr) = form_and_text(qual, "is_normalized", args)?;
    Ok(Value::bool_(normalize::is_normalized(form, &unistr)))
}

/// Split the bound `self` off a `UCD` method call, checking it is a `UCD`
/// (the descriptor check CPython's method descriptors perform).
fn ucd_method_args<'a>(method: &str, args: &'a [ExpandedCallArg]) -> Result<&'a [ExpandedCallArg]> {
    match args.split_first() {
        Some((this, rest)) if this.name.is_none() && has_native_state::<UcdState>(&this.value) => {
            Ok(rest)
        }
        Some((this, _)) if this.name.is_none() => Err(PyError::named(
            "TypeError",
            format!(
                "descriptor '{method}' for 'unicodedata.UCD' objects doesn't apply to a '{}' object",
                value_type_name_str(&this.value)
            ),
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!("unbound method UCD.{method}() needs an argument"),
        )),
    }
}

/// Error for assigning or deleting any attribute of a `UCD` instance.
fn attribute_write_error(args: &[ExpandedCallArg]) -> PyError {
    let name = args.get(1).and_then(|a| extract_str(&a.value));
    match name.as_deref() {
        Some("unidata_version") => {
            PyError::named("AttributeError", "readonly attribute".to_string())
        }
        Some(name) => PyError::named(
            "AttributeError",
            format!("'unicodedata.UCD' object has no attribute '{name}'"),
        ),
        None => PyError::named("TypeError", "attribute name must be string".to_string()),
    }
}

/// Install `ucd_3_2_0`, the `UCD` instance CPython exposes for the 3.2.0
/// database (used by IDNA / stringprep).  `UCD` itself cannot be instantiated
/// from Python, so the instance is built here around the native payload.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let class: Option<Rc<RefCell<PyClass>>> = match mod_rc.borrow().attrs.get("UCD") {
        Some(v) => match v.kind() {
            ValueKind::PyClass(rc) => Some(Rc::clone(rc)),
            _ => None,
        },
        None => None,
    };
    let Some(class) = class else {
        return;
    };
    {
        // CPython's `UCD.__doc__` is `None`.
        let mut class = class.borrow_mut();
        class
            .attrs
            .entry("__doc__".to_string())
            .or_insert_with(Value::none);
    }
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })));
    let state = UcdState {
        version: UCD_3_2_0_VERSION.to_string(),
    };
    if set_native_state(&inst, state).is_ok() {
        mod_rc
            .borrow_mut()
            .attrs
            .insert("ucd_3_2_0".to_string(), inst);
    }
}

pyrust_module! {
    constants {
        "unidata_version" => Value::string(UNIDATA_VERSION),
        "__doc__" => Value::string(concat!(
            "This module provides access to the Unicode Character Database which
",
            "defines character properties for all Unicode characters. The data in
",
            "this database is based on the UnicodeData.txt file version
",
            "15.0.0 which is publicly available from ftp://ftp.unicode.org/.
",
            "
",
            "The module uses the same names and symbols as defined by the
",
            "UnicodeData File Format 15.0.0.",
        )),
    }

    fn category(args) -> Result<Value> {
        let _ = _interp;
        category_impl(QUAL_MODULE, args)
    }

    fn bidirectional(args) -> Result<Value> {
        let _ = _interp;
        bidirectional_impl(QUAL_MODULE, args)
    }

    fn combining(args) -> Result<Value> {
        let _ = _interp;
        combining_impl(QUAL_MODULE, args)
    }

    fn east_asian_width(args) -> Result<Value> {
        let _ = _interp;
        east_asian_width_impl(QUAL_MODULE, args)
    }

    fn mirrored(args) -> Result<Value> {
        let _ = _interp;
        mirrored_impl(QUAL_MODULE, args)
    }

    fn decomposition(args) -> Result<Value> {
        let _ = _interp;
        decomposition_impl(QUAL_MODULE, args)
    }

    fn decimal(args) -> Result<Value> {
        let _ = _interp;
        decimal_impl(QUAL_MODULE, args)
    }

    fn digit(args) -> Result<Value> {
        let _ = _interp;
        digit_impl(QUAL_MODULE, args)
    }

    fn numeric(args) -> Result<Value> {
        let _ = _interp;
        numeric_impl(QUAL_MODULE, args)
    }

    fn name(args) -> Result<Value> {
        let _ = _interp;
        name_impl(QUAL_MODULE, args)
    }

    fn lookup(args) -> Result<Value> {
        let _ = _interp;
        lookup_impl(QUAL_MODULE, args)
    }

    fn normalize(args) -> Result<Value> {
        let _ = _interp;
        normalize_impl(QUAL_MODULE, args)
    }

    fn is_normalized(args) -> Result<Value> {
        let _ = _interp;
        is_normalized_impl(QUAL_MODULE, args)
    }

    // Unicode Character Database view; `ucd_3_2_0` is the only instance and
    // the type cannot be instantiated from Python (as in CPython).
    #[native_state(UcdState)]
    class UCD {
        fn __new__(args) -> Result<Value> {
            let _ = (_interp, args);
            Err(PyError::named(
                "TypeError",
                "cannot create 'unicodedata.UCD' instances".to_string(),
            ))
        }

        // `object.__reduce_ex__` refuses C types without pickle support.
        fn __reduce_ex__(args) -> Result<Value> {
            let _ = (_interp, args);
            Err(PyError::named(
                "TypeError",
                "cannot pickle 'unicodedata.UCD' object".to_string(),
            ))
        }

        #[getter]
        fn unidata_version(#[state] state: &UcdState) -> Result<Value> {
            Ok(Value::string(state.version.as_str()))
        }

        // `UCD` instances have no `__dict__` and one read-only member.
        fn __setattr__(args) -> Result<Value> {
            let _ = _interp;
            Err(attribute_write_error(args))
        }

        fn __delattr__(args) -> Result<Value> {
            let _ = _interp;
            Err(attribute_write_error(args))
        }

        fn category(args) -> Result<Value> {
            let _ = _interp;
            category_impl(QUAL_UCD, ucd_method_args("category", args)?)
        }

        fn bidirectional(args) -> Result<Value> {
            let _ = _interp;
            bidirectional_impl(QUAL_UCD, ucd_method_args("bidirectional", args)?)
        }

        fn combining(args) -> Result<Value> {
            let _ = _interp;
            combining_impl(QUAL_UCD, ucd_method_args("combining", args)?)
        }

        fn east_asian_width(args) -> Result<Value> {
            let _ = _interp;
            east_asian_width_impl(QUAL_UCD, ucd_method_args("east_asian_width", args)?)
        }

        fn mirrored(args) -> Result<Value> {
            let _ = _interp;
            mirrored_impl(QUAL_UCD, ucd_method_args("mirrored", args)?)
        }

        fn decomposition(args) -> Result<Value> {
            let _ = _interp;
            decomposition_impl(QUAL_UCD, ucd_method_args("decomposition", args)?)
        }

        fn decimal(args) -> Result<Value> {
            let _ = _interp;
            decimal_impl(QUAL_UCD, ucd_method_args("decimal", args)?)
        }

        fn digit(args) -> Result<Value> {
            let _ = _interp;
            digit_impl(QUAL_UCD, ucd_method_args("digit", args)?)
        }

        fn numeric(args) -> Result<Value> {
            let _ = _interp;
            numeric_impl(QUAL_UCD, ucd_method_args("numeric", args)?)
        }

        fn name(args) -> Result<Value> {
            let _ = _interp;
            name_impl(QUAL_UCD, ucd_method_args("name", args)?)
        }

        fn lookup(args) -> Result<Value> {
            let _ = _interp;
            lookup_impl(QUAL_UCD, ucd_method_args("lookup", args)?)
        }

        fn normalize(args) -> Result<Value> {
            let _ = _interp;
            normalize_impl(QUAL_UCD, ucd_method_args("normalize", args)?)
        }

        fn is_normalized(args) -> Result<Value> {
            let _ = _interp;
            is_normalized_impl(QUAL_UCD, ucd_method_args("is_normalized", args)?)
        }
    }
}
