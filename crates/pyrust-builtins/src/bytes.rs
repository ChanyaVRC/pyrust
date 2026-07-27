use indexmap::IndexMap;
use pyrust_core::{
    PyBigIntSign, PyDict, PyError, PyKey, Result, StrKey, Value, ValueKind, builtin_type_name,
    py_value_display_name,
};

use crate::method_signature::{KeywordPolicy, PositionalArity};

pub const TYPE_NAME: &str = "bytes";

/// Canonical list of method names dispatched by `call`.
/// Single source of truth for `has_method` and the drift-guard test.
macro_rules! define_instance_methods {
    ($($method:literal),+ $(,)?) => {
        pub const METHODS: &[&str] = &[$($method),+];

        /// Returns `true` if `method` is the name of a built-in `bytes`
        /// instance method.
        ///
        /// The generated match keeps the fused exact-bytes call gate
        /// constant-time without maintaining a second method inventory.
        #[inline(always)]
        pub fn has_method(method: &str) -> bool {
            matches!(method, $($method)|+)
        }
    };
}

define_instance_methods!(
    "__iter__",
    "hex",
    "decode",
    "startswith",
    "endswith",
    "find",
    "rfind",
    "index",
    "rindex",
    "count",
    "upper",
    "lower",
    // Added in #829
    "replace",
    "strip",
    "lstrip",
    "rstrip",
    "removeprefix",
    "removesuffix",
    "split",
    "rsplit",
    "splitlines",
    "join",
    "title",
    "capitalize",
    "isdigit",
    "isalpha",
    "isalnum",
    "isupper",
    "islower",
    "isspace",
    "center",
    "ljust",
    "rjust",
    "zfill",
    "translate",
    // Added in #1425
    "partition",
    "rpartition",
    "swapcase",
    "isascii",
    "istitle",
    // Added in #1170
    "expandtabs",
    "__getnewargs__",
);

pub const CLASS_ATTRS: crate::primitive_class_attrs::PrimitiveClassAttrs =
    crate::primitive_class_attrs::PrimitiveClassAttrs::new(TYPE_NAME, METHODS)
        .with_native_class_methods(&["fromhex"])
        .with_native_static_methods(&["maketrans"])
        .with_flags(crate::primitive_class_attrs::PrimitiveClassFlags::NONE.with_new());

/// Positional signature for every public bytes method, plus the class-level
/// `fromhex` and `maketrans` helpers.
pub fn positional_arity(method: &str) -> Option<PositionalArity> {
    Some(match method {
        "__iter__" | "upper" | "lower" | "title" | "capitalize" | "isdigit" | "isalpha"
        | "isalnum" | "isupper" | "islower" | "isspace" | "swapcase" | "isascii" | "istitle"
        | "__getnewargs__" => PositionalArity::exact(0),
        "join" | "removeprefix" | "removesuffix" | "zfill" | "partition" | "rpartition"
        | "fromhex" => PositionalArity::exact(1),
        "strip" | "lstrip" | "rstrip" => PositionalArity::range(0, 1),
        "splitlines" | "expandtabs" => PositionalArity::range_takes_at_most(0, 1),
        "hex" | "decode" | "split" | "rsplit" => PositionalArity::range_takes_at_most(0, 2),
        "center" | "ljust" | "rjust" => PositionalArity::range(1, 2),
        "translate" => PositionalArity::range_takes_at_most(1, 2),
        "startswith" | "endswith" | "find" | "rfind" | "index" | "rindex" | "count" => {
            PositionalArity::range(1, 3)
        }
        "replace" => PositionalArity::range(2, 3),
        "maketrans" => PositionalArity::exact(2),
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

/// Keyword policy for bytes methods and its positional-only static/class
/// helpers. Accepted keyword names are parsed by the method-local
/// `argument_merging`, `hex`, `decode`, and `translation` implementations.
pub fn keyword_policy(method: &str) -> Option<KeywordPolicy> {
    let policy = match method {
        "hex" | "decode" | "split" | "rsplit" | "splitlines" | "expandtabs" | "translate" => {
            KeywordPolicy::Accept
        }
        _ if positional_arity(method).is_some() => KeywordPolicy::Reject,
        _ => return None,
    };
    Some(policy)
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

pub fn call(method: &str, receiver: &Value, args: &[Value], kwargs: &PyDict) -> Result<Value> {
    validate_method_keywords(method, !kwargs.is_empty())?;
    validate_method_positional_arity(method, args.len())?;
    call_prevalidated(method, receiver, args, kwargs)
}

/// Dispatch after an interpreter adapter has already validated positional
/// arity.
#[doc(hidden)]
pub fn call_prevalidated(
    method: &str,
    receiver: &Value,
    args: &[Value],
    kwargs: &PyDict,
) -> Result<Value> {
    let bytes: &[u8] = match receiver.kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "expected bytes receiver, got {}",
                    pyrust_core::builtin_type_name(receiver)
                ),
            ));
        }
    };
    // __getnewargs__ supports the pickle protocol: it returns a 1-tuple
    // containing the bytes itself, i.e. b'hi'.__getnewargs__() == (b'hi',).
    // Handled here (not in `call_on_slice`) so `bytearray` — which reuses
    // `call_on_slice` but has no `__getnewargs__` in CPython — never reaches it.
    if method == "__getnewargs__" {
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "bytes.__getnewargs__() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        return Ok(Value::tuple(vec![receiver.clone()]));
    }
    call_on_slice(method, bytes, args, kwargs)
}

