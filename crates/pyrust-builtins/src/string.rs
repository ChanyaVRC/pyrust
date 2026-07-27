use std::collections::HashSet;

use pyrust_core::{
    PyBigIntSign, PyDict, PyError, PyKey, Result, Value, ValueKind, builtin_type_name,
    cesu8_codepoints, cp_is_printable, expect_arg_count, extract_fill_char, extract_int,
    extract_optional_int, py_value_display_name,
};
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use crate::{
    method_signature::{KeywordPolicy, PositionalArity},
    unicode_data,
};

pub const TYPE_NAME: &str = "str";

/// Compute the byte offset of a subslice `sub` within its parent `parent`.
///
/// `sub` must be a contiguous subslice of `parent` (i.e. produced by Rust's
/// `split`, `split_whitespace`, `trim_*`, etc. applied to `parent`).  The
/// assertion is a safety net; in correct code it always holds.
#[inline(always)]
fn subslice_offset(parent: &str, sub: &str) -> usize {
    let off = sub.as_ptr() as usize - parent.as_ptr() as usize;
    debug_assert!(
        off + sub.len() <= parent.len(),
        "subslice_offset: sub ({off}..{}) is outside parent (..{})",
        off + sub.len(),
        parent.len()
    );
    off
}

/// Canonical list of method names dispatched by `call`.
/// Single source of truth for `has_method` and the drift-guard test.
///
/// Note: `format` is listed here so that `hasattr(s, "format")` and
/// `getattr(s, "format")` work correctly for str instances.  The bound-method
/// dispatch in `runtime/builtin_methods` intercepts `"format"` before the
/// interpreter-free call below and routes kwargs through the formatting
/// domain. `format_map` follows the same route.
pub const METHODS: &[&str] = &[
    "__iter__",
    "index",
    "count",
    "split",
    "rsplit",
    "join",
    "splitlines",
    "partition",
    "rpartition",
    "strip",
    "lstrip",
    "rstrip",
    "removeprefix",
    "removesuffix",
    "center",
    "ljust",
    "rjust",
    "zfill",
    "expandtabs",
    "upper",
    "lower",
    "casefold",
    "capitalize",
    "swapcase",
    "title",
    "find",
    "rfind",
    "rindex",
    "replace",
    "format",
    "format_map",
    "startswith",
    "endswith",
    "isdigit",
    "isalpha",
    "isalnum",
    "isspace",
    "isdecimal",
    "isnumeric",
    "islower",
    "isupper",
    "istitle",
    "isascii",
    "isidentifier",
    "isprintable",
    "encode",
    "translate",
    "__getnewargs__",
];

pub const CLASS_ATTRS: crate::primitive_class_attrs::PrimitiveClassAttrs =
    crate::primitive_class_attrs::PrimitiveClassAttrs::new(TYPE_NAME, METHODS)
        .with_native_static_methods(&["maketrans"])
        .with_flags(crate::primitive_class_attrs::PrimitiveClassFlags::NONE.with_new());

/// Returns `true` if `method` is the name of a built-in `str` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Positional signature for every public str method.
///
/// `format`, `format_map`, and `maketrans` are executed by interpreter-owned
/// adapters, but their signature policy remains here with the str method
/// table.
pub fn positional_arity(method: &str) -> Option<PositionalArity> {
    Some(match method {
        "__iter__" | "upper" | "lower" | "casefold" | "capitalize" | "swapcase" | "title"
        | "isdigit" | "isalpha" | "isalnum" | "isspace" | "isdecimal" | "isnumeric" | "islower"
        | "isupper" | "istitle" | "isascii" | "isidentifier" | "isprintable" | "__getnewargs__" => {
            PositionalArity::exact(0)
        }
        "join" | "partition" | "rpartition" | "removeprefix" | "removesuffix" | "zfill"
        | "format_map" | "translate" => PositionalArity::exact(1),
        "strip" | "lstrip" | "rstrip" => PositionalArity::range(0, 1),
        "splitlines" | "expandtabs" => PositionalArity::range_takes_at_most(0, 1),
        "center" | "ljust" | "rjust" => PositionalArity::range(1, 2),
        "index" | "count" | "find" | "rfind" | "rindex" | "startswith" | "endswith" => {
            PositionalArity::range(1, 3)
        }
        "split" | "rsplit" | "encode" => PositionalArity::range_takes_at_most(0, 2),
        "replace" => PositionalArity::range(2, 3),
        "format" => PositionalArity::variadic(0),
        "maketrans" => PositionalArity::range(1, 3),
        _ => return None,
    })
}

