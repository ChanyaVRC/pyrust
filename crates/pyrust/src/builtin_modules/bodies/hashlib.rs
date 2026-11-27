// `hashlib` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/hashlib.py` plus the `_hashlib` / `_blake2` accelerators it wraps
// (issues #3146, #3372).
//
// Pure-Rust implementations of MD5, SHA-1, SHA-224/256/384/512, SHA-512/224,
// SHA-512/256, SHA3-224/256/384/512, SHAKE128/256, BLAKE2b, BLAKE2s, HMAC,
// PBKDF2-HMAC and scrypt live under `bodies/hashlib/`.  This file is the
// Python-visible surface: the named constructors, `new`, `pbkdf2_hmac`,
// `scrypt`, `file_digest`, `algorithms_guaranteed` / `algorithms_available`,
// and the `HASH` / `HASHXOF` / `blake2b` / `blake2s` object types.
//
// `HASH` and `HASHXOF` are reported under `_hashlib` and `blake2b` / `blake2s`
// under `_blake2`, as in CPython.  `HASH` / `HASHXOF` are not attributes of
// `hashlib` itself, so `prepare_module` moves them out of the module namespace
// into per-GIL-group storage that the constructors read.
//
// Reference: <https://docs.python.org/3/library/hashlib.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, is_str_or_str_subclass, render_value_repr,
    value_type_name_str,
};
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyKey, PySet, Value, ValueKind};
use pyrust_core::PyToPrimitive;
use pyrust_derive::pyrust_module;

#[path = "hashlib/args.rs"]
mod args;
#[path = "hashlib/blake2.rs"]
mod blake2;
#[path = "hashlib/engine.rs"]
mod engine;
#[path = "hashlib/md5.rs"]
mod md5;
#[path = "hashlib/scrypt.rs"]
mod scrypt;
#[path = "hashlib/sha1.rs"]
mod sha1;
#[path = "hashlib/sha2.rs"]
mod sha2;
#[path = "hashlib/sha3.rs"]
mod sha3;

use self::args::{Sig, no_args, one_arg, unpack};
use self::blake2::Blake2Params;
use self::engine::{HashState, hex_encode, pbkdf2_hmac};
pub(crate) use self::engine::{HmacAlgo, HmacState};

type ClassRc = Rc<RefCell<PyClass>>;

const MODULE_DOC: &str = "hashlib module - A common interface to many hash functions.\n\nnew(name, data=b'', **kwargs) - returns a new hash object implementing the\n                                given hash function; initializing the hash\n                                using the given binary data.\n\nNamed constructor functions are also available, these are faster\nthan using new(name):\n\nmd5(), sha1(), sha224(), sha256(), sha384(), sha512(), blake2b(), blake2s(),\nsha3_224, sha3_256, sha3_384, sha3_512, shake_128, and shake_256.\n\nMore algorithms may be available on your platform but the above are guaranteed\nto exist.  See the algorithms_guaranteed and algorithms_available attributes\nto find out what algorithm names can be passed to new().\n\nNOTE: If you want the adler32 or crc32 hash functions they are available in\nthe zlib module.\n\nChoose your hash function wisely.  Some have known collision weaknesses.\nsha384 and sha512 will be slow on 32 bit platforms.\n\nHash objects have these methods:\n - update(data): Update the hash object with the bytes in data. Repeated calls\n                 are equivalent to a single call with the concatenation of all\n                 the arguments.\n - digest():     Return the digest of the bytes passed to the update() method\n                 so far as a bytes object.\n - hexdigest():  Like digest() except the digest is returned as a string\n                 of double length, containing only hexadecimal digits.\n - copy():       Return a copy (clone) of the hash object. This can be used to\n                 efficiently compute the digests of datas that share a common\n                 initial substring.\n\nFor example, to obtain the digest of the byte string 'Nobody inspects the\nspammish repetition':\n\n    >>> import hashlib\n    >>> m = hashlib.md5()\n    >>> m.update(b\"Nobody inspects\")\n    >>> m.update(b\" the spammish repetition\")\n    >>> m.digest()\n    b'\\xbbd\\x9c\\x83\\xdd\\x1e\\xa5\\xc9\\xd9\\xde\\xc9\\xa1\\x8d\\xf0\\xff\\xe9'\n\nMore condensed:\n\n    >>> hashlib.sha224(b\"Nobody inspects the spammish repetition\").hexdigest()\n    'a4337bc45a8fc544c03f52dc550cd6e1e87021bc896588bd79e901e2'\n";

const HASH_DOC: &str = "A hash is an object used to calculate a checksum of a string of information.\n\nMethods:\n\nupdate() -- updates the current digest with an additional string\ndigest() -- return the current digest value\nhexdigest() -- return the current digest as a string of hexadecimal digits\ncopy() -- return a copy of the current hash object\n\nAttributes:\n\nname -- the hash algorithm being used by this object\ndigest_size -- number of bytes in this hashes output";

const HASHXOF_DOC: &str = "A hash is an object used to calculate a checksum of a string of information.\n\nMethods:\n\nupdate() -- updates the current digest with an additional string\ndigest(length) -- return the current digest value\nhexdigest(length) -- return the current digest as a string of hexadecimal digits\ncopy() -- return a copy of the current hash object\n\nAttributes:\n\nname -- the hash algorithm being used by this object\ndigest_size -- number of bytes in this hashes output";

/// Names guaranteed on every platform (`hashlib.algorithms_guaranteed`).
const ALGORITHMS_GUARANTEED: [&str; 14] = [
    "md5",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512",
    "blake2b",
    "blake2s",
    "sha3_224",
    "sha3_256",
    "sha3_384",
    "sha3_512",
    "shake_128",
    "shake_256",
];

/// Extra names `hashlib.new()` accepts beyond the guaranteed set.
const ALGORITHMS_EXTRA: [&str; 2] = ["sha512_224", "sha512_256"];

/// `hashlib.__all__` (a tuple, as in CPython).
const ALL_NAMES: [&str; 19] = [
    "md5",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512",
    "blake2b",
    "blake2s",
    "sha3_224",
    "sha3_256",
    "sha3_384",
    "sha3_512",
    "shake_128",
    "shake_256",
    "new",
    "algorithms_guaranteed",
    "algorithms_available",
    "file_digest",
    "pbkdf2_hmac",
];

/// Default `bufsize` of `file_digest` (`2**18`).
const FILE_DIGEST_BUFSIZE: i64 = 1 << 18;

fn name_set(names: &[&[&str]]) -> Value {
    let mut set = PySet::default();
    for group in names {
        for name in *group {
            set.insert(PyKey::str_from(name));
        }
    }
    Value::set(set)
}

// ── Native hash object classes ───────────────────────────────────────────────

