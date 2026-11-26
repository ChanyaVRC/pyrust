//! Declarative macros generating thin native-Rust `_*` internal C-extension
//! compatibility shims (issue #3250).
//!
//! Each generated shim re-exports the public/internal surface of its existing
//! PyRust stdlib counterpart on first import (via `post_load_init`), keeping
//! exact object identity (`_collections.deque is collections.deque`, etc.) and
//! deterministic insertion-ordered `__dict__` iteration.

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_registry::BuiltinReg;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyNumber, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class};
use crate::value::{PyDict, PyModule, Value, ValueKind};

const SKIP_DUNDERS: &[&str] = &[
    "__name__",
    "__doc__",
    "__package__",
    "__loader__",
    "__spec__",
    "__file__",
    "__path__",
    "__all__",
    "__cached__",
    "__builtins__",
];

fn call_positional(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

/// Copy every non-dunder attribute from `src_module_name` into `dest` in exact
/// insertion order without overwriting attributes already defined on `dest`.
fn copy_all_from(
    interp: &mut Interpreter,
    src_module_name: &str,
    dest: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let src_val = interp.load_module(src_module_name)?;
    let ValueKind::PyModule(src_rc) = src_val.kind() else {
        return Ok(());
    };
    let snapshot = src_rc.borrow().attrs_snapshot();
    let mut dst = dest.borrow_mut();
    for (k, v) in snapshot {
        if SKIP_DUNDERS.contains(&k.as_str()) {
            continue;
        }
        if dst.get_attr_value(&k).is_none() {
            dst.insert_attr(k, v);
        }
    }
    Ok(())
}

/// Copy specific named attributes from `src_module_name` into `dest`.
fn copy_selected_from(
    interp: &mut Interpreter,
    src_module_name: &str,
    names: &[&str],
    dest: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let src_val = interp.load_module(src_module_name)?;
    for &name in names {
        if let Ok(v) = interp.get_attr(&src_val, name) {
            let mut dst = dest.borrow_mut();
            if dst.get_attr_value(name).is_none() {
                dst.insert_attr(name.to_string(), v);
            }
        }
    }
    Ok(())
}

/// Copy `(dest_name, src_name)` attribute aliases from `src_module_name` into `dest`.
fn copy_aliases_from(
    interp: &mut Interpreter,
    src_module_name: &str,
    pairs: &[(&str, &str)],
    dest: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let src_val = interp.load_module(src_module_name)?;
    for &(dst_name, src_name) in pairs {
        if let Ok(v) = interp.get_attr(&src_val, src_name) {
            dest.borrow_mut().insert_attr(dst_name.to_string(), v);
        }
    }
    Ok(())
}

/// Wichura's Algorithm AS 241 for the inverse normal cumulative distribution
/// function, matching CPython 3.12's `_statistics._normal_dist_inv_cdf`.
fn normal_dist_inv_cdf(p: f64, mu: f64, sigma: f64) -> Result<f64> {
    if !(0.0 < p && p < 1.0) {
        return Err(PyError::named(
            "ValueError",
            "p must be in the range 0.0 < p < 1.0",
        ));
    }
    if sigma <= 0.0 {
        return Err(PyError::named("ValueError", "sigma must be positive"));
    }
    let q = p - 0.5;
    let x = if q.abs() <= 0.425 {
        let r = 0.180625 - q * q;
        let num = (((((((2.509_080_928_730_122_7e3 * r + 3.343_057_558_358_813e4) * r
            + 6.726_577_092_700_87e4)
            * r
            + 4.592_195_393_154_987e4)
            * r
            + 1.373_169_376_550_946e4)
            * r
            + 1.971_590_950_306_551_3e3)
            * r
            + 1.331_416_678_917_843_8e2)
            * r
            + 3.387_132_872_796_366_6)
            * q;
        let den = ((((((5.226_495_278_852_854e3 * r + 2.872_908_573_572_194_3e4) * r
            + 3.930_789_580_009_271e4)
            * r
            + 2.121_379_430_158_659_7e4)
            * r
            + 5.394_196_021_424_751e3)
            * r
            + 6.871_870_074_920_579e2)
            * r
            + 4.231_333_070_160_091e1)
            * r
            + 1.0;
        num / den
    } else {
        let mut r = if q <= 0.0 { p } else { 1.0 - p };
        r = (-r.ln()).sqrt();
        let val = if r <= 5.0 {
            let r = r - 1.6;
            let num = ((((((7.745_450_142_783_414e-4 * r + 2.272_384_498_926_918_4e-2) * r
                + 2.417_807_251_774_506e-1)
                * r
                + 1.270_458_252_452_368_4)
                * r
                + 3.647_848_324_763_204_6)
                * r
                + 5.769_497_221_460_691)
                * r
                + 4.630_337_846_156_546)
                * r
                + 1.423_437_110_749_683_5;
            let den = ((((((1.050_750_071_644_416_8e-9 * r + 5.475_938_084_995_345e-4) * r
                + 1.519_866_656_361_645_7e-2)
                * r
                + 1.481_039_764_274_800_8e-1)
                * r
                + 6.897_673_349_851e-1)
                * r
                + 1.676_384_830_183_803_8)
                * r
                + 2.053_191_626_637_759)
                * r
                + 1.0;
            num / den
        } else {
            let r = r - 5.0;
            let num = ((((((2.010_334_399_292_288_1e-7 * r + 2.711_555_568_743_487_6e-5) * r
                + 1.242_660_947_388_078_4e-3)
                * r
                + 2.653_218_952_657_612_3e-2)
                * r
                + 2.965_605_718_285_048_7e-1)
                * r
                + 1.784_826_539_917_291_3)
                * r
                + 5.463_784_911_164_114)
                * r
                + 6.657_904_643_501_103;
            let den = ((((((2.044_263_103_389_939_7e-15 * r + 1.421_511_758_316_446e-7) * r
                + 1.846_318_317_510_054_8e-5)
                * r
                + 7.868_691_311_456_133e-4)
                * r
                + 1.487_536_129_085_061_5e-2)
                * r
                + 1.369_298_809_227_358e-1)
                * r
                + 5.998_322_065_558_88e-1)
                * r
                + 1.0;
            num / den
        };
        if q < 0.0 { -val } else { val }
    };
    Ok(mu + x * sigma)
}

fn json_escape_string(s: &str, ensure_ascii: bool) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c if ensure_ascii && !c.is_ascii() => {
                let cp = c as u32;
                if cp < 0x10000 {
                    out.push_str(&format!("\\u{cp:04x}"));
                } else {
                    let n = cp - 0x10000;
                    let hi = 0xD800 + (n >> 10);
                    let lo = 0xDC00 + (n & 0x3FF);
                    out.push_str(&format!("\\u{hi:04x}\\u{lo:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_scanstring(s: &str, end_idx: usize, strict: bool) -> Result<(String, usize)> {
    let chars: Vec<char> = s.chars().collect();
    if end_idx > chars.len() {
        return Err(PyError::named("ValueError", "Unterminated string"));
    }
    let mut i = end_idx;
    let mut out = String::new();
    while i < chars.len() {
        let ch = chars[i];
        i += 1;
        match ch {
            '"' => return Ok((out, i)),
            '\\' => {
                if i >= chars.len() {
                    return Err(PyError::named("ValueError", "Unterminated string"));
                }
                let esc = chars[i];
                i += 1;
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'b' => out.push('\x08'),
                    'f' => out.push('\x0c'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        if i + 4 > chars.len() {
                            return Err(PyError::named("ValueError", "Invalid \\uXXXX escape"));
                        }
                        let hex: String = chars[i..i + 4].iter().collect();
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| PyError::named("ValueError", "Invalid \\uXXXX escape"))?;
                        i += 4;
                        if (0xD800..=0xDBFF).contains(&code)
                            && i + 6 <= chars.len()
                            && chars[i] == '\\'
                            && chars[i + 1] == 'u'
                        {
                            let hex2: String = chars[i + 2..i + 6].iter().collect();
                            if let Ok(low) = u32::from_str_radix(&hex2, 16)
                                && (0xDC00..=0xDFFF).contains(&low)
                            {
                                let full = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                if let Some(c) = char::from_u32(full) {
                                    out.push(c);
                                    i += 6;
                                    continue;
                                }
                            }
                        }
                        out.push_str(&pyrust_core::cesu8_encode_codepoint(code));
                    }
                    other => {
                        return Err(PyError::named(
                            "ValueError",
                            format!("Invalid \\escape: {other:?}"),
                        ));
                    }
                }
            }
            c if strict && (c as u32) < 0x20 => {
                return Err(PyError::named("ValueError", "Invalid control character"));
            }
            c => out.push(c),
        }
    }
    Err(PyError::named("ValueError", "Unterminated string"))
}

// ── Shims with Native Extra Functions ────────────────────────────────────────

pub mod _thread {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_thread";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_thread.";

    pyrust_module! {
        fn _count() -> Result<Value> {
            Ok(Value::int(
                _interp.cold.gil_state.active_worker_count_val() as i64,
            ))
        }

        fn daemon_threads_allowed() -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(true))
        }

        fn _is_main_interpreter() -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(true))
        }

        fn exit() -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("SystemExit", ""))
        }

        fn exit_thread() -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("SystemExit", ""))
        }

        fn interrupt_main(#[default(2i64.into())] _signum: PyIndex) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named("KeyboardInterrupt", ""))
        }

        fn start_new_thread(
            function: PyValueRef<'_>,
            args: PyValueRef<'_>,
            #[default(PyValue(Value::none()))] kwargs: PyValue,
        ) -> Result<Value> {
            if !matches!(args.0.kind(), ValueKind::Tuple(_)) {
                return Err(PyError::named(
                    "TypeError",
                    "2nd arg must be a tuple",
                ));
            }
            if !kwargs.0.is_none() && kwargs.0.as_dict().is_none() {
                return Err(PyError::named(
                    "TypeError",
                    "optional 3rd arg must be a dictionary",
                ));
            }
            let th_mod = _interp.load_module("threading")?;
            let thread_cls = _interp.get_attr(&th_mod, "Thread")?;
            let mut kw_args = vec![
                ExpandedCallArg {
                    name: Some("target".to_string()),
                    value: function.0.clone(),
                },
                ExpandedCallArg {
                    name: Some("args".to_string()),
                    value: args.0.clone(),
                },
                ExpandedCallArg {
                    name: Some("daemon".to_string()),
                    value: Value::bool_(true),
                },
            ];
            if !kwargs.0.is_none() {
                kw_args.push(ExpandedCallArg {
                    name: Some("kwargs".to_string()),
                    value: kwargs.0.clone(),
                });
            }
            let thread_obj = _interp.call_function_expanded(thread_cls, &kw_args)?;
            let start_fn = _interp.get_attr(&thread_obj, "start")?;
            let _ = _interp.call_function_expanded(start_fn, &[])?;
            _interp.get_attr(&thread_obj, "ident")
        }

        fn start_new(
            function: PyValueRef<'_>,
            args: PyValueRef<'_>,
            #[default(PyValue(Value::none()))] kwargs: PyValue,
        ) -> Result<Value> {
            let m = _interp.load_module("_thread")?;
            let f = _interp.get_attr(&m, "start_new_thread")?;
            call_positional(
                _interp,
                f,
                &[function.0.clone(), args.0.clone(), kwargs.0.clone()],
            )
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_selected_from(
            interp,
            "threading",
            &[
                "TIMEOUT_MAX",
                "RLock",
                "get_ident",
                "get_native_id",
                "stack_size",
            ],
            module,
        )?;
        copy_aliases_from(
            interp,
            "threading",
            &[
                ("allocate_lock", "Lock"),
                ("allocate", "Lock"),
                ("LockType", "_LockType"),
                ("_local", "local"),
                ("_excepthook", "excepthook"),
                ("_ExceptHookArgs", "ExceptHookArgs"),
                ("_set_sentinel", "Lock"),
            ],
            module,
        )?;
        if let Some(rt_err) = lookup_exc_class("RuntimeError") {
            module
                .borrow_mut()
                .insert_attr("error".to_string(), Value::py_class(rt_err));
        }
        Ok(())
    }
}