#[inline]
pub fn validate_method_positional_arity(method: &str, given: usize) -> Result<()> {
    if given == 0 {
        return Ok(());
    }
    match positional_arity(method) {
        Some(arity) => arity.reject_excess(TYPE_NAME, method, given),
        None => Ok(()),
    }
}

/// String methods whose accepted keywords are bound by the interpreter
/// adapter before entering the receiver-only implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeywordBinder {
    Split,
    RSplit,
    SplitLines,
    Encode,
    ExpandTabs,
}

impl KeywordBinder {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Split => "split",
            Self::RSplit => "rsplit",
            Self::SplitLines => "splitlines",
            Self::Encode => "encode",
            Self::ExpandTabs => "expandtabs",
        }
    }
}

/// Resolve the keyword-binding route without teaching the interpreter a
/// second list of concrete str method names.
pub fn keyword_binder(method: &str) -> Option<KeywordBinder> {
    match method {
        "split" => Some(KeywordBinder::Split),
        "rsplit" => Some(KeywordBinder::RSplit),
        "splitlines" => Some(KeywordBinder::SplitLines),
        "encode" => Some(KeywordBinder::Encode),
        "expandtabs" => Some(KeywordBinder::ExpandTabs),
        _ => None,
    }
}

pub fn keyword_policy(method: &str) -> Option<KeywordPolicy> {
    if method == "format" || keyword_binder(method).is_some() {
        return Some(KeywordPolicy::Accept);
    }
    positional_arity(method).map(|_| KeywordPolicy::Reject)
}

#[inline]
pub fn validate_method_keywords(method: &str, has_keywords: bool) -> Result<()> {
    if !has_keywords {
        return Ok(());
    }
    match keyword_policy(method) {
        Some(policy) => policy.validate(TYPE_NAME, method, true),
        None => Ok(()),
    }
}

/// Interpreter-owned route for string methods that cannot use `call`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterpreterMethod {
    Format,
    FormatMap,
    MakeTrans,
}

impl InterpreterMethod {
    const fn name(self) -> &'static str {
        match self {
            Self::Format => "format",
            Self::FormatMap => "format_map",
            Self::MakeTrans => "maketrans",
        }
    }
}

#[inline]
pub fn validate_interpreter_method_positional_arity(
    method: InterpreterMethod,
    given: usize,
) -> Result<()> {
    validate_method_positional_arity(method.name(), given)
}

#[inline]
pub fn validate_interpreter_method_keywords(
    method: InterpreterMethod,
    has_keywords: bool,
) -> Result<()> {
    validate_method_keywords(method.name(), has_keywords)
}

/// Classify a string method that must be evaluated by the VM dispatcher.
///
/// `format` needs keyword arguments and the interpreter's
/// `format_str_template`; `format_map` routes through
/// `format_str_template_map`; `maketrans` is a staticmethod whose receiver is
/// discarded and forwarded to `str_maketrans`.  The interpreter-free `call`
/// arm for `format`/`format_map` is a drift-guard stub that is never reached
/// at runtime.
pub fn interpreter_method(method: &str) -> Option<InterpreterMethod> {
    match method {
        "format" => Some(InterpreterMethod::Format),
        "format_map" => Some(InterpreterMethod::FormatMap),
        "maketrans" => Some(InterpreterMethod::MakeTrans),
        _ => None,
    }
}

/// Compatibility predicate for callers that have not migrated to the typed
/// [`InterpreterMethod`] route yet.
#[deprecated(since = "0.1.0", note = "use interpreter_method(method).is_some()")]
pub fn requires_vm_template(method: &str) -> bool {
    interpreter_method(method).is_some()
}

pub fn call(method: &str, src: &Value, args: &[Value]) -> Result<Value> {
    validate_method_positional_arity(method, args.len())?;
    call_prevalidated(method, src, args)
}