/// The four object types, resolved once per `hashlib` import.
#[derive(Clone)]
struct HashClasses {
    hash: ClassRc,
    xof: ClassRc,
    blake2b: ClassRc,
    blake2s: ClassRc,
}

// Thread-local justification:
// (a) Why `thread_local!`-style storage: `PyClass` is `!Send + !Sync`, so the
//     class objects cannot live in a process-global `OnceLock`.  `HASH` and
//     `HASHXOF` are deliberately not attributes of the `hashlib` module (as in
//     CPython), so the constructors need a side channel to reach them.
// (b) Multi-interpreter / multi-threaded impact: interpreters on one GIL group
//     share the most recently imported generation; instances keep the class
//     they were created with, so a re-import never invalidates live objects.
pyrust_core::gil_local! {
    static HASH_CLASSES: RefCell<Option<HashClasses>> = const { RefCell::new(None) };
}

/// Post-import wiring: hide `HASH` / `HASHXOF`, make every type slot-only
/// (no `__dict__`, like the C types), and forbid subclassing the BLAKE2 types.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    let class_of = |value: Option<Value>| -> Option<ClassRc> {
        match value?.kind() {
            ValueKind::PyClass(class) => Some(Rc::clone(class)),
            _ => None,
        }
    };
    let (hash, xof, blake2b, blake2s) = {
        let mut module_mut = module_rc.borrow_mut();
        let hash = class_of(module_mut.attrs.shift_remove("HASH"));
        let xof = class_of(module_mut.attrs.shift_remove("HASHXOF"));
        let blake2b = class_of(module_mut.attrs.get("blake2b").cloned());
        let blake2s = class_of(module_mut.attrs.get("blake2s").cloned());
        (hash, xof, blake2b, blake2s)
    };
    let (Some(hash), Some(xof), Some(blake2b), Some(blake2s)) = (hash, xof, blake2b, blake2s)
    else {
        return;
    };
    for (class, tp_name) in [
        (&hash, "_hashlib.HASH"),
        (&xof, "_hashlib.HASHXOF"),
        (&blake2b, "_blake2.blake2b"),
        (&blake2s, "_blake2.blake2s"),
    ] {
        let mut class_mut = class.borrow_mut();
        class_mut.slots = Some(indexmap::IndexSet::new());
        class_mut.error_name = Some(tp_name);
    }
    blake2b.borrow_mut().non_subclassable_name = Some("_blake2.blake2b");
    blake2s.borrow_mut().non_subclassable_name = Some("_blake2.blake2s");
    HASH_CLASSES.with(|slot| {
        *slot.borrow_mut() = Some(HashClasses {
            hash,
            xof,
            blake2b,
            blake2s,
        });
    });
}

fn hash_classes() -> Result<HashClasses> {
    HASH_CLASSES
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| PyError::runtime("hashlib is not initialised".to_string()))
}

/// A bare slot-only instance of `class`; the caller attaches the state.
fn new_instance(class: &ClassRc) -> Value {
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(class),
        attrs: InstanceAttrs::new(),
    })))
}

/// Wrap `state` in the object type CPython would use for it.
fn wrap_state(state: HashState) -> Result<Value> {
    let classes = hash_classes()?;
    let class = match state.name() {
        "blake2b" => &classes.blake2b,
        "blake2s" => &classes.blake2s,
        _ if state.is_shake() => &classes.xof,
        _ => &classes.hash,
    };
    let instance = new_instance(class);
    set_native_state(&instance, state)?;
    Ok(instance)
}

// ── Shared helpers for `hmac` fast path ──────────────────────────────────────

/// Names of the `hashlib` constructors `hmac` runs natively.
const HMAC_CONSTRUCTOR_NAMES: [&str; 14] = [
    "md5",
    "sha1",
    "sha224",
    "sha256",
    "sha384",
    "sha512",
    "sha3_224",
    "sha3_256",
    "sha3_384",
    "sha3_512",
    "shake_128",
    "shake_256",
    "blake2b",
    "blake2s",
];

/// The `hashlib` algorithm name behind `ctor` when it is one of the named
/// `hashlib` constructors themselves (`hashlib.sha256`, `hashlib.blake2b`, ...),
/// the objects `hmac` maps straight onto its native engine.
pub(crate) fn builtin_constructor_name(
    interp: &mut Interpreter,
    ctor: &Value,
) -> Result<Option<&'static str>> {
    let module = interp.load_module("hashlib")?;
    for name in HMAC_CONSTRUCTOR_NAMES {
        let candidate = interp.get_attr(&module, name)?;
        if candidate.is_identical_to(ctor) {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// Lowercase hexadecimal rendering of `bytes`.
pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    hex_encode(bytes)
}

/// Compute a one-shot lowercase hexadecimal MD5 digest in native Rust.
pub(crate) fn md5_hexdigest(data: &[u8]) -> String {
    let mut state = HashState::from_name("md5").expect("md5 hash algorithm must exist");
    state.update(data);
    hex_encode(&state.digest())
}

// ── Buffer & integer extraction helpers ──────────────────────────────────────

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

fn overflow_error(msg: impl Into<String>) -> PyError {
    PyError::named("OverflowError", msg.into())
}

fn is_str_value(val: &Value) -> bool {
    if matches!(val.kind(), ValueKind::Str(_)) {
        return true;
    }
    if let Some(backing) = builtin_data_backing(val)
        && matches!(backing.kind(), ValueKind::Str(_))
    {
        return true;
    }
    false
}

pub(crate) fn try_extract_bytes_like(val: &Value) -> Option<Vec<u8>> {
    if let ValueKind::Bytes(rc) = val.kind() {
        return Some(rc.as_ref().clone());
    }
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
        return Some(snap);
    }
    if pyrust_builtins::memoryview::is_memoryview(val) {
        return pyrust_builtins::memoryview::to_bytes_checked(val).ok();
    }
    if let Some(backing) = builtin_data_backing(val) {
        if let ValueKind::Bytes(rc) = backing.kind() {
            return Some(rc.as_ref().clone());
        }
        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
            return Some(snap);
        }
    }
    if let Some(snap) = super::array::buffer_snapshot(val) {
        return Some(snap);
    }
    None
}

pub(crate) fn extract_bytes_like(val: &Value) -> Result<Vec<u8>> {
    if pyrust_builtins::memoryview::is_memoryview(val) {
        return pyrust_builtins::memoryview::to_simple_buffer_bytes(val);
    }
    try_extract_bytes_like(val).ok_or_else(|| {
        type_error(format!(
            "a bytes-like object is required, not '{}'",
            value_type_name_str(val)
        ))
    })
}