pub mod _abc {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_abc";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_abc.";

    pyrust_module! {
        fn _reset_caches(#[positional_only] _cls: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn _reset_registry(#[positional_only] _cls: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_all_from(interp, "abc", module)
    }
}

pub mod _collections {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_collections";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_collections.";

    pyrust_module! {
        fn _count_elements(
            #[positional_only] mapping: PyValueRef<'_>,
            #[positional_only] iterable: PyValueRef<'_>,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(iterable.0)?;
            let get_fn = _interp.get_attr(mapping.0, "get")?;
            let setitem_fn = _interp.get_attr(mapping.0, "__setitem__")?;
            for elem in items {
                let cur =
                    call_positional(_interp, get_fn.clone(), &[elem.clone(), Value::int(0)])?;
                let next_val = if let Some(n) = cur.as_int() {
                    Value::int(n + 1)
                } else {
                    let add_fn = _interp.get_attr(&cur, "__add__")?;
                    call_positional(_interp, add_fn, &[Value::int(1)])?
                };
                call_positional(_interp, setitem_fn.clone(), &[elem, next_val])?;
            }
            Ok(Value::none())
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_selected_from(
            interp,
            "collections",
            &["deque", "defaultdict", "OrderedDict", "_deque_iterator"],
            module,
        )
    }
}

pub mod _weakref {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_weakref";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_weakref.";

