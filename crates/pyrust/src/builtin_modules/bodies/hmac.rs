// `hmac` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/hmac.py` (issues #3146, #3444).
//
// `HMAC`, `new`, `digest`, `compare_digest`, `trans_5C`, `trans_36` and
// `digest_size`.  CPython routes a `str` or built-in constructor `digestmod`
// through OpenSSL (`_hashlib.hmac_new` / `hmac_digest`) and every other
// `digestmod` through the pure-Python RFC 2104 construction in `_init_old`;
// the same split is kept here: names and the `hashlib` constructors run on the
// native `hashlib` engine, while custom callables and PEP 247 modules are
// driven through their `update` / `digest` / `copy` methods.
//
// Reference: <https://docs.python.org/3/library/hmac.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use super::hashlib::{
    HmacAlgo, HmacState, builtin_constructor_name, extract_bytes_like, extract_hash_input,
    hex_digest, try_extract_bytes_like,
};
use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, extract_str_value, is_str_or_str_subclass,
    value_type_name_str,
};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

const MODULE_DOC: &str = "HMAC (Keyed-Hashing for Message Authentication) module.\n\nImplements the HMAC algorithm as described by RFC 2104.";

const HMAC_DOC: &str = "RFC 2104 HMAC class.  Also complies with RFC 4231.\n\n    This supports the API for Cryptographic Hash Functions (PEP 247).\n    ";