/// The buffer argument of a hash constructor / `update`: `str` is rejected
/// with CPython's dedicated wording.
pub(crate) fn extract_hash_input(val: &Value) -> Result<Vec<u8>> {
    if is_str_value(val) {
        return Err(type_error("Strings must be encoded before hashing"));
    }
    if pyrust_builtins::memoryview::is_memoryview(val) {
        return pyrust_builtins::memoryview::to_simple_buffer_bytes(val);
    }
    try_extract_bytes_like(val)
        .ok_or_else(|| type_error("object supporting the buffer API required"))
}

fn not_an_integer(val: &Value) -> PyError {
    type_error(format!(
        "'{}' object cannot be interpreted as an integer",
        value_type_name_str(val)
    ))
}

/// `PyNumber_Index` as an `i128`; `None` means the value does not fit (it is
/// then out of range for every C integer type this module converts to).
fn index_i128(interp: &mut Interpreter, val: &Value) -> Result<Option<i128>> {
    let idx = match val.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => val.clone(),
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => interp
            .try_value_to_index(val)?
            .ok_or_else(|| not_an_integer(val))?,
        _ => return Err(not_an_integer(val)),
    };
    match idx.kind() {
        ValueKind::Bool(b) => Ok(Some(i128::from(b))),
        ValueKind::Int(n) => Ok(Some(i128::from(n))),
        ValueKind::BigInt(bi) => Ok(bi.to_i128()),
        _ => Err(not_an_integer(val)),
    }
}