/// Dispatch after an interpreter adapter has already validated positional
/// arity.
#[doc(hidden)]
pub fn call_prevalidated(method: &str, src: &Value, args: &[Value]) -> Result<Value> {
    let s: &str = src.as_str().unwrap();
    match method {
        // Common Sequence Operations (via char indexing).  ASCII-ness is cached
        // O(1) on the string header (#2124), so the find/index/count fast paths
        // no longer rescan the whole string on every call.
        "index" => str_index(s, src.str_is_ascii(), args),
        "count" => str_count(s, src.str_is_ascii(), args),
        // Splitting / joining
        "split" => split(src, s, args),
        "rsplit" => rsplit(src, s, args),
        "join" => join(s, args),
        "splitlines" => str_splitlines(s, args),
        "partition" => {
            expect_arg_count(args, 1, 1, "partition")?;
            // CPython: "must be str, not <T>" (no param name in the message)
            let sep = match args[0].kind() {
                ValueKind::Str(s) => s,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("must be str, not {}", builtin_type_name(&args[0])),
                    ));
                }
            };
            str_partition(s, sep)
        }
        "rpartition" => {
            expect_arg_count(args, 1, 1, "rpartition")?;
            let sep = match args[0].kind() {
                ValueKind::Str(s) => s,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("must be str, not {}", builtin_type_name(&args[0])),
                    ));
                }
            };
            str_rpartition(s, sep)
        }
        // Stripping
        "strip" => strip_chars(src, s, args, true, true, "strip"),
        "lstrip" => strip_chars(src, s, args, true, false, "lstrip"),
        "rstrip" => strip_chars(src, s, args, false, true, "rstrip"),
        // Prefix/suffix removal
        "removeprefix" => {
            expect_arg_count(args, 1, 1, "removeprefix")?;
            // CPython: "removeprefix() argument must be str, not <type>"
            // None displays as "None" not "NoneType" in this message.
            let prefix = match args[0].kind() {
                ValueKind::Str(p) => p,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "removeprefix() argument must be str, not {}",
                            py_value_display_name(&args[0])
                        ),
                    ));
                }
            };
            Ok(str_removeprefix(src, s, prefix))
        }
        "removesuffix" => {
            expect_arg_count(args, 1, 1, "removesuffix")?;
            // CPython: "removesuffix() argument must be str, not <type>"
            // None displays as "None" not "NoneType" in this message.
            let suffix = match args[0].kind() {
                ValueKind::Str(p) => p,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "removesuffix() argument must be str, not {}",
                            py_value_display_name(&args[0])
                        ),
                    ));
                }
            };
            Ok(str_removesuffix(src, s, suffix))
        }
        // Justification / padding
        "center" => {
            expect_arg_count(args, 1, 2, "center")?;
            let width = extract_int(&args[0], "center", "width")?;
            let fill = extract_fill_char(args)?;
            str_center(src, s, src.str_is_ascii(), width, fill)
        }
        "ljust" => {
            expect_arg_count(args, 1, 2, "ljust")?;
            let width = extract_int(&args[0], "ljust", "width")?;
            let fill = extract_fill_char(args)?;
            str_ljust(src, s, src.str_is_ascii(), width, fill)
        }
        "rjust" => {
            expect_arg_count(args, 1, 2, "rjust")?;
            let width = extract_int(&args[0], "rjust", "width")?;
            let fill = extract_fill_char(args)?;
            str_rjust(src, s, src.str_is_ascii(), width, fill)
        }
        "zfill" => {
            expect_arg_count(args, 1, 1, "zfill")?;
            let width = extract_int(&args[0], "zfill", "width")?;
            str_zfill(src, s, src.str_is_ascii(), width)
        }
        "expandtabs" => {
            // expandtabs() takes at most 1 argument (<got> given)
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "expandtabs() takes at most 1 argument ({} given)",
                        args.len()
                    ),
                ));
            }
            let tabsize = extract_optional_int(args, 0)?.unwrap_or(8);
            Ok(str_expandtabs(src, s, tabsize))
        }
        // Case
        "upper" => Ok(Value::string(if src.str_is_ascii() {
            s.to_ascii_uppercase()
        } else {
            s.to_uppercase()
        })),
        "lower" => Ok(Value::string(if src.str_is_ascii() {
            s.to_ascii_lowercase()
        } else {
            s.to_lowercase()
        })),
        "casefold" => Ok(Value::string(unicode_casefold(s, src.str_is_ascii()))),
        "capitalize" => Ok(Value::string(capitalize(s))),
        "swapcase" => Ok(Value::string(swapcase(s, src.str_is_ascii()))),
        "title" => Ok(Value::string(titlecase(s, src.str_is_ascii()))),
        // Searching
        "find" => str_find(s, src.str_is_ascii(), args, false),
        "rfind" => str_rfind(s, src.str_is_ascii(), args, false),
        "rindex" => str_rfind(s, src.str_is_ascii(), args, true),
        // Replacement
        "replace" => str_replace(src, s, args),
        // Testing
        "startswith" => str_startswith(s, src.str_is_ascii(), args),
        "endswith" => str_endswith(s, src.str_is_ascii(), args),
        "isdigit" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    // is_python_digit includes superscript/subscript No codepoints which
                    // are all non-ASCII, so pure ASCII strings can shortcut with is_ascii_digit.
                    s.bytes().all(|b| b.is_ascii_digit())
                } else {
                    // Use cesu8_codepoints so surrogate bytes don't reach chars().
                    // char::from_u32 returns None for surrogates; None → false → all() fails.
                    cesu8_codepoints(s).all(|n| char::from_u32(n).is_some_and(is_python_digit))
                },
        )),
        "isalpha" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(|b| b.is_ascii_alphabetic())
                } else {
                    cesu8_codepoints(s).all(|n| char::from_u32(n).is_some_and(is_python_alpha))
                },
        )),
        "isalnum" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(|b| b.is_ascii_alphanumeric())
                } else {
                    cesu8_codepoints(s).all(|n| char::from_u32(n).is_some_and(is_python_alnum))
                },
        )),
        "isspace" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(is_python_space_ascii)
                } else {
                    cesu8_codepoints(s).all(|n| char::from_u32(n).is_some_and(is_python_space))
                },
        )),
        "isdecimal" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(|b| b.is_ascii_digit())
                } else {
                    cesu8_codepoints(s).all(|n| {
                        char::from_u32(n).is_some_and(|c| {
                            // general_category tracks a newer Unicode than CPython
                            // 3.12 (Unicode 15.0); codepoints assigned in 16.0+ were
                            // Cn in 15.0 and must not count as decimal.
                            !unicode_data::is_assigned_after_15_0(c)
                                && c.general_category() == GeneralCategory::DecimalNumber
                        })
                    })
                },
        )),
        "isnumeric" => Ok(Value::bool_(
            !s.is_empty()
                && if src.str_is_ascii() {
                    s.bytes().all(|b| b.is_ascii_digit())
                } else {
                    cesu8_codepoints(s)
                        .all(|n| char::from_u32(n).is_some_and(unicode_data::is_numeric))
                },
        )),
        "islower" => Ok(Value::bool_(str_islower(s, src.str_is_ascii()))),
        "isupper" => Ok(Value::bool_(str_isupper(s, src.str_is_ascii()))),
        "istitle" => Ok(Value::bool_(str_istitle(s, src.str_is_ascii()))),
        "isascii" => Ok(Value::bool_(src.str_is_ascii())),
        "isidentifier" => Ok(Value::bool_(str_isidentifier(s, src.str_is_ascii()))),
        "isprintable" => Ok(Value::bool_(if src.str_is_ascii() {
            // Printable ASCII: 0x20 (space) through 0x7e (~). DEL (0x7f) is not printable.
            s.bytes().all(|b| (0x20..0x7f).contains(&b))
        } else {
            // Use cesu8_codepoints to handle surrogate bytes without invoking
            // chars(), which panics in debug builds on surrogate byte sequences.
            cesu8_codepoints(s).all(cp_is_printable)
        })),
        "encode" => str_encode(s, args),
        "translate" => str_translate(s, args),
        // `format` is intercepted by the interpreter's builtin-method domain
        // and routed through `format_str_template` (which handles kwargs).
        // This arm exists solely to satisfy the drift-guard test that verifies
        // every entry in METHODS has a dispatch arm; it is never reached at
        // runtime.
        "format" => Err(PyError::named(
            "TypeError",
            format!(
                "descriptor 'format' of 'str' object needs an argument ({} given)",
                args.len()
            ),
        )),
        // `format_map` is intercepted by `call_str_method` in the interpreter
        // and routed through `format_str_template_map` (which needs `&mut Interpreter`).
        // This arm exists solely to satisfy the drift-guard test that verifies every
        // entry in METHODS has a dispatch arm; it will never be reached at runtime.
        "format_map" => Err(PyError::named(
            "TypeError",
            format!(
                "str.format_map() takes exactly one argument ({} given)",
                args.len()
            ),
        )),
        // Intercepted by the interpreter's iteration domain; drift sentinel.
        "__iter__" => Err(PyError::named(
            "TypeError",
            "'str' __iter__ must be dispatched by the interpreter",
        )),
        // __getnewargs__ supports the pickle protocol: it returns a 1-tuple
        // containing the str itself, i.e. 'hello'.__getnewargs__() == ('hello',).
        "__getnewargs__" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "str.__getnewargs__() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            Ok(Value::tuple(vec![src.clone()]))
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'str' object has no attribute '{method}'"),
        )),
    }
}

// Method implementation groups share this module's private string API.

include!("string/layout.rs");
include!("string/partition_and_lines.rs");
include!("string/casing_and_classification.rs");
include!("string/searching.rs");
include!("string/splitting.rs");
include!("string/joining.rs");
include!("string/replacement.rs");
include!("string/prefix_suffix.rs");
include!("string/strip_and_indices.rs");
include!("string/encode_call.rs");
include!("string/codec_encoding.rs");
include!("string/cp1252_and_escape.rs");
include!("string/translation.rs");