    pyrust_module! {
        fn _remove_dead_weakref(
            #[positional_only] d: PyValueRef<'_>,
            #[positional_only] key: PyValueRef<'_>,
        ) -> Result<Value> {
            if let Ok(pop_fn) = _interp.get_attr(d.0, "pop") {
                let _ = call_positional(_interp, pop_fn, &[key.0.clone(), Value::none()]);
            }
            Ok(Value::none())
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_selected_from(
            interp,
            "weakref",
            &[
                "ref",
                "ReferenceType",
                "proxy",
                "ProxyType",
                "CallableProxyType",
                "getweakrefcount",
                "getweakrefs",
            ],
            module,
        )
    }
}

pub mod _struct {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_struct";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_struct.";

    pyrust_module! {
        fn _clearcache() -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_all_from(interp, "struct", module)
    }
}

pub mod _json {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_json";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_json.";

    pyrust_module! {
        fn encode_basestring(#[positional_only] string: PyStr<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::string(json_escape_string(&string, false)))
        }

        fn encode_basestring_ascii(#[positional_only] string: PyStr<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(Value::string(json_escape_string(&string, true)))
        }

        fn scanstring(
            #[positional_only] string: PyStr<'_>,
            #[positional_only] end: PyIndex,
            #[default(true.into())] strict: PyBool,
        ) -> Result<Value> {
            let _ = _interp;
            let end_i = end.expect_i64(FN_NAME, "end")?.max(0) as usize;
            let (decoded, next_idx) = json_scanstring(&string, end_i, strict.0)?;
            Ok(Value::tuple(vec![
                Value::string(decoded),
                Value::int(next_idx as i64),
            ]))
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_all_from(interp, "json", module)
    }
}

pub mod _sre {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_sre";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_sre.";