/// Convert to a C integer type, raising CPython's `OverflowError` wording.
fn index_in_range(
    interp: &mut Interpreter,
    val: &Value,
    min: i128,
    max: i128,
    c_type: &str,
) -> Result<i128> {
    match index_i128(interp, val)? {
        Some(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(overflow_error(format!(
            "Python int too large to convert to C {c_type}"
        ))),
    }
}

fn c_int(interp: &mut Interpreter, val: &Value) -> Result<i128> {
    index_in_range(
        interp,
        val,
        i128::from(i32::MIN),
        i128::from(i32::MAX),
        "int",
    )
}

fn c_long(interp: &mut Interpreter, val: &Value) -> Result<i128> {
    index_in_range(
        interp,
        val,
        i128::from(i64::MIN),
        i128::from(i64::MAX),
        "long",
    )
}

fn c_ssize_t(interp: &mut Interpreter, val: &Value) -> Result<i128> {
    index_in_range(
        interp,
        val,
        i128::from(isize::MIN as i64),
        i128::from(isize::MAX as i64),
        "ssize_t",
    )
}

/// `str` argument (exact `str` or subclass) or the clinic `must be str` error.
fn str_arg(fn_name: &str, param: &str, val: &Value) -> Result<String> {
    if is_str_or_str_subclass(val) {
        return Ok(crate::interpreter::extract_str_value(val));
    }
    Err(bad_argument(fn_name, param, "str", val))
}

/// `_PyArg_BadArgument`: `None` is spelled `None`, other types by name.
fn bad_argument(fn_name: &str, param: &str, expected: &str, val: &Value) -> PyError {
    let actual = if val.is_none() {
        "None".to_string()
    } else {
        value_type_name_str(val).into_owned()
    };
    type_error(format!(
        "{fn_name}() argument '{param}' must be {expected}, not {actual}"
    ))
}

// ── Hash construction ────────────────────────────────────────────────────────

const ALGORITHM_SIG_ARGS: [&str; 1] = ["string"];
const USEDFORSECURITY: [&str; 1] = ["usedforsecurity"];

/// Shared body of `md5()` ... `shake_256()` (`_hashlib.openssl_*`).
fn construct_named(
    interp: &mut Interpreter,
    fn_name: &'static str,
    algorithm: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let sig = Sig {
        name: fn_name,
        positional: &ALGORITHM_SIG_ARGS,
        positional_only: 0,
        required: 0,
        kwonly: &USEDFORSECURITY,
    };
    let slots = unpack(&sig, args)?;
    if let Some(flag) = &slots[1] {
        interp.truthy_value(flag)?;
    }
    let mut state = HashState::from_name(algorithm)
        .ok_or_else(|| value_error(format!("unsupported hash type {algorithm}")))?;
    if let Some(data) = &slots[0] {
        let data = extract_hash_input(data)?;
        state.update(&data);
    }
    wrap_state(state)
}

/// Resolve `name` / `data` / `**kwargs` the way `hashlib.new` does: the BLAKE2
/// names go to the builtin constructors, everything else to `_hashlib.new`.
fn hash_new(
    interp: &mut Interpreter,
    name: &Value,
    data: Option<Value>,
    kwargs: Vec<ExpandedCallArg>,
) -> Result<Value> {
    if is_str_or_str_subclass(name) {
        let key = crate::interpreter::extract_str_value(name);
        if key == "blake2b" || key == "blake2s" {
            let mut call_args = Vec::with_capacity(kwargs.len() + 1);
            if let Some(data) = data {
                call_args.push(ExpandedCallArg {
                    name: None,
                    value: data,
                });
            }
            call_args.extend(kwargs);
            let classes = hash_classes()?;
            let is_b = key == "blake2b";
            let class = if is_b {
                &classes.blake2b
            } else {
                &classes.blake2s
            };
            return blake2_construct(interp, is_b, class, &call_args);
        }
    }
    let sig = Sig {
        name: "new",
        positional: &["name", "string"],
        positional_only: 0,
        required: 1,
        kwonly: &USEDFORSECURITY,
    };
    let mut call_args = vec![ExpandedCallArg {
        name: None,
        value: name.clone(),
    }];
    call_args.push(ExpandedCallArg {
        name: None,
        value: data.unwrap_or_else(|| Value::bytes(Vec::new())),
    });
    call_args.extend(kwargs);
    let slots = unpack(&sig, &call_args)?;
    if !is_str_or_str_subclass(&call_args[0].value) {
        return Err(type_error("name must be a string"));
    }
    let name = crate::interpreter::extract_str_value(&call_args[0].value);
    if let Some(flag) = &slots[2] {
        interp.truthy_value(flag)?;
    }
    let mut state = HashState::from_name(&name)
        .ok_or_else(|| value_error(format!("unsupported hash type {name}")))?;
    if let Some(data) = &slots[1] {
        let data = extract_hash_input(data)?;
        state.update(&data);
    }
    wrap_state(state)
}

// ── BLAKE2 ───────────────────────────────────────────────────────────────────

const BLAKE2_KWONLY: [&str; 12] = [
    "digest_size",
    "key",
    "salt",
    "person",
    "fanout",
    "depth",
    "leaf_size",
    "node_offset",
    "node_depth",
    "inner_size",
    "last_node",
    "usedforsecurity",
];

/// `blake2b(data=b'', /, *, digest_size=64, key=b'', ...)` and the `blake2s`
/// twin: parse, validate and build a new instance of `class`.
fn blake2_construct(
    interp: &mut Interpreter,
    is_b: bool,
    class: &ClassRc,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let fn_name = if is_b { "blake2b" } else { "blake2s" };
    let sig = Sig {
        name: fn_name,
        positional: &["data"],
        positional_only: 1,
        required: 0,
        kwonly: &BLAKE2_KWONLY,
    };
    let slots = unpack(&sig, args)?;
    let max_digest: i128 = if is_b { 64 } else { 32 };
    let max_salt: usize = if is_b { 16 } else { 8 };

    // Argument Clinic converts every parameter before the body validates any.
    let digest_size = match &slots[1] {
        Some(v) => c_int(interp, v)?,
        None => max_digest,
    };
    let key = bytes_slot(&slots[2])?;
    let salt = bytes_slot(&slots[3])?;
    let person = bytes_slot(&slots[4])?;
    let fanout = int_slot(interp, &slots[5], 1)?;
    let depth = int_slot(interp, &slots[6], 1)?;
    let leaf_size = match &slots[7] {
        Some(v) => match index_i128(interp, v)? {
            Some(n) if n < 0 => return Err(value_error("value must be positive")),
            Some(n) if n <= i128::from(u64::MAX) => n,
            _ => {
                return Err(overflow_error(
                    "Python int too large to convert to C unsigned long",
                ));
            }
        },
        None => 0,
    };
    let node_offset = match &slots[8] {
        Some(v) => {
            if !matches!(
                v.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            ) {
                return Err(type_error("an integer is required"));
            }
            match index_i128(interp, v)? {
                Some(n) if n < 0 => return Err(value_error("value must be positive")),
                Some(n) if n <= i128::from(u64::MAX) => n,
                _ => return Err(overflow_error("int too big to convert")),
            }
        }
        None => 0,
    };
    let node_depth = int_slot(interp, &slots[9], 0)?;
    let inner_size = int_slot(interp, &slots[10], 0)?;
    let last_node = match &slots[11] {
        Some(v) => interp.truthy_value(v)?,
        None => false,
    };
    if let Some(flag) = &slots[12] {
        interp.truthy_value(flag)?;
    }

    if digest_size < 1 || digest_size > max_digest {
        return Err(value_error(format!(
            "digest_size must be between 1 and {max_digest} bytes"
        )));
    }
    if key.len() as i128 > max_digest {
        return Err(value_error(format!(
            "maximum key length is {max_digest} bytes"
        )));
    }
    if salt.len() > max_salt {
        return Err(value_error(format!(
            "maximum salt length is {max_salt} bytes"
        )));
    }
    if person.len() > max_salt {
        return Err(value_error(format!(
            "maximum person length is {max_salt} bytes"
        )));
    }
    if !(0..=255).contains(&fanout) {
        return Err(value_error("fanout must be between 0 and 255"));
    }
    if !(1..=255).contains(&depth) {
        return Err(value_error("depth must be between 1 and 255"));
    }
    let leaf_size =
        u32::try_from(leaf_size).map_err(|_| overflow_error("leaf_size is too large"))?;
    let max_node_offset: i128 = if is_b {
        i128::from(u64::MAX)
    } else {
        (1i128 << 48) - 1
    };
    if node_offset > max_node_offset {
        return Err(overflow_error("node_offset is too large"));
    }
    if !(0..=255).contains(&node_depth) {
        return Err(value_error("node_depth must be between 0 and 255"));
    }
    if inner_size < 0 || inner_size > max_digest {
        return Err(value_error(format!(
            "inner_size must be between 0 and is {max_digest}"
        )));
    }
    let data = match &slots[0] {
        Some(v) => Some(extract_hash_input(v)?),
        None => None,
    };

    let params = Blake2Params {
        digest_size: digest_size as u8,
        key: &key,
        salt: &salt,
        person: &person,
        fanout: fanout as u8,
        depth: depth as u8,
        leaf_size,
        node_offset: node_offset as u64,
        node_depth: node_depth as u8,
        inner_size: inner_size as u8,
        last_node,
    };
    let mut state = if is_b {
        HashState::new_blake2b(params)
    } else {
        HashState::new_blake2s(params)
    };
    if let Some(data) = data {
        state.update(&data);
    }
    let instance = new_instance(class);
    set_native_state(&instance, state)?;
    Ok(instance)
}

fn bytes_slot(slot: &Option<Value>) -> Result<Vec<u8>> {
    match slot {
        Some(v) => extract_bytes_like(v),
        None => Ok(Vec::new()),
    }
}

fn int_slot(interp: &mut Interpreter, slot: &Option<Value>, default: i128) -> Result<i128> {
    match slot {
        Some(v) => c_int(interp, v),
        None => Ok(default),
    }
}

// ── Digest helpers shared by the object types ────────────────────────────────

/// Largest XOF output we are willing to allocate (`MemoryError` beyond).
const MAX_XOF_BYTES: i128 = 1 << 31;

/// `digest(length)` / `hexdigest(length)` of a SHAKE object.
fn xof_output(
    interp: &mut Interpreter,
    state: &HashState,
    args: &[ExpandedCallArg],
    hex: bool,
) -> Result<Value> {
    let sig = Sig {
        name: if hex { "hexdigest" } else { "digest" },
        positional: &["length"],
        positional_only: 0,
        required: 1,
        kwonly: &[],
    };
    let slots = unpack(&sig, &args[1..])?;
    let length = c_ssize_t(interp, slots[0].as_ref().expect("required slot"))?;
    if length < 0 {
        return Err(if hex {
            PyError::named("MemoryError", String::new())
        } else {
            PyError::named(
                "SystemError",
                "Negative size passed to PyBytes_FromStringAndSize",
            )
        });
    }
    if length > MAX_XOF_BYTES / if hex { 2 } else { 1 } {
        return Err(PyError::named("MemoryError", String::new()));
    }
    let bytes = state.digest_xof(length as usize);
    Ok(if hex {
        Value::string(hex_encode(&bytes))
    } else {
        Value::bytes(bytes)
    })
}

fn receiver(args: &[ExpandedCallArg]) -> Result<&Value> {
    args.first()
        .map(|arg| &arg.value)
        .ok_or_else(|| type_error("descriptor requires an object but received no arguments"))
}

/// `copy` / `pickle` of a hash object: CPython's C types are not picklable.
fn cannot_pickle(state: &HashState) -> PyError {
    let tp_name = match state.name() {
        "blake2b" => "_blake2.blake2b",
        "blake2s" => "_blake2.blake2s",
        _ if state.is_shake() => "_hashlib.HASHXOF",
        _ => "_hashlib.HASH",
    };
    type_error(format!("cannot pickle '{tp_name}' object"))
}

fn reduce_ex(args: &[ExpandedCallArg]) -> Result<Value> {
    let state =
        with_native_state::<HashState, _>(receiver(args)?, "HASH", |state| Ok(state.clone()))?;
    Err(cannot_pickle(&state))
}

fn hash_repr(interp: &mut Interpreter, state: &HashState, this: &Value) -> Value {
    let _ = interp;
    let type_name = if state.is_shake() { "HASHXOF" } else { "HASH" };
    Value::string(format!(
        "<{} _hashlib.{type_name} object @ {:#x}>",
        state.name(),
        this.object_id().as_int().unwrap_or(0)
    ))
}

// ── Module-level key derivation ──────────────────────────────────────────────

/// `_hashlib.UnsupportedDigestmodError` (a `ValueError`), raised with
/// OpenSSL's `unsupported` reason like CPython does for an unknown digest.
fn unsupported_digest(interp: &mut Interpreter) -> PyError {
    let message = "[digital envelope routines] unsupported";
    let class = interp
        .load_module("_hashlib")
        .and_then(|module| interp.get_attr(&module, "UnsupportedDigestmodError"))
        .ok()
        .and_then(|class| match class.kind() {
            ValueKind::PyClass(rc) => Some(Rc::clone(rc)),
            _ => None,
        });
    match class {
        Some(class) => PyError::class(class, message),
        None => value_error(message),
    }
}

fn pbkdf2(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let sig = Sig {
        name: "pbkdf2_hmac",
        positional: &["hash_name", "password", "salt", "iterations", "dklen"],
        positional_only: 0,
        required: 4,
        kwonly: &[],
    };
    let slots = unpack(&sig, args)?;
    let get = |i: usize| slots[i].as_ref().expect("required slot");
    let hash_name = str_arg("pbkdf2_hmac", "hash_name", get(0))?;
    let password = extract_bytes_like(get(1))?;
    let salt = extract_bytes_like(get(2))?;
    let iterations = c_long(interp, get(3))?;
    let base = HashState::from_name(&hash_name).ok_or_else(|| unsupported_digest(interp))?;
    if iterations < 1 {
        return Err(value_error("iteration value must be greater than 0."));
    }
    if iterations > i128::from(i32::MAX) {
        return Err(overflow_error("iteration value is too great."));
    }
    let dklen = match &slots[4] {
        Some(v) if !v.is_none() => c_long(interp, v)?,
        _ => base.digest_size() as i128,
    };
    if dklen < 1 {
        return Err(value_error("key length must be greater than 0."));
    }
    if dklen > i128::from(i32::MAX) {
        return Err(overflow_error("key length is too great."));
    }
    if base.is_shake() {
        return Err(value_error("no reason supplied"));
    }
    Ok(Value::bytes(pbkdf2_hmac(
        &base,
        &password,
        &salt,
        iterations as u64,
        dklen as usize,
    )))
}

fn scrypt_kdf(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let sig = Sig {
        name: "scrypt",
        positional: &["password"],
        positional_only: 0,
        required: 1,
        kwonly: &["salt", "n", "r", "p", "maxmem", "dklen"],
    };
    let slots = unpack(&sig, args)?;
    let password = extract_bytes_like(slots[0].as_ref().expect("required slot"))?;
    let salt = match &slots[1] {
        Some(v) if !v.is_none() => Some(extract_bytes_like(v)?),
        Some(v) => return Err(bytes_like_required(v)),
        None => None,
    };
    for (index, param) in [(2, "n"), (3, "r"), (4, "p")] {
        if let Some(v) = &slots[index]
            && !matches!(
                v.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return Err(bad_argument("scrypt", param, "int", v));
        }
    }
    let maxmem = match &slots[5] {
        Some(v) => c_long(interp, v)?,
        None => 0,
    };
    let dklen = match &slots[6] {
        Some(v) => c_long(interp, v)?,
        None => 64,
    };
    let Some(salt) = salt else {
        return Err(type_error("salt is required"));
    };
    let n = scrypt_uint(interp, &slots[2], "n")?;
    let r = scrypt_uint(interp, &slots[3], "r")?;
    let p = scrypt_uint(interp, &slots[4], "p")?;
    if n < 2 || n & (n - 1) != 0 {
        return Err(value_error("n must be a power of 2."));
    }
    if !(0..=i128::from(i32::MAX)).contains(&maxmem) {
        return Err(value_error(format!(
            "maxmem must be positive and smaller than {}",
            i32::MAX
        )));
    }
    if !(1..=i128::from(i32::MAX)).contains(&dklen) {
        return Err(value_error(format!(
            "dklen must be greater than 0 and smaller than {}",
            i32::MAX
        )));
    }
    let (n, r, p) = (n as u64, r as u64, p as u64);
    match scrypt::validate(n, r, p, maxmem as u64) {
        Ok(()) => {}
        Err(scrypt::ScryptParamError::Invalid) => {
            return Err(value_error(
                "Invalid parameter combination for n, r, p, maxmem.",
            ));
        }
        Err(scrypt::ScryptParamError::MemoryLimit) => {
            return Err(value_error(
                "[digital envelope routines] memory limit exceeded",
            ));
        }
    }
    Ok(Value::bytes(scrypt::scrypt(
        &password,
        &salt,
        n,
        r,
        p,
        dklen as usize,
    )))
}

fn bytes_like_required(val: &Value) -> PyError {
    type_error(format!(
        "a bytes-like object is required, not '{}'",
        value_type_name_str(val)
    ))
}

/// `n` / `r` / `p`: a missing, negative or oversized value reports the same
/// "required and must be an unsigned int" text.
fn scrypt_uint(interp: &mut Interpreter, slot: &Option<Value>, param: &str) -> Result<i128> {
    let missing = || type_error(format!("{param} is required and must be an unsigned int"));
    let value = slot.as_ref().ok_or_else(missing)?;
    match index_i128(interp, value)? {
        Some(n) if (0..=i128::from(u64::MAX)).contains(&n) => Ok(n),
        _ => Err(missing()),
    }
}

// ── file_digest ──────────────────────────────────────────────────────────────

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
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
    args: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, args)
}