/// Dispatch a bytes method on a raw `&[u8]` slice.  Used by `bytearray` to
/// reuse bytes read-method implementations without constructing a temporary
/// `Value::bytes`.  Results that produce new bytes values (upper, lower, etc.)
/// return `Value::bytes`; the bytearray module wraps those into bytearray.
pub fn call_on_slice(method: &str, bytes: &[u8], args: &[Value], kwargs: &PyDict) -> Result<Value> {
    match method {
        "hex" => bytes_hex(bytes, args, kwargs),
        "decode" => bytes_decode(bytes, args, kwargs),
        "startswith" => bytes_startswith(bytes, args),
        "endswith" => bytes_endswith(bytes, args),
        "find" => bytes_find(bytes, args),
        "rfind" => bytes_rfind(bytes, args),
        "index" => bytes_index(bytes, args),
        "rindex" => bytes_rindex(bytes, args),
        "count" => bytes_count(bytes, args),
        "upper" => Ok(Value::bytes(
            bytes.iter().map(|b| b.to_ascii_uppercase()).collect(),
        )),
        "lower" => Ok(Value::bytes(
            bytes.iter().map(|b| b.to_ascii_lowercase()).collect(),
        )),
        // Added in #829
        "replace" => bytes_replace(bytes, args),
        "strip" => bytes_strip(bytes, args, true, true),
        "lstrip" => bytes_strip(bytes, args, true, false),
        "rstrip" => bytes_strip(bytes, args, false, true),
        "removeprefix" => bytes_removeprefix(bytes, args),
        "removesuffix" => bytes_removesuffix(bytes, args),
        "split" => {
            let merged = merge_split_kwargs("split", args, kwargs)?;
            bytes_split(bytes, &merged)
        }
        "rsplit" => {
            let merged = merge_split_kwargs("rsplit", args, kwargs)?;
            bytes_rsplit(bytes, &merged)
        }
        "splitlines" => {
            if kwargs.is_empty() {
                // Interpreter adapters pass an already-bound, truth-normalized
                // Bool positionally. Avoid cloning that hot-path argument into
                // a second temporary Vec.
                bytes_splitlines(bytes, args)
            } else {
                let merged = merge_single_kwarg("splitlines", "keepends", args, kwargs)?;
                bytes_splitlines(bytes, &merged)
            }
        }
        "join" => bytes_join(bytes, args),
        "title" => Ok(Value::bytes(bytes_title(bytes))),
        "capitalize" => Ok(Value::bytes(bytes_capitalize(bytes))),
        "isdigit" => Ok(Value::bool_(
            !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_digit()),
        )),
        "isalpha" => Ok(Value::bool_(
            !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_alphabetic()),
        )),
        "isalnum" => Ok(Value::bool_(
            !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_alphanumeric()),
        )),
        "isupper" => Ok(Value::bool_(bytes_isupper(bytes))),
        "islower" => Ok(Value::bool_(bytes_islower(bytes))),
        "isspace" => Ok(Value::bool_(
            !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_whitespace()),
        )),
        "center" => bytes_center(bytes, args),
        "ljust" => bytes_ljust(bytes, args),
        "rjust" => bytes_rjust(bytes, args),
        "zfill" => bytes_zfill(bytes, args),
        "translate" => bytes_translate(bytes, args, kwargs),
        // Added in #1425
        "partition" => bytes_partition(bytes, args, false),
        "rpartition" => bytes_partition(bytes, args, true),
        "swapcase" => Ok(Value::bytes(
            bytes
                .iter()
                .map(|&b| {
                    if b.is_ascii_uppercase() {
                        b.to_ascii_lowercase()
                    } else if b.is_ascii_lowercase() {
                        b.to_ascii_uppercase()
                    } else {
                        b
                    }
                })
                .collect(),
        )),
        "isascii" => Ok(Value::bool_(bytes.iter().all(|&b| b < 128))),
        "istitle" => Ok(Value::bool_(bytes_istitle(bytes))),
        // Added in #1170
        "expandtabs" => {
            let merged = merge_single_kwarg("expandtabs", "tabsize", args, kwargs)?;
            bytes_expandtabs(bytes, &merged)
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'bytes' object has no attribute '{method}'"),
        )),
    }
}

// Method implementation groups share this module's private bytes API.

include!("bytes/hex.rs");
include!("bytes/decode_call.rs");
include!("bytes/codec_dispatch.rs");
include!("bytes/raw_unicode_escape.rs");
include!("bytes/unicode_escape.rs");
include!("bytes/utf7.rs");
include!("bytes/unicode_codecs.rs");
include!("bytes/search.rs");
include!("bytes/search_helpers.rs");
include!("bytes/replacement_and_strip.rs");
include!("bytes/argument_merging.rs");
include!("bytes/splitting.rs");
include!("bytes/casing_and_padding.rs");
include!("bytes/translation_and_partition.rs");
include!("bytes/static_helpers.rs");