// `def`s of `Lib/hmac.py`: CPython reports arity errors for these in the
// interpreter's `initialize_locals` wording.
const SIG_INIT: Sig = Sig {
    qualname: "HMAC.__init__",
    params: &["self", "key", "msg", "digestmod"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};
const SIG_UPDATE: Sig = Sig {
    qualname: "HMAC.update",
    params: &["self", "msg"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_COPY: Sig = Sig {
    qualname: "HMAC.copy",
    params: &["self"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_DIGEST_METHOD: Sig = Sig {
    qualname: "HMAC.digest",
    params: &["self"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_HEXDIGEST_METHOD: Sig = Sig {
    qualname: "HMAC.hexdigest",
    params: &["self"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_NEW: Sig = Sig {
    qualname: "new",
    params: &["key", "msg", "digestmod"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};
const SIG_DIGEST: Sig = Sig {
    qualname: "digest",
    params: &["key", "msg", "digest"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

/// `HMAC.blocksize`: the default block size of custom `digestmod`s.
const DEFAULT_BLOCKSIZE: i64 = 64;

/// The native state behind an `HMAC` instance.
enum Engine {
    /// A `hashlib` algorithm run entirely in Rust.
    Native(Box<HmacState>),
    /// A custom `digestmod`: the two keyed hash objects it produced.
    Py { inner: Value, outer: Value },
}

/// How a `digestmod` argument is evaluated.
enum Resolved {
    Native(Box<HmacAlgo>),
    /// `shake_128` / `shake_256`: OpenSSL cannot key an XOF.
    Xof,
    /// A `str` naming no known hash.
    Unknown(String),
    Generic(Cons),
}

/// The `digest_cons` of `Lib/hmac.py`.
#[derive(Clone)]
enum Cons {
    /// `callable(digestmod)`: `digestmod(data)`.
    Callable(Value),
    /// A PEP 247 module: `digestmod.new(data)`.
    Module(Value),
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, pos)
}

fn is_callable(interp: &mut Interpreter, val: &Value) -> Result<bool> {
    let res = call_pos(
        interp,
        Value::builtin_function("callable"),
        std::slice::from_ref(val),
    )?;
    interp.truthy_value(&res)
}

fn binary(interp: &mut Interpreter, left: Value, op: BinaryOp, right: Value) -> Result<Value> {
    interp.eval_binary(left, op, right)
}

/// `isinstance(value, (bytes, bytearray))`.
fn is_bytes_or_bytearray(val: &Value) -> bool {
    let plain = |v: &Value| {
        matches!(v.kind(), ValueKind::Bytes(_))
            || pyrust_builtins::bytearray::as_bytearray_snapshot(v).is_some()
    };
    plain(val) || builtin_data_backing(val).is_some_and(|backing| plain(&backing))
}

/// `bytes((x ^ xor) for x in range(256))`.
fn translation_table(xor: u8) -> Vec<u8> {
    (0..=255u8).map(|x| x ^ xor).collect()
}

fn xof_error() -> PyError {
    value_error("no reason supplied")
}

fn unsupported_hash(name: &str) -> PyError {
    value_error(format!("unsupported hash type {name}"))
}

/// Decide how `digestmod` is evaluated.
fn resolve_digestmod(interp: &mut Interpreter, digestmod: &Value) -> Result<Resolved> {
    if is_str_or_str_subclass(digestmod) {
        let name = extract_str_value(digestmod);
        return Ok(match HmacAlgo::from_name(&name) {
            Some(algo) if algo.is_xof() => Resolved::Xof,
            Some(algo) => Resolved::Native(Box::new(algo)),
            None => Resolved::Unknown(name),
        });
    }
    if let Some(name) = builtin_constructor_name(interp, digestmod)? {
        return Ok(match HmacAlgo::from_name(name) {
            Some(algo) if algo.is_xof() => Resolved::Xof,
            Some(algo) => Resolved::Native(Box::new(algo)),
            None => Resolved::Unknown(name.to_string()),
        });
    }
    Ok(Resolved::Generic(if is_callable(interp, digestmod)? {
        Cons::Callable(digestmod.clone())
    } else {
        Cons::Module(digestmod.clone())
    }))
}

/// `digest_cons(data)` / `digest_cons()`.
fn construct(interp: &mut Interpreter, cons: &Cons, data: Option<&Value>) -> Result<Value> {
    match cons {
        Cons::Callable(ctor) => {
            let pos: &[Value] = data.map_or(&[], std::slice::from_ref);
            call_pos(interp, ctor.clone(), pos)
        }
        Cons::Module(module) => {
            let new = interp.get_attr(module, "new")?;
            let data = data.cloned().unwrap_or_else(|| Value::bytes(Vec::new()));
            call_pos(interp, new, &[data])
        }
    }
}

/// `warnings.warn(message, RuntimeWarning, stacklevel)`.
fn warn_runtime(interp: &mut Interpreter, message: Value) -> Result<()> {
    let warnings = interp.load_module("warnings")?;
    let warn = interp.get_attr(&warnings, "warn")?;
    let category = crate::interpreter::lookup_exc_class("RuntimeWarning")
        .map(Value::py_class)
        .unwrap_or_else(Value::none);
    call_pos(interp, warn, &[message, category, Value::int(2)])?;
    Ok(())
}

/// `fmt % value` with CPython's `%` formatting semantics.
fn percent_format(interp: &mut Interpreter, fmt: &str, value: Value) -> Result<Value> {
    binary(interp, Value::string(fmt), BinaryOp::Mod, value)
}

/// A block size as the count `bytes.ljust` accepts.
fn block_len(interp: &mut Interpreter, size: &Value) -> Result<usize> {
    let n = match size.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        _ => {
            let index = interp.try_value_to_index(size)?.ok_or_else(|| {
                type_error(format!(
                    "'{}' object cannot be interpreted as an integer",
                    value_type_name_str(size)
                ))
            })?;
            match index.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot fit 'int' into an index-sized integer",
                    ));
                }
            }
        }
    };
    Ok(usize::try_from(n.max(0)).unwrap_or(usize::MAX))
}

/// Build the keyed inner / outer hash objects of `HMAC._init_old`.
///
/// Returns `(engine, digest_size, block_size)`.
fn init_generic(
    interp: &mut Interpreter,
    this: &Value,
    key: &Value,
    key_bytes: Vec<u8>,
    cons: &Cons,
) -> Result<(Engine, Value, Value)> {
    let outer = construct(interp, cons, None)?;
    let inner = construct(interp, cons, None)?;
    let digest_size = interp.get_attr(&inner, "digest_size")?;

    let blocksize = match interp.get_attr(&inner, "block_size") {
        Ok(reported) => {
            let small = binary(interp, reported.clone(), BinaryOp::Lt, Value::int(16))?;
            if interp.truthy_value(&small)? {
                let default = interp.get_attr(this, "blocksize")?;
                let message = percent_format(
                    interp,
                    "block_size of %d seems too small; using our default of %d.",
                    Value::tuple(vec![reported, default.clone()]),
                )?;
                warn_runtime(interp, message)?;
                default
            } else {
                reported
            }
        }
        Err(ref e) if e.class_name_is("AttributeError") => {
            let default = interp.get_attr(this, "blocksize")?;
            let message = percent_format(
                interp,
                "No block_size attribute on given digest object; Assuming %d.",
                default.clone(),
            )?;
            warn_runtime(interp, message)?;
            default
        }
        Err(e) => return Err(e),
    };

    let mut key_bytes = key_bytes;
    let too_long = binary(
        interp,
        Value::int(key_bytes.len() as i64),
        BinaryOp::Gt,
        blocksize.clone(),
    )?;
    if interp.truthy_value(&too_long)? {
        let hashed = construct(interp, cons, Some(key))?;
        let hashed = call_method(interp, &hashed, "digest", &[])?;
        key_bytes = extract_bytes_like(&hashed)?;
    }
    let width = block_len(interp, &blocksize)?;
    if key_bytes.len() < width {
        key_bytes.resize(width, 0);
    }
    let outer_pad: Vec<u8> = key_bytes.iter().map(|b| b ^ 0x5c).collect();
    let inner_pad: Vec<u8> = key_bytes.iter().map(|b| b ^ 0x36).collect();
    call_method(interp, &outer, "update", &[Value::bytes(outer_pad)])?;
    call_method(interp, &inner, "update", &[Value::bytes(inner_pad)])?;
    Ok((Engine::Py { inner, outer }, digest_size, blocksize))
}

/// `HMAC.__init__(self, key, msg=None, digestmod='')`.
fn hmac_init(
    interp: &mut Interpreter,
    this: &Value,
    key: &Value,
    msg: &Value,
    digestmod: &Value,
) -> Result<()> {
    if !is_bytes_or_bytearray(key) {
        return Err(type_error(format!(
            "key: expected bytes or bytearray, but got '{}'",
            value_type_name_str(key)
        )));
    }
    if !interp.truthy_value(digestmod)? {
        return Err(type_error("Missing required argument 'digestmod'."));
    }
    let key_bytes = try_extract_bytes_like(key).unwrap_or_default();
    let (engine, digest_size, block_size) = match resolve_digestmod(interp, digestmod)? {
        Resolved::Xof => return Err(xof_error()),
        Resolved::Unknown(name) => return Err(unsupported_hash(&name)),
        Resolved::Native(algo) => {
            let state = HmacState::new(&algo, &key_bytes);
            let digest_size = Value::int(state.digest_size() as i64);
            let block_size = Value::int(state.block_size() as i64);
            (Engine::Native(Box::new(state)), digest_size, block_size)
        }
        Resolved::Generic(cons) => init_generic(interp, this, key, key_bytes, &cons)?,
    };
    interp.assign_attr(this.clone(), "digest_size", digest_size)?;
    interp.assign_attr(this.clone(), "block_size", block_size)?;
    set_native_state(this, engine)?;
    if !msg.is_none() {
        let update = interp.get_attr(this, "update")?;
        call_pos(interp, update, std::slice::from_ref(msg))?;
    }
    Ok(())
}

/// Run `f` on the engine of `this`.  An instance whose `__init__` never ran
/// (or a foreign object) fails like the first slot the Python code reads,
/// `attr`, being unset.
fn read_engine<R>(this: &Value, attr: &str, f: impl FnOnce(&Engine) -> Result<R>) -> Result<R> {
    if !has_native_state::<Engine>(this) {
        return Err(PyError::named(
            "AttributeError",
            format!(
                "'{}' object has no attribute '{attr}'",
                value_type_name_str(this)
            ),
        ));
    }
    with_native_state::<Engine, _>(this, "HMAC", f)
}

/// The inner hash object of a custom-`digestmod` instance, `None` for a
/// native one.
fn py_inner(this: &Value) -> Result<Option<Value>> {
    read_engine(this, "_hmac", |engine| {
        Ok(match engine {
            Engine::Native(_) => None,
            Engine::Py { inner, .. } => Some(inner.clone()),
        })
    })
}

/// `HMAC.update(self, msg)`.
fn hmac_update(interp: &mut Interpreter, this: &Value, msg: &Value) -> Result<()> {
    if let Some(inner) = py_inner(this)? {
        call_method(interp, &inner, "update", std::slice::from_ref(msg))?;
        return Ok(());
    }
    let data = extract_hash_input(msg)?;
    with_native_state_mut::<Engine, _>(this, "HMAC", |engine| {
        if let Engine::Native(state) = engine {
            state.update(&data);
        }
        Ok(())
    })
}

/// `HMAC.copy(self)`.
fn hmac_copy(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let ValueKind::PyInstance(instance) = this.kind() else {
        return Err(type_error(format!(
            "descriptor 'copy' requires a 'HMAC' object but received a '{}'",
            value_type_name_str(this)
        )));
    };
    let class = Rc::clone(&instance.borrow().class);
    let other = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })));
    // CPython 3.12.3's `HMAC.copy` copies `digest_size` but not `block_size`,
    // so `copy().block_size` raises `AttributeError`; kept for parity.
    let digest_size = interp.get_attr(this, "digest_size")?;
    interp.assign_attr(other.clone(), "digest_size", digest_size)?;
    let engine = read_engine(this, "_hmac", |engine| {
        Ok(match engine {
            Engine::Native(state) => Engine::Native(state.clone()),
            Engine::Py { inner, outer } => Engine::Py {
                inner: inner.clone(),
                outer: outer.clone(),
            },
        })
    })?;
    let engine = match engine {
        Engine::Native(state) => Engine::Native(state),
        Engine::Py { inner, outer } => {
            let inner = call_method(interp, &inner, "copy", &[])?;
            let outer = call_method(interp, &outer, "copy", &[])?;
            Engine::Py { inner, outer }
        }
    };
    set_native_state(&other, engine)?;
    Ok(other)
}

/// `HMAC.digest()` / `HMAC.hexdigest()`; neither alters the object.
fn hmac_current(interp: &mut Interpreter, this: &Value, hex: bool) -> Result<Value> {
    enum Snapshot {
        Native(Vec<u8>),
        Py(Value, Value),
    }
    let attr = if matches!(this.kind(), ValueKind::PyInstance(_)) {
        "_hmac"
    } else {
        "_current"
    };
    let snapshot = read_engine(this, attr, |engine| {
        Ok(match engine {
            Engine::Native(state) => Snapshot::Native(state.digest()),
            Engine::Py { inner, outer } => Snapshot::Py(inner.clone(), outer.clone()),
        })
    })?;
    match snapshot {
        Snapshot::Native(mac) => Ok(if hex {
            Value::string(hex_digest(&mac))
        } else {
            Value::bytes(mac)
        }),
        Snapshot::Py(inner, outer) => {
            let h = call_method(interp, &outer, "copy", &[])?;
            let inner_digest = call_method(interp, &inner, "digest", &[])?;
            call_method(interp, &h, "update", &[inner_digest])?;
            call_method(interp, &h, if hex { "hexdigest" } else { "digest" }, &[])
        }
    }
}

/// `HMAC.name`.
fn hmac_name(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    enum Named {
        Native(String),
        Py(Value),
    }
    let named = read_engine(this, "_hmac", |engine| {
        Ok(match engine {
            Engine::Native(state) => Named::Native(format!("hmac-{}", state.hash_name())),
            Engine::Py { inner, .. } => Named::Py(inner.clone()),
        })
    })?;
    match named {
        Named::Native(name) => Ok(Value::string(name)),
        Named::Py(inner) => {
            let name = interp.get_attr(&inner, "name")?;
            let text = call_pos(interp, Value::builtin_function("str"), &[name])?;
            Ok(Value::string(format!("hmac-{}", extract_str_value(&text))))
        }
    }
}

/// `hmac.digest(key, msg, digest)`.
fn hmac_digest(
    interp: &mut Interpreter,
    key: &Value,
    msg: &Value,
    digest: &Value,
) -> Result<Value> {
    let resolved = resolve_digestmod(interp, digest)?;
    let cons = match resolved {
        Resolved::Generic(cons) => cons,
        named => {
            // `_hashlib.hmac_digest` converts both buffers before it looks at
            // the algorithm.
            let key_bytes = extract_bytes_like(key)?;
            let msg_bytes = extract_bytes_like(msg)?;
            return match named {
                Resolved::Native(algo) => Ok(Value::bytes(algo.oneshot(&key_bytes, &msg_bytes))),
                Resolved::Xof => Err(xof_error()),
                Resolved::Unknown(name) => Err(unsupported_hash(&name)),
                Resolved::Generic(_) => unreachable!("generic digestmod handled above"),
            };
        }
    };

    let inner = construct(interp, &cons, None)?;
    let outer = construct(interp, &cons, None)?;
    let blocksize = match interp.get_attr(&inner, "block_size") {
        Ok(size) => size,
        Err(ref e) if e.class_name_is("AttributeError") => Value::int(DEFAULT_BLOCKSIZE),
        Err(e) => return Err(e),
    };
    let len = Value::builtin_function("len");
    let mut key = key.clone();
    let key_len = call_pos(interp, len.clone(), std::slice::from_ref(&key))?;
    let too_long = binary(interp, key_len, BinaryOp::Gt, blocksize.clone())?;
    if interp.truthy_value(&too_long)? {
        let hashed = construct(interp, &cons, Some(&key))?;
        key = call_method(interp, &hashed, "digest", &[])?;
    }
    let key_len = call_pos(interp, len, std::slice::from_ref(&key))?;
    let missing = binary(interp, blocksize, BinaryOp::Sub, key_len)?;
    let padding = binary(interp, Value::bytes(vec![0]), BinaryOp::Mul, missing)?;
    let key = binary(interp, key, BinaryOp::Add, padding)?;
    let inner_key = call_method(
        interp,
        &key,
        "translate",
        &[Value::bytes(translation_table(0x36))],
    )?;
    call_method(interp, &inner, "update", &[inner_key])?;
    let outer_key = call_method(
        interp,
        &key,
        "translate",
        &[Value::bytes(translation_table(0x5c))],
    )?;
    call_method(interp, &outer, "update", &[outer_key])?;
    call_method(interp, &inner, "update", std::slice::from_ref(msg))?;
    let inner_digest = call_method(interp, &inner, "digest", &[])?;
    call_method(interp, &outer, "update", &[inner_digest])?;
    call_method(interp, &outer, "digest", &[])
}

/// Slot-only `HMAC` instances (`__slots__`), like the Python class.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    let class = module_rc.borrow().attrs.get("HMAC").cloned();
    let Some(class) = class else {
        return;
    };
    let ValueKind::PyClass(class_rc) = class.kind() else {
        return;
    };
    class_rc.borrow_mut().slots = Some(
        ["block_size", "digest_size"]
            .into_iter()
            .map(String::from)
            .collect(),
    );
    // A `@property` bound in a class body knows its name.
    let name_prop = class_rc.borrow().attrs.get("name").cloned();
    if let Some(prop) = name_prop
        && let Ok(set_name) = interp.get_attr(&prop, "__set_name__")
    {
        let _ = call_pos(interp, set_name, &[class.clone(), Value::string("name")]);
    }
}

fn try_extract_str(val: &Value) -> Option<String> {
    if let ValueKind::Str(s) = val.kind() {
        return Some(s.to_string());
    }
    if let Some(backing) = builtin_data_backing(val)
        && let ValueKind::Str(s) = backing.kind()
    {
        return Some(s.to_string());
    }
    None
}

/// Constant-time byte slice equality matching CPython's `_Py_compare_digest`.
/// When lengths differ, compares `b` against itself so runtime depends only on
/// `b.len()`, and `black_box` prevents short-circuit optimization.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff: usize = a.len() ^ b.len();
    let left = if a.len() == b.len() { a } else { b };
    for (&x, &y) in left.iter().zip(b.iter()) {
        diff |= usize::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

/// `_hashlib.compare_digest(a, b, /)`.
fn compare_digest_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.iter().any(|arg| arg.name.is_some()) {
        return Err(type_error(
            "_hashlib.compare_digest() takes no keyword arguments",
        ));
    }
    if args.len() != 2 {
        return Err(type_error(format!(
            "compare_digest expected 2 arguments, got {}",
            args.len()
        )));
    }
    let a = &args[0].value;
    let b = &args[1].value;

    if let (Some(sa), Some(sb)) = (try_extract_str(a), try_extract_str(b)) {
        if !sa.is_ascii() || !sb.is_ascii() {
            return Err(type_error(
                "comparing strings with non-ASCII characters is not supported",
            ));
        }
        return Ok(Value::bool_(constant_time_eq(sa.as_bytes(), sb.as_bytes())));
    }

    let not_bytes_like = |val: &Value| {
        type_error(format!(
            "a bytes-like object is required, not '{}'",
            value_type_name_str(val)
        ))
    };
    // `PyObject_GetBuffer(PyBUF_SIMPLE)`: a memoryview that is not a simple
    // buffer raises `BufferError`; any other non-buffer is `None`.
    let buffer_of = |val: &Value| -> Result<Option<Vec<u8>>> {
        if pyrust_builtins::memoryview::is_memoryview(val) {
            return extract_bytes_like(val).map(Some);
        }
        Ok(try_extract_bytes_like(val))
    };
    let is_buffer = |val: &Value| {
        pyrust_builtins::memoryview::is_memoryview(val) || try_extract_bytes_like(val).is_some()
    };
    if !is_buffer(a) && !is_buffer(b) {
        return Err(type_error(format!(
            "unsupported operand types(s) or combination of types: '{}' and '{}'",
            value_type_name_str(a),
            value_type_name_str(b)
        )));
    }
    let Some(bytes_a) = buffer_of(a)? else {
        return Err(not_bytes_like(a));
    };
    let Some(bytes_b) = buffer_of(b)? else {
        return Err(not_bytes_like(b));
    };
    Ok(Value::bool_(constant_time_eq(&bytes_a, &bytes_b)))
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "trans_5C" => Value::bytes(translation_table(0x5c)),
        "trans_36" => Value::bytes(translation_table(0x36)),
        "digest_size" => Value::none(),
    }

    #[native_state(Engine)]
    class HMAC {
        constants {
            "__doc__" => Value::string(HMAC_DOC),
            "blocksize" => Value::int(DEFAULT_BLOCKSIZE),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_INIT, args)?;
            let this = b.take_or_none(0);
            let key = b.take_or_none(1);
            let msg = b.take_or_none(2);
            let digestmod = b.take_or(3, || Value::string(""));
            hmac_init(_interp, &this, &key, &msg, &digestmod)?;
            Ok(Value::none())
        }

        /// Feed data from msg into this hashing object.
        fn update(args) -> Result<Value> {
            let mut b = args::bind(&SIG_UPDATE, args)?;
            let this = b.take_or_none(0);
            let msg = b.take_or_none(1);
            hmac_update(_interp, &this, &msg)?;
            Ok(Value::none())
        }

        /// Return a separate copy of this hashing object.
        ///
        /// An update to this copy won't affect the original object.
        fn copy(args) -> Result<Value> {
            let mut b = args::bind(&SIG_COPY, args)?;
            let this = b.take_or_none(0);
            hmac_copy(_interp, &this)
        }

        /// Return the hash value of this hashing object.
        ///
        /// This returns the hmac value as bytes.  The object is
        /// not altered in any way by this function; you can continue
        /// updating the object after calling this function.
        fn digest(args) -> Result<Value> {
            let mut b = args::bind(&SIG_DIGEST_METHOD, args)?;
            let this = b.take_or_none(0);
            hmac_current(_interp, &this, false)
        }

        /// Like digest(), but returns a string of hexadecimal digits instead.
        fn hexdigest(args) -> Result<Value> {
            let mut b = args::bind(&SIG_HEXDIGEST_METHOD, args)?;
            let this = b.take_or_none(0);
            hmac_current(_interp, &this, true)
        }

        /// `copy.copy()`: CPython shallow-copies the slots (sharing the
        /// underlying hash state), which the invisible native state cannot
        /// express, so hand out an independent copy instead.
        fn __copy__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_COPY, args)?;
            let this = b.take_or_none(0);
            hmac_copy(_interp, &this)
        }

        /// The OpenSSL-backed object CPython wraps cannot be pickled or
        /// deep-copied.
        fn __reduce_ex__(args) -> Result<Value> {
            let this = args.first().map_or_else(Value::none, |arg| arg.value.clone());
            let native = read_engine(&this, "_hmac", |engine| {
                Ok(matches!(engine, Engine::Native(_)))
            })?;
            Err(type_error(if native {
                "cannot pickle '_hashlib.HMAC' object"
            } else {
                "cannot pickle 'hmac.HMAC' object"
            }))
        }

        #[getter]
        fn name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            hmac_name(_interp, self_val.0)
        }
    }

    /// Create a new hashing object and return it.
    ///
    /// key: bytes or buffer, The starting key for the hash.
    /// msg: bytes or buffer, Initial input for the hash, or None.
    /// digestmod: A hash name suitable for hashlib.new(). *OR*
    ///            A hashlib constructor returning a new hash object. *OR*
    ///            A module supporting PEP 247.
    ///
    ///            Required as of 3.8, despite its position after the optional
    ///            msg argument.  Passing it as a keyword argument is
    ///            recommended, though not required for legacy API reasons.
    ///
    /// You can now feed arbitrary bytes into the object using its update()
    /// method, and can ask for the hash value at any time by calling its digest()
    /// or hexdigest() methods.
    fn new(args) -> Result<Value> {
        let mut b = args::bind(&SIG_NEW, args)?;
        let key = b.take_or_none(0);
        let msg = b.take_or_none(1);
        let digestmod = b.take_or(2, || Value::string(""));
        let module = _interp.load_module("hmac")?;
        let class = _interp.get_attr(&module, "HMAC")?;
        call_pos(_interp, class, &[key, msg, digestmod])
    }

    /// Fast inline implementation of HMAC.
    ///
    /// key: bytes or buffer, The key for the keyed hash object.
    /// msg: bytes or buffer, Input message.
    /// digest: A hash name suitable for hashlib.new() for best performance. *OR*
    ///         A hashlib constructor returning a new hash object. *OR*
    ///         A module supporting PEP 247.
    fn digest(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DIGEST, args)?;
        let key = b.take_or_none(0);
        let msg = b.take_or_none(1);
        let digest = b.take_or_none(2);
        hmac_digest(_interp, &key, &msg, &digest)
    }

    /// Return 'a == b'.
    ///
    /// This function uses an approach designed to prevent
    /// timing analysis, making it appropriate for cryptography.
    fn compare_digest(args) -> Result<Value> {
        compare_digest_impl(args)
    }
}