/// `hasattr(obj, name)`: only `AttributeError` counts as "absent".
fn has_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    match interp.get_attr(obj, name) {
        Ok(_) => Ok(true),
        Err(ref e) if e.class_name_is("AttributeError") => Ok(false),
        Err(e) => Err(e),
    }
}

/// Bind `file_digest(fileobj, digest, /, *, _bufsize=2**18)` with the wording
/// CPython's frame setup uses for a Python function.
fn file_digest_args(args: &[ExpandedCallArg]) -> Result<(Value, Value, Value)> {
    let positional: Vec<&Value> = args
        .iter()
        .filter(|a| a.name.is_none())
        .map(|a| &a.value)
        .collect();
    if positional.len() > 2 {
        return Err(type_error(format!(
            "file_digest() takes 2 positional arguments but {} were given",
            positional.len()
        )));
    }
    let mut bufsize = None;
    for arg in args.iter().filter(|a| a.name.is_some()) {
        match arg.name.as_deref() {
            Some("_bufsize") => bufsize = Some(arg.value.clone()),
            Some(name) => {
                let passed_positional_only: Vec<String> = args
                    .iter()
                    .filter_map(|a| a.name.as_deref())
                    .filter(|n| matches!(*n, "fileobj" | "digest"))
                    .map(str::to_string)
                    .collect();
                if !passed_positional_only.is_empty() {
                    return Err(type_error(format!(
                        "file_digest() got some positional-only arguments passed as keyword arguments: '{}'",
                        passed_positional_only.join(", ")
                    )));
                }
                return Err(type_error(format!(
                    "file_digest() got an unexpected keyword argument '{name}'"
                )));
            }
            None => {}
        }
    }
    match positional.as_slice() {
        [fileobj, digest] => Ok((
            (*fileobj).clone(),
            (*digest).clone(),
            bufsize.unwrap_or_else(|| Value::int(FILE_DIGEST_BUFSIZE)),
        )),
        [_] => Err(type_error(
            "file_digest() missing 1 required positional argument: 'digest'",
        )),
        _ => Err(type_error(
            "file_digest() missing 2 required positional arguments: 'fileobj' and 'digest'",
        )),
    }
}