    pyrust_module! {
        constants {
            "CODESIZE" => Value::int(4),
            "MAGIC" => Value::int(20221023),
            "MAXREPEAT" => Value::int(4294967295),
            "MAXGROUPS" => Value::int(2147483647),
            "copyright" => Value::string(" SRE 2.2.2 Copyright (c) 1997-2002 by Secret Labs AB "),
        }

        fn getcodesize() -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(4))
        }

        fn ascii_tolower(#[positional_only] c: PyIndex) -> Result<Value> {
            let _ = _interp;
            let cp = c.expect_i64(FN_NAME, "c")?;
            let res = if (b'A' as i64..=b'Z' as i64).contains(&cp) {
                cp + 32
            } else {
                cp
            };
            Ok(Value::int(res))
        }

        fn ascii_iscased(#[positional_only] c: PyIndex) -> Result<Value> {
            let _ = _interp;
            let cp = c.expect_i64(FN_NAME, "c")?;
            let res = (b'A' as i64..=b'Z' as i64).contains(&cp)
                || (b'a' as i64..=b'z' as i64).contains(&cp);
            Ok(Value::bool_(res))
        }

        fn unicode_tolower(#[positional_only] c: PyIndex) -> Result<Value> {
            let _ = _interp;
            let cp = c.expect_i64(FN_NAME, "c")?;
            let res = u32::try_from(cp)
                .ok()
                .and_then(char::from_u32)
                .and_then(|ch| ch.to_lowercase().next())
                .map(|ch| i64::from(ch as u32))
                .unwrap_or(cp);
            Ok(Value::int(res))
        }