/// A file opened in binary mode for reading, judged by its `mode` / `closed`
/// attributes (the fallback used where `readinto()` is unavailable).
fn is_binary_readable_file(interp: &mut Interpreter, fileobj: &Value) -> Result<bool> {
    let mode = match interp.get_attr(fileobj, "mode") {
        Ok(mode) => mode,
        Err(ref e) if e.class_name_is("AttributeError") => return Ok(false),
        Err(e) => return Err(e),
    };
    if !is_str_or_str_subclass(&mode) {
        return Ok(false);
    }
    let mode = crate::interpreter::extract_str_value(&mode);
    if !mode.contains('b') || !(mode.contains('r') || mode.contains('+')) {
        return Ok(false);
    }
    if !has_attr(interp, fileobj, "read")? {
        return Ok(false);
    }
    let closed = match interp.get_attr(fileobj, "closed") {
        Ok(closed) => interp.truthy_value(&closed)?,
        Err(ref e) if e.class_name_is("AttributeError") => false,
        Err(e) => return Err(e),
    };
    Ok(!closed)
}

fn file_digest_read_loop(
    interp: &mut Interpreter,
    fileobj: &Value,
    digestobj: &Value,
    bufsize: &Value,
) -> Result<Value> {
    loop {
        let chunk = call_method(interp, fileobj, "read", std::slice::from_ref(bufsize))?;
        if !interp.truthy_value(&chunk)? {
            break;
        }
        call_method(interp, digestobj, "update", &[chunk])?;
    }
    Ok(digestobj.clone())
}

fn file_digest_impl(
    interp: &mut Interpreter,
    fileobj: &Value,
    digest: &Value,
    bufsize: &Value,
) -> Result<Value> {
    let digestobj = if is_str_or_str_subclass(digest) {
        hash_new(interp, digest, None, Vec::new())?
    } else {
        call_pos(interp, digest.clone(), &[])?
    };

    if has_attr(interp, fileobj, "getbuffer")? {
        // io.BytesIO object, use zero-copy buffer.
        let buffer = call_method(interp, fileobj, "getbuffer", &[])?;
        call_method(interp, &digestobj, "update", &[buffer])?;
        return Ok(digestobj);
    }

    // Only binary files implement readinto().
    let readable =
        has_attr(interp, fileobj, "readinto")? && has_attr(interp, fileobj, "readable")? && {
            let flag = call_method(interp, fileobj, "readable", &[])?;
            interp.truthy_value(&flag)?
        };
    if !readable {
        // PyRust's own file objects expose `read()` but not `readinto()`.
        if is_binary_readable_file(interp, fileobj)? {
            return file_digest_read_loop(interp, fileobj, &digestobj, bufsize);
        }
        let shown = render_value_repr(interp, fileobj)?;
        return Err(value_error(format!(
            "'{shown}' is not a file-like object in binary reading mode."
        )));
    }

    // Reusable buffer to reduce allocations.
    let buf = call_pos(
        interp,
        Value::builtin_function("bytearray"),
        std::slice::from_ref(bufsize),
    )?;
    let view = call_pos(
        interp,
        Value::builtin_function("memoryview"),
        std::slice::from_ref(&buf),
    )?;
    loop {
        let size = call_method(interp, fileobj, "readinto", std::slice::from_ref(&buf))?;
        if matches!(size.kind(), ValueKind::Int(0)) {
            break; // EOF
        }
        let upto = call_pos(interp, Value::builtin_function("slice"), &[size])?;
        let operator = interp.load_module("operator")?;
        let getitem = interp.get_attr(&operator, "getitem")?;
        let chunk = call_pos(interp, getitem, &[view.clone(), upto])?;
        call_method(interp, &digestobj, "update", &[chunk])?;
    }
    Ok(digestobj)
}