        fn unicode_iscased(#[positional_only] c: PyIndex) -> Result<Value> {
            let _ = _interp;
            let cp = c.expect_i64(FN_NAME, "c")?;
            let res = u32::try_from(cp)
                .ok()
                .and_then(char::from_u32)
                .is_some_and(|ch| ch.is_lowercase() || ch.is_uppercase());
            Ok(Value::bool_(res))
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_all_from(interp, "re", module)
    }
}

pub mod _warnings {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_warnings";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_warnings.";

    pyrust_module! {
        constants {
            "_defaultaction" => Value::string("default"),
            "_onceregistry" => Value::dict(PyDict::default()),
        }

        fn _filters_mutated() -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_selected_from(
            interp,
            "warnings",
            &["warn", "warn_explicit", "filters"],
            module,
        )
    }
}

pub mod _hashlib {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_hashlib";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_hashlib.";

    pyrust_module! {
        exception UnsupportedDigestmodError(ValueError);

        fn get_fips_mode() -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(0))
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_selected_from(interp, "hashlib", &["new", "pbkdf2_hmac"], module)?;
        copy_aliases_from(
            interp,
            "hashlib",
            &[
                ("openssl_md5", "md5"),
                ("openssl_sha1", "sha1"),
                ("openssl_sha224", "sha224"),
                ("openssl_sha256", "sha256"),
                ("openssl_sha384", "sha384"),
                ("openssl_sha512", "sha512"),
                ("openssl_sha3_224", "sha3_224"),
                ("openssl_sha3_256", "sha3_256"),
                ("openssl_sha3_384", "sha3_384"),
                ("openssl_sha3_512", "sha3_512"),
                ("openssl_shake_128", "shake_128"),
                ("openssl_shake_256", "shake_256"),
            ],
            module,
        )?;
        copy_aliases_from(
            interp,
            "hmac",
            &[
                ("compare_digest", "compare_digest"),
                ("HMAC", "HMAC"),
                ("hmac_new", "new"),
                ("hmac_digest", "digest"),
            ],
            module,
        )?;
        let hl = interp.load_module("hashlib")?;
        if let Ok(algos) = interp.get_attr(&hl, "algorithms_guaranteed")
            && let Ok(fs) = call_positional(interp, Value::builtin_function("frozenset"), &[algos])
        {
            module
                .borrow_mut()
                .insert_attr("openssl_md_meth_names".to_string(), fs);
        }
        if let Ok(md5_fn) = interp.get_attr(&hl, "md5")
            && let Ok(h) = call_positional(interp, md5_fn, &[])
            && let Ok(cls) = call_positional(interp, Value::builtin_function("type"), &[h])
        {
            module.borrow_mut().insert_attr("HASH".to_string(), cls);
        }
        if let Ok(shake_fn) = interp.get_attr(&hl, "shake_128")
            && let Ok(h) = call_positional(interp, shake_fn, &[])
            && let Ok(cls) = call_positional(interp, Value::builtin_function("type"), &[h])
        {
            module.borrow_mut().insert_attr("HASHXOF".to_string(), cls);
        }
        Ok(())
    }
}

pub mod _typing {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_typing";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_typing.";

    pyrust_module! {
        fn _idfunc(#[positional_only] x: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(x.0.clone())
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_selected_from(
            interp,
            "typing",
            &[
                "Generic",
                "ParamSpec",
                "ParamSpecArgs",
                "ParamSpecKwargs",
                "TypeAliasType",
                "TypeVar",
                "TypeVarTuple",
            ],
            module,
        )
    }
}

pub mod _opcode {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_opcode";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_opcode.";

    pyrust_module! {
        fn get_specialization_stats() -> Result<Value> {
            let _ = _interp;
            Ok(Value::dict(PyDict::default()))
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_selected_from(interp, "opcode", &["stack_effect"], module)
    }
}

pub mod _statistics {
    use super::*;
    pub(crate) const MODULE_NAME: &str = "_statistics";
    #[allow(dead_code)]
    pub(crate) const FN_PREFIX: &str = "_statistics.";

    pyrust_module! {
        fn _normal_dist_inv_cdf(
            #[positional_only] p: PyNumber,
            #[positional_only] mu: PyNumber,
            #[positional_only] sigma: PyNumber,
        ) -> Result<Value> {
            let _ = _interp;
            let res = normal_dist_inv_cdf(p.as_f64(), mu.as_f64(), sigma.as_f64())?;
            Ok(Value::float(res))
        }
    }

    pub(crate) fn init_shim(
        interp: &mut Interpreter,
        module: &Rc<RefCell<PyModule>>,
    ) -> Result<()> {
        copy_all_from(interp, "statistics", module)
    }
}

// ── Declarative Macro for Thin Pure Re-Export Shims ──────────────────────────

macro_rules! define_reexport_shims {
    (
        $(
            $mod_ident:ident {
                $( constants { $($const_key:literal => $const_val:expr),* $(,)? } )?
                init($interp:ident, $module:ident) $init_block:block
            }
        ),* $(,)?
    ) => {
        $(
            pub mod $mod_ident {
                #[allow(unused_imports)]
                use super::*;

                pub(crate) const MODULE_NAME: &str = stringify!($mod_ident);
                #[allow(dead_code)]
                pub(crate) const FN_PREFIX: &str = concat!(stringify!($mod_ident), ".");

                pyrust_module! {
                    $(
                        constants {
                            $($const_key => $const_val,)*
                        }
                    )?
                }

                pub(crate) fn init_shim(
                    $interp: &mut Interpreter,
                    $module: &Rc<RefCell<PyModule>>,
                ) -> Result<()> $init_block
            }
        )*
    };
}

define_reexport_shims! {
    _io {
        init(interp, module) {
            copy_all_from(interp, "io", module)?;
            copy_aliases_from(
                interp,
                "io",
                &[
                    ("_IOBase", "IOBase"),
                    ("_RawIOBase", "RawIOBase"),
                    ("_BufferedIOBase", "BufferedIOBase"),
                    ("_TextIOBase", "TextIOBase"),
                ],
                module,
            )?;
            if let Some(bio) = lookup_exc_class("BlockingIOError") {
                module
                    .borrow_mut()
                    .insert_attr("BlockingIOError".to_string(), Value::py_class(bio));
            }
            Ok(())
        }
    },

    _collections_abc {
        init(interp, module) {
            crate::builtin_modules::collections_abc::ensure_mixins_initialized(interp)?;
            crate::builtin_modules::collections_abc::populate_abc_exports(module);
            Ok(())
        }
    },

    _weakrefset {
        init(interp, module) {
            copy_selected_from(interp, "weakref", &["WeakSet", "ref"], module)?;
            copy_selected_from(interp, "types", &["GenericAlias"], module)
        }
    },

    _ast {
        constants {
            "PyCF_ONLY_AST" => Value::int(1024),
        }
        init(interp, module) {
            copy_all_from(interp, "ast", module)
        }
    },

    _operator {
        init(interp, module) {
            copy_all_from(interp, "operator", module)?;
            copy_aliases_from(interp, "hmac", &[("_compare_digest", "compare_digest")], module)
        }
    },

    _functools {
        init(interp, module) {
            copy_selected_from(interp, "functools", &["reduce", "partial", "cmp_to_key"], module)
        }
    },

    _socket {
        init(interp, module) {
            copy_all_from(interp, "socket", module)
        }
    },

    _signal {
        init(interp, module) {
            copy_all_from(interp, "signal", module)
        }
    },

    _stat {
        init(interp, module) {
            copy_all_from(interp, "stat", module)
        }
    },

    _csv {
        constants {
            "_dialects" => Value::dict(PyDict::default()),
        }
        init(interp, module) {
            copy_all_from(interp, "csv", module)
        }
    },

    _pickle {
        init(interp, module) {
            copy_selected_from(
                interp,
                "pickle",
                &[
                    "PickleBuffer",
                    "PickleError",
                    "Pickler",
                    "PicklingError",
                    "Unpickler",
                    "UnpicklingError",
                    "dump",
                    "dumps",
                    "load",
                    "loads",
                ],
                module,
            )
        }
    },

    _random {
        init(interp, module) {
            copy_selected_from(interp, "random", &["Random"], module)
        }
    },

    _bisect {
        init(interp, module) {
            copy_selected_from(
                interp,
                "bisect",
                &["bisect_left", "bisect_right", "insort_left", "insort_right"],
                module,
            )
        }
    },

    _heapq {
        init(interp, module) {
            copy_selected_from(
                interp,
                "heapq",
                &[
                    "heapify",
                    "heappop",
                    "heappush",
                    "heappushpop",
                    "heapreplace",
                    "_heapify_max",
                    "_heappop_max",
                    "_heapreplace_max",
                ],
                module,
            )
        }
    },

    _datetime {
        init(interp, module) {
            copy_selected_from(
                interp,
                "datetime",
                &[
                    "MAXYEAR",
                    "MINYEAR",
                    "UTC",
                    "date",
                    "datetime",
                    "time",
                    "timedelta",
                    "timezone",
                    "tzinfo",
                ],
                module,
            )
        }
    },

    _decimal {
        init(interp, module) {
            copy_all_from(interp, "decimal", module)
        }
    },

    _codecs {
        init(interp, module) {
            copy_all_from(interp, "codecs", module)
        }
    },

    _locale {
        constants {
            "CHAR_MAX" => Value::int(127),
        }
        init(interp, module) {
            copy_all_from(interp, "locale", module)
        }
    },

    _contextvars {
        init(interp, module) {
            copy_selected_from(
                interp,
                "contextvars",
                &["Context", "ContextVar", "Token", "copy_context"],
                module,
            )
        }
    },

    _queue {
        init(interp, module) {
            copy_selected_from(interp, "queue", &["Empty", "SimpleQueue"], module)
        }
    },

    _md5 {
        init(interp, module) {
            copy_selected_from(interp, "hashlib", &["md5"], module)?;
            let hl = interp.load_module("hashlib")?;
            if let Ok(md5_fn) = interp.get_attr(&hl, "md5")
                && let Ok(h) = call_positional(interp, md5_fn, &[])
                && let Ok(cls) = call_positional(interp, Value::builtin_function("type"), &[h])
            {
                module.borrow_mut().insert_attr("MD5Type".to_string(), cls);
            }
            Ok(())
        }
    },

    _sha1 {
        init(interp, module) {
            copy_selected_from(interp, "hashlib", &["sha1"], module)?;
            let hl = interp.load_module("hashlib")?;
            if let Ok(f) = interp.get_attr(&hl, "sha1")
                && let Ok(h) = call_positional(interp, f, &[])
                && let Ok(cls) = call_positional(interp, Value::builtin_function("type"), &[h])
            {
                module.borrow_mut().insert_attr("SHA1Type".to_string(), cls);
            }
            Ok(())
        }
    },

    _sha2 {
        init(interp, module) {
            copy_selected_from(
                interp,
                "hashlib",
                &["sha224", "sha256", "sha384", "sha512"],
                module,
            )?;
            let hl = interp.load_module("hashlib")?;
            if let Ok(f) = interp.get_attr(&hl, "sha256")
                && let Ok(h) = call_positional(interp, f, &[])
                && let Ok(cls) = call_positional(interp, Value::builtin_function("type"), &[h])
            {
                for ty_name in ["SHA224Type", "SHA256Type", "SHA384Type", "SHA512Type"] {
                    module
                        .borrow_mut()
                        .insert_attr(ty_name.to_string(), cls.clone());
                }
            }
            Ok(())
        }
    },

    _sha3 {
        constants {
            "implementation" => Value::string("pyrust"),
        }
        init(interp, module) {
            copy_selected_from(
                interp,
                "hashlib",
                &[
                    "sha3_224",
                    "sha3_256",
                    "sha3_384",
                    "sha3_512",
                    "shake_128",
                    "shake_256",
                ],
                module,
            )
        }
    },

    _blake2 {
        constants {
            "BLAKE2B_MAX_DIGEST_SIZE" => Value::int(64),
            "BLAKE2B_MAX_KEY_SIZE" => Value::int(64),
            "BLAKE2B_SALT_SIZE" => Value::int(16),
            "BLAKE2B_PERSON_SIZE" => Value::int(16),
            "BLAKE2S_MAX_DIGEST_SIZE" => Value::int(32),
            "BLAKE2S_MAX_KEY_SIZE" => Value::int(32),
            "BLAKE2S_SALT_SIZE" => Value::int(8),
            "BLAKE2S_PERSON_SIZE" => Value::int(8),
        }
        init(interp, module) {
            copy_selected_from(interp, "hashlib", &["blake2b", "blake2s"], module)
        }
    },

    _bz2 {
        init(interp, module) {
            copy_selected_from(interp, "bz2", &["BZ2Compressor", "BZ2Decompressor"], module)
        }
    },

    _lzma {
        init(interp, module) {
            copy_all_from(interp, "lzma", module)
        }
    },

    _sqlite3 {
        init(interp, module) {
            copy_all_from(interp, "sqlite3", module)
        }
    },

    _elementtree {
        init(interp, module) {
            copy_selected_from(
                interp,
                "xml.etree.ElementTree",
                &["Element", "SubElement", "TreeBuilder", "XMLParser", "ParseError"],
                module,
            )
        }
    },

    _asyncio {
        init(interp, module) {
            copy_all_from(interp, "asyncio", module)
        }
    },

    _tokenize {
        init(interp, module) {
            copy_all_from(interp, "tokenize", module)
        }
    },

    _zoneinfo {
        init(interp, module) {
            copy_selected_from(interp, "zoneinfo", &["ZoneInfo"], module)
        }
    },

    _uuid {
        constants {
            "has_uuid_generate_time_safe" => Value::int(0),
        }
        init(interp, module) {
            copy_all_from(interp, "uuid", module)
        }
    },

    _markupbase {
        init(interp, module) {
            let hp_mod = interp.load_module("html.parser")?;
            if let Ok(hp_cls) = interp.get_attr(&hp_mod, "HTMLParser")
                && let ValueKind::PyClass(cls_rc) = hp_cls.kind()
                && let Some(base_rc) = cls_rc.borrow().base.as_ref().map(Rc::clone)
            {
                module
                    .borrow_mut()
                    .insert_attr("ParserBase".to_string(), Value::py_class(base_rc));
            }
            Ok(())
        }
    },

    _threading_local {
        init(interp, module) {
            copy_selected_from(
                interp,
                "threading",
                &["local", "RLock", "current_thread"],
                module,
            )
        }
    },
}

// ── Uniform Registration Dispatch Across All 47 Shims ────────────────────────

macro_rules! register_shim_dispatch {
    ($($mod_ident:ident),* $(,)?) => {
        pub(crate) fn all_regs() -> Vec<BuiltinReg> {
            let mut all = Vec::new();
            $(
                all.extend_from_slice($mod_ident::regs());
            )*
            all
        }

        pub(crate) fn load_builtin_module(name: &str) -> Option<Value> {
            match name {
                $(
                    stringify!($mod_ident) => Some($mod_ident::module()),
                )*
                _ => None,
            }
        }

        pub(crate) fn is_builtin_module(name: &str) -> bool {
            matches!(name, $(stringify!($mod_ident))|*)
        }

        pub(crate) fn post_load_init(
            name: &str,
            interp: &mut Interpreter,
            module: &Rc<RefCell<PyModule>>,
        ) -> Result<bool> {
            match name {
                $(
                    stringify!($mod_ident) => {
                        $mod_ident::init_shim(interp, module)?;
                        Ok(true)
                    }
                )*
                _ => Ok(false),
            }
        }
    };
}

register_shim_dispatch!(
    _thread,
    _io,
    _abc,
    _collections,
    _collections_abc,
    _weakref,
    _weakrefset,
    _ast,
    _struct,
    _operator,
    _functools,
    _socket,
    _signal,
    _stat,
    _csv,
    _json,
    _pickle,
    _random,
    _bisect,
    _heapq,
    _datetime,
    _decimal,
    _sre,
    _warnings,
    _codecs,
    _locale,
    _contextvars,
    _queue,
    _hashlib,
    _md5,
    _sha1,
    _sha2,
    _sha3,
    _blake2,
    _bz2,
    _lzma,
    _sqlite3,
    _elementtree,
    _asyncio,
    _typing,
    _opcode,
    _tokenize,
    _zoneinfo,
    _uuid,
    _statistics,
    _markupbase,
    _threading_local,
);