// ── Native module surface ────────────────────────────────────────────────────

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "algorithms_guaranteed" => name_set(&[&ALGORITHMS_GUARANTEED]),
        "algorithms_available" => name_set(&[&ALGORITHMS_GUARANTEED, &ALGORITHMS_EXTRA]),
        "__all__" => Value::tuple(ALL_NAMES.into_iter().map(Value::string).collect()),
    }

    /// new(name, data=b'') - Return a new hashing object using the named algorithm;
    /// optionally initialized with data (which must be a bytes-like object).
    fn new(args) -> Result<Value> {
        // `hashlib.__hash_new(name, data=b'', **kwargs)` is a Python function.
        let positional: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        if positional.len() > 2 {
            return Err(type_error(format!(
                "__hash_new() takes from 1 to 2 positional arguments but {} were given",
                positional.len()
            )));
        }
        let mut name = positional.first().map(|a| a.value.clone());
        let mut data = positional.get(1).map(|a| a.value.clone());
        let mut kwargs = Vec::new();
        for arg in args.iter().filter(|a| a.name.is_some()) {
            match arg.name.as_deref() {
                Some("name") => {
                    if name.is_some() {
                        return Err(type_error(
                            "__hash_new() got multiple values for argument 'name'",
                        ));
                    }
                    name = Some(arg.value.clone());
                }
                Some("data") => {
                    if data.is_some() {
                        return Err(type_error(
                            "__hash_new() got multiple values for argument 'data'",
                        ));
                    }
                    data = Some(arg.value.clone());
                }
                _ => kwargs.push(arg.clone()),
            }
        }
        let name = name.ok_or_else(|| {
            type_error("__hash_new() missing 1 required positional argument: 'name'")
        })?;
        hash_new(_interp, &name, data, kwargs)
    }

    /// Returns a md5 hash object; optionally initialized with a string
    fn md5(args) -> Result<Value> {
        construct_named(_interp, "openssl_md5", "md5", args)
    }

    /// Returns a sha1 hash object; optionally initialized with a string
    fn sha1(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha1", "sha1", args)
    }

    /// Returns a sha224 hash object; optionally initialized with a string
    fn sha224(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha224", "sha224", args)
    }

    /// Returns a sha256 hash object; optionally initialized with a string
    fn sha256(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha256", "sha256", args)
    }

    /// Returns a sha384 hash object; optionally initialized with a string
    fn sha384(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha384", "sha384", args)
    }

    /// Returns a sha512 hash object; optionally initialized with a string
    fn sha512(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha512", "sha512", args)
    }

    /// Returns a sha3-224 hash object; optionally initialized with a string
    fn sha3_224(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha3_224", "sha3_224", args)
    }

    /// Returns a sha3-256 hash object; optionally initialized with a string
    fn sha3_256(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha3_256", "sha3_256", args)
    }

    /// Returns a sha3-384 hash object; optionally initialized with a string
    fn sha3_384(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha3_384", "sha3_384", args)
    }

    /// Returns a sha3-512 hash object; optionally initialized with a string
    fn sha3_512(args) -> Result<Value> {
        construct_named(_interp, "openssl_sha3_512", "sha3_512", args)
    }

    /// Returns a shake-128 variable hash object; optionally initialized with a string
    fn shake_128(args) -> Result<Value> {
        construct_named(_interp, "openssl_shake_128", "shake_128", args)
    }

    /// Returns a shake-256 variable hash object; optionally initialized with a string
    fn shake_256(args) -> Result<Value> {
        construct_named(_interp, "openssl_shake_256", "shake_256", args)
    }

    /// Password based key derivation function 2 (PKCS #5 v2.0) with HMAC as
    /// pseudorandom function.
    fn pbkdf2_hmac(args) -> Result<Value> {
        pbkdf2(_interp, args)
    }

    /// scrypt password-based key derivation function.
    fn scrypt(args) -> Result<Value> {
        scrypt_kdf(_interp, args)
    }

    /// Hash the contents of a file-like object. Returns a digest object.
    ///
    /// *fileobj* must be a file-like object opened for reading in binary mode.
    /// It accepts file objects from open(), io.BytesIO(), and SocketIO objects.
    /// The function may bypass Python's I/O and use the file descriptor *fileno*
    /// directly.
    ///
    /// *digest* must either be a hash algorithm name as a *str*, a hash
    /// constructor, or a callable that returns a hash object.
    fn file_digest(args) -> Result<Value> {
        let (fileobj, digest, bufsize) = file_digest_args(args)?;
        file_digest_impl(_interp, &fileobj, &digest, &bufsize)
    }

    #[native_state(HashState)]
    class HASH {
        constants {
            "__module__" => Value::string("_hashlib"),
            "__doc__" => Value::string(HASH_DOC),
        }

        fn __new__(args) -> Result<Value> {
            let _ = args;
            Err(type_error("cannot create '_hashlib.HASH' instances"))
        }

        /// Update this hash object's state with the provided string.
        fn update(args) -> Result<Value> {
            let this = receiver(args)?;
            let data = extract_hash_input(one_arg("HASH.update", args)?)?;
            with_state_mut(this, |state| {
                state.update(&data);
                Ok(Value::none())
            })
        }

        /// Return the digest value as a bytes object.
        fn digest(args) -> Result<Value> {
            no_args("HASH.digest", args)?;
            with_state(receiver(args)?, |state| Ok(Value::bytes(state.digest())))
        }

        /// Return the digest value as a string of hexadecimal digits.
        fn hexdigest(args) -> Result<Value> {
            no_args("HASH.hexdigest", args)?;
            with_state(receiver(args)?, |state| {
                Ok(Value::string(hex_encode(&state.digest())))
            })
        }

        /// Return a copy of the hash object.
        fn copy(args) -> Result<Value> {
            no_args("HASH.copy", args)?;
            let cloned = with_state(receiver(args)?, |state| Ok(state.clone()))?;
            wrap_state(cloned)
        }

        fn __reduce_ex__(args) -> Result<Value> {
            reduce_ex(args)
        }

        #[getter]
        fn name(#[state] state: &HashState) -> Result<Value> {
            Ok(Value::string(state.name()))
        }

        #[getter]
        fn digest_size(#[state] state: &HashState) -> Result<Value> {
            Ok(Value::int(state.digest_size() as i64))
        }

        #[getter]
        fn block_size(#[state] state: &HashState) -> Result<Value> {
            Ok(Value::int(state.block_size() as i64))
        }

        fn __repr__(args) -> Result<Value> {
            let this = receiver(args)?;
            let state = with_state(this, |state| Ok(state.clone()))?;
            Ok(hash_repr(_interp, &state, this))
        }
    }

    #[native_state(HashState)]
    class HASHXOF("HASH") {
        constants {
            "__module__" => Value::string("_hashlib"),
            "__doc__" => Value::string(HASHXOF_DOC),
        }

        fn __new__(args) -> Result<Value> {
            let _ = args;
            Err(type_error("cannot create '_hashlib.HASHXOF' instances"))
        }

        /// Return the digest value as a bytes object.
        fn digest(args) -> Result<Value> {
            let state = with_state(receiver(args)?, |state| Ok(state.clone()))?;
            xof_output(_interp, &state, args, false)
        }

        /// Return the digest value as a string of hexadecimal digits.
        fn hexdigest(args) -> Result<Value> {
            let state = with_state(receiver(args)?, |state| Ok(state.clone()))?;
            xof_output(_interp, &state, args, true)
        }

        /// Return a copy of the hash object.
        fn copy(args) -> Result<Value> {
            no_args("HASHXOF.copy", args)?;
            let cloned = with_state(receiver(args)?, |state| Ok(state.clone()))?;
            wrap_state(cloned)
        }

        #[getter]
        fn digest_size(#[state] state: &HashState) -> Result<Value> {
            let _ = state;
            Ok(Value::int(0))
        }
    }

    #[native_state(HashState)]
    class blake2b {
        constants {
            "__module__" => Value::string("_blake2"),
            "__doc__" => Value::string("Return a new BLAKE2b hash object."),
            "MAX_DIGEST_SIZE" => Value::int(64),
            "MAX_KEY_SIZE" => Value::int(64),
            "PERSON_SIZE" => Value::int(16),
            "SALT_SIZE" => Value::int(16),
        }

        fn __new__(args) -> Result<Value> {
            let cls = receiver(args)?;
            let ValueKind::PyClass(class) = cls.kind() else {
                return Err(type_error("blake2b.__new__(X): X is not a type object"));
            };
            let class = Rc::clone(class);
            blake2_construct(_interp, true, &class, &args[1..])
        }

        /// Update this hash object's state with the provided string.
        fn update(args) -> Result<Value> {
            let this = receiver(args)?;
            let data = extract_hash_input(one_arg("blake2b.update", args)?)?;
            with_state_mut(this, |state| {
                state.update(&data);
                Ok(Value::none())
            })
        }

        /// Return the digest value as a bytes object.
        fn digest(args) -> Result<Value> {
            no_args("blake2b.digest", args)?;
            with_state(receiver(args)?, |state| Ok(Value::bytes(state.digest())))
        }

        /// Return the digest value as a string of hexadecimal digits.
        fn hexdigest(args) -> Result<Value> {
            no_args("blake2b.hexdigest", args)?;
            with_state(receiver(args)?, |state| {
                Ok(Value::string(hex_encode(&state.digest())))
            })
        }

        /// Return a copy of the hash object.
        fn copy(args) -> Result<Value> {
            no_args("blake2b.copy", args)?;
            let cloned = with_state(receiver(args)?, |state| Ok(state.clone()))?;
            wrap_state(cloned)
        }

        fn __reduce_ex__(args) -> Result<Value> {
            reduce_ex(args)
        }

        #[getter]
        fn name(#[state] state: &HashState) -> Result<Value> {
            let _ = state;
            Ok(Value::string("blake2b"))
        }

        #[getter]
        fn digest_size(#[state] state: &HashState) -> Result<Value> {
            Ok(Value::int(state.digest_size() as i64))
        }

        #[getter]
        fn block_size(#[state] state: &HashState) -> Result<Value> {
            let _ = state;
            Ok(Value::int(128))
        }
    }

    #[native_state(HashState)]
    class blake2s {
        constants {
            "__module__" => Value::string("_blake2"),
            "__doc__" => Value::string("Return a new BLAKE2s hash object."),
            "MAX_DIGEST_SIZE" => Value::int(32),
            "MAX_KEY_SIZE" => Value::int(32),
            "PERSON_SIZE" => Value::int(8),
            "SALT_SIZE" => Value::int(8),
        }

        fn __new__(args) -> Result<Value> {
            let cls = receiver(args)?;
            let ValueKind::PyClass(class) = cls.kind() else {
                return Err(type_error("blake2s.__new__(X): X is not a type object"));
            };
            let class = Rc::clone(class);
            blake2_construct(_interp, false, &class, &args[1..])
        }

        /// Update this hash object's state with the provided string.
        fn update(args) -> Result<Value> {
            let this = receiver(args)?;
            let data = extract_hash_input(one_arg("blake2s.update", args)?)?;
            with_state_mut(this, |state| {
                state.update(&data);
                Ok(Value::none())
            })
        }

        /// Return the digest value as a bytes object.
        fn digest(args) -> Result<Value> {
            no_args("blake2s.digest", args)?;
            with_state(receiver(args)?, |state| Ok(Value::bytes(state.digest())))
        }

        /// Return the digest value as a string of hexadecimal digits.
        fn hexdigest(args) -> Result<Value> {
            no_args("blake2s.hexdigest", args)?;
            with_state(receiver(args)?, |state| {
                Ok(Value::string(hex_encode(&state.digest())))
            })
        }

        /// Return a copy of the hash object.
        fn copy(args) -> Result<Value> {
            no_args("blake2s.copy", args)?;
            let cloned = with_state(receiver(args)?, |state| Ok(state.clone()))?;
            wrap_state(cloned)
        }

        fn __reduce_ex__(args) -> Result<Value> {
            reduce_ex(args)
        }

        #[getter]
        fn name(#[state] state: &HashState) -> Result<Value> {
            let _ = state;
            Ok(Value::string("blake2s"))
        }

        #[getter]
        fn digest_size(#[state] state: &HashState) -> Result<Value> {
            Ok(Value::int(state.digest_size() as i64))
        }

        #[getter]
        fn block_size(#[state] state: &HashState) -> Result<Value> {
            let _ = state;
            Ok(Value::int(64))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::engine::{HashState, hex_encode, pbkdf2_hmac};

    fn hex_of(name: &str, msg: &[u8]) -> String {
        let mut h = HashState::from_name(name).expect("known hash");
        h.update(msg);
        hex_encode(&h.digest())
    }

    #[test]
    fn rfc_and_nist_vectors() {
        assert_eq!(hex_of("md5", b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex_of("md5", b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex_of("sha1", b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex_of("sha224", b"abc"),
            "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7"
        );
        assert_eq!(
            hex_of("sha256", b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_of("sha384", b"abc"),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            hex_of("sha512", b"abc"),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            hex_of("sha512_224", b"abc"),
            "4634270f707b6a54daae7530460842e20e37ed265ceee9a43e8924aa"
        );
        assert_eq!(
            hex_of("sha512_256", b"abc"),
            "53048e2681941ef99b2e29b76b4c7dabe4c2d0c634fc6d46e0e2f13107e7af23"
        );
        assert_eq!(
            hex_of("sha3_224", b"abc"),
            "e642824c3f8cf24ad09234ee7d3c766fc9a3a5168d0c94ad73b46fdf"
        );
        assert_eq!(
            hex_of("sha3_256", b"abc"),
            "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532"
        );
        assert_eq!(
            hex_of("sha3_384", b"abc"),
            "ec01498288516fc926459f58e2c6ad8df9b473cb0fc08c2596da7cf0e49be4b298d88cea927ac7f539f1edf228376d25"
        );
        assert_eq!(
            hex_of("sha3_512", b"abc"),
            "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0"
        );
        let mut s128 = HashState::from_name("shake_128").unwrap();
        s128.update(b"abc");
        assert_eq!(
            hex_encode(&s128.digest_xof(16)),
            "5881092dd818bf5cf8a3ddb793fbcba7"
        );
        let mut s256 = HashState::from_name("shake_256").unwrap();
        s256.update(b"abc");
        assert_eq!(
            hex_encode(&s256.digest_xof(16)),
            "483366601360a8771c6863080cc4114d"
        );
        assert_eq!(
            hex_of("blake2b", b"abc"),
            "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
        assert_eq!(
            hex_of("blake2s", b"abc"),
            "508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982"
        );
        let sha256 = HashState::from_name("sha256").unwrap();
        assert_eq!(
            hex_encode(&pbkdf2_hmac(&sha256, b"password", b"salt", 1, 32)),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
    }
}
