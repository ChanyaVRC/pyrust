// `hashlib` module — body for the `hashlib @inject` entry in
// `pyrust_builtin_modules!` (issue #3146).
//
// Pure-Rust implementations of MD5, SHA-1, SHA-224/256/384/512, SHA-512/224,
// SHA-512/256, SHA3-224/256/384/512, SHAKE128/256, BLAKE2b, BLAKE2s, HMAC, and
// PBKDF2-HMAC live under `bodies/hashlib/`.  The public Python 3.12 surface
// (constructors, `new`, `pbkdf2_hmac`, `file_digest`, `algorithms_guaranteed`,
// `algorithms_available`, and hash wrapper classes) is injected from
// `hashlib_py.py`.
//
// Reference: <https://docs.python.org/3/library/hashlib.html>

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
    value_type_name_str,
};
use crate::value::{PyKey, PyToPrimitive, Value, ValueKind};
use pyrust_core::BuiltinTypeOps;
use pyrust_derive::pyrust_module;

#[path = "hashlib/blake2.rs"]
mod blake2;
#[path = "hashlib/engine.rs"]
mod engine;
#[path = "hashlib/md5.rs"]
mod md5;
#[path = "hashlib/sha1.rs"]
mod sha1;
#[path = "hashlib/sha2.rs"]
mod sha2;
#[path = "hashlib/sha3.rs"]
mod sha3;

use self::blake2::Blake2Params;
use self::engine::{HashState, hex_encode, hmac_oneshot, pbkdf2_hmac};

/// Python-source definitions for the public `hashlib` surface.
const HASHLIB_PY_SOURCE: &str = include_str!("hashlib_py.py");

/// Private native helpers seeded into the injected exec namespace and removed
/// from `hashlib.__dict__` after injection so `dir(hashlib)` matches CPython.
const HASHLIB_NATIVE_HELPERS: [&str; 9] = [
    "_new_hash",
    "_new_blake2",
    "_hash_update",
    "_hash_digest",
    "_hash_hexdigest",
    "_hash_digest_xof",
    "_hash_hexdigest_xof",
    "_hash_copy",
    "_pbkdf2_hmac",
];

/// Public names exported from `HASHLIB_PY_SOURCE` onto the `hashlib` module.
const HASHLIB_PY_EXPORTS: [&str; 20] = [
    "__all__",
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

/// Execute `HASHLIB_PY_SOURCE` once, hide internal helpers, and export the
/// public CPython 3.12 `hashlib` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in HASHLIB_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(HASHLIB_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("hashlib: exec namespace not a dict".into()))?;
    for name in HASHLIB_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("hashlib"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Shared helpers for `hmac` fast path ──────────────────────────────────────

/// Compute a one-shot HMAC in native Rust for any built-in non-XOF hash name.
/// Returns `Ok(Some(digest))` if recognized, `Ok(None)` if unknown, or `Err`
/// if the algorithm is an XOF (`shake_128`/`shake_256`) that HMAC rejects.
pub(crate) fn hmac_oneshot_by_name(name: &str, key: &[u8], msg: &[u8]) -> Result<Option<Vec<u8>>> {
    let Some(state) = HashState::from_name(name) else {
        return Ok(None);
    };
    if state.is_shake() {
        return Err(PyError::named(
            "ValueError",
            format!("unsupported hash type {name}"),
        ));
    }
    Ok(Some(hmac_oneshot(&state, key, msg)))
}

// ── Opaque native hash state ─────────────────────────────────────────────────

struct HashStateOps;
const HASH_STATE_OPS: &HashStateOps = &HashStateOps;

impl BuiltinTypeOps for HashStateOps {
    fn type_name(&self) -> &'static str {
        "_hashlib_state"
    }
}

fn wrap_state(state: HashState) -> Value {
    let boxed: Box<dyn Any> = Box::new(state);
    Value::builtin_object(HASH_STATE_OPS, boxed)
}

fn with_hash_state<R>(val: &Value, action: impl FnOnce(&HashState) -> R) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid hashlib internal state".into()));
    };
    if !pyrust_core::builtin_ops_is::<HashStateOps>(ops) {
        return Err(PyError::runtime("invalid hashlib internal state".into()));
    }
    let borrow = state.borrow();
    let hs = borrow
        .downcast_ref::<HashState>()
        .ok_or_else(|| PyError::runtime("invalid hashlib internal state".into()))?;
    Ok(action(hs))
}

fn with_hash_state_mut<R>(val: &Value, action: impl FnOnce(&mut HashState) -> R) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid hashlib internal state".into()));
    };
    if !pyrust_core::builtin_ops_is::<HashStateOps>(ops) {
        return Err(PyError::runtime("invalid hashlib internal state".into()));
    }
    let mut borrow = state.borrow_mut();
    let hs = borrow
        .downcast_mut::<HashState>()
        .ok_or_else(|| PyError::runtime("invalid hashlib internal state".into()))?;
    Ok(action(hs))
}

// ── Buffer & integer extraction helpers ──────────────────────────────────────

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
    if let Some(backing) = builtin_data_backing(val) {
        if let ValueKind::Bytes(rc) = backing.kind() {
            return Some(rc.as_ref().clone());
        }
        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
            return Some(snap);
        }
    }
    if let ValueKind::PyInstance(inst_rc) = val.kind() {
        let inst = inst_rc.borrow();
        let is_array = {
            let cls = inst.class.borrow();
            cls.name == "array"
                && cls
                    .attrs
                    .get("__module__")
                    .and_then(|v| v.as_str())
                    .is_some_and(|m| m == "array")
        };
        if is_array
            && let Some(buf) = inst.attrs.get("_buf")
            && let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(buf)
        {
            return Some(snap);
        }
    }
    None
}

pub(crate) fn extract_bytes_like(val: &Value) -> Result<Vec<u8>> {
    try_extract_bytes_like(val).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(val)
            ),
        )
    })
}

fn extract_hash_input(val: &Value) -> Result<Vec<u8>> {
    if is_str_value(val) {
        return Err(PyError::named(
            "TypeError",
            "Strings must be encoded before hashing".to_string(),
        ));
    }
    try_extract_bytes_like(val).ok_or_else(|| {
        PyError::named(
            "TypeError",
            "object supporting the buffer API required".to_string(),
        )
    })
}

fn resolve_index_value(interp: &mut Interpreter, val: &Value) -> Result<Value> {
    match val.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => Ok(val.clone()),
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
            if let Some(indexed) = interp.try_value_to_index(val)? {
                Ok(indexed)
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(val)
                    ),
                ))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_i128(interp: &mut Interpreter, val: &Value) -> Result<i128> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Bool(b) => Ok(i128::from(b)),
        ValueKind::Int(n) => Ok(i128::from(n)),
        ValueKind::BigInt(bi) => bi.to_i128().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C long".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_str_arg(val: &Value, err_msg: impl FnOnce(&str) -> String) -> Result<String> {
    if let ValueKind::Str(s) = val.kind() {
        return Ok(s.to_string());
    }
    if let Some(backing) = builtin_data_backing(val)
        && let ValueKind::Str(s) = backing.kind()
    {
        return Ok(s.to_string());
    }
    Err(PyError::named(
        "TypeError",
        err_msg(&value_type_name_str(val)),
    ))
}

// ── Native module helpers ────────────────────────────────────────────────────

pyrust_module! {
    /// `_new_hash(name, data)` -> `(state_obj, canonical_name, digest_size, block_size, is_shake)`.
    fn _new_hash(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let name = extract_str_arg(&args[0].value, |_| "name must be a string".to_string())?;
        let mut state = HashState::from_name(&name).ok_or_else(|| {
            PyError::named("ValueError", format!("unsupported hash type {name}"))
        })?;
        let data = extract_hash_input(&args[1].value)?;
        if !data.is_empty() {
            state.update(&data);
        }
        let canonical = state.name();
        let digest_size = state.digest_size() as i64;
        let block_size = state.block_size() as i64;
        let is_shake = state.is_shake();
        Ok(Value::tuple(vec![
            wrap_state(state),
            Value::string(canonical),
            Value::int(digest_size),
            Value::int(block_size),
            Value::bool_(is_shake),
        ]))
    }

    /// `_new_blake2(is_b, data, digest_size, key, salt, person, fanout, depth,
    ///              leaf_size, node_offset, node_depth, inner_size, last_node)` -> `state_obj`.
    fn _new_blake2(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let is_b = _interp.truthy_value(&args[0].value)?;
        let max_digest: i128 = if is_b { 64 } else { 32 };
        let max_key: usize = if is_b { 64 } else { 32 };
        let max_salt: usize = if is_b { 16 } else { 8 };
        let max_person: usize = if is_b { 16 } else { 8 };

        let digest_size = extract_i128(_interp, &args[2].value)?;
        if digest_size < 1 || digest_size > max_digest {
            return Err(PyError::named(
                "ValueError",
                format!("digest_size must be between 1 and {max_digest} bytes"),
            ));
        }

        let key = extract_bytes_like(&args[3].value)?;
        if key.len() > max_key {
            return Err(PyError::named(
                "ValueError",
                format!("maximum key length is {max_key} bytes"),
            ));
        }

        let salt = extract_bytes_like(&args[4].value)?;
        if salt.len() > max_salt {
            return Err(PyError::named(
                "ValueError",
                format!("maximum salt length is {max_salt} bytes"),
            ));
        }

        let person = extract_bytes_like(&args[5].value)?;
        if person.len() > max_person {
            return Err(PyError::named(
                "ValueError",
                format!("maximum person length is {max_person} bytes"),
            ));
        }

        let fanout = extract_i128(_interp, &args[6].value)?;
        if !(0..=255).contains(&fanout) {
            return Err(PyError::named(
                "ValueError",
                "fanout must be between 0 and 255".to_string(),
            ));
        }

        let depth = extract_i128(_interp, &args[7].value)?;
        if !(1..=255).contains(&depth) {
            return Err(PyError::named(
                "ValueError",
                "depth must be between 1 and 255".to_string(),
            ));
        }

        let leaf_size = extract_i128(_interp, &args[8].value)?;
        if leaf_size < 0 {
            return Err(PyError::named(
                "ValueError",
                "value must be positive".to_string(),
            ));
        }
        let leaf_size = u32::try_from(leaf_size).map_err(|_| {
            PyError::named("OverflowError", "leaf_size is too large".to_string())
        })?;

        let node_offset = extract_i128(_interp, &args[9].value)?;
        if node_offset < 0 {
            return Err(PyError::named(
                "ValueError",
                "value must be positive".to_string(),
            ));
        }
        let max_node_offset: i128 = if is_b {
            i128::from(u64::MAX)
        } else {
            (1i128 << 48) - 1
        };
        if node_offset > max_node_offset {
            return Err(PyError::named(
                "OverflowError",
                "node_offset is too large".to_string(),
            ));
        }
        let node_offset = node_offset as u64;

        let node_depth = extract_i128(_interp, &args[10].value)?;
        if !(0..=255).contains(&node_depth) {
            return Err(PyError::named(
                "ValueError",
                "node_depth must be between 0 and 255".to_string(),
            ));
        }

        let inner_size = extract_i128(_interp, &args[11].value)?;
        if inner_size < 0 || inner_size > max_digest {
            return Err(PyError::named(
                "ValueError",
                format!("inner_size must be between 0 and is {max_digest}"),
            ));
        }

        let last_node = _interp.truthy_value(&args[12].value)?;
        let data = extract_hash_input(&args[1].value)?;

        let params = Blake2Params {
            digest_size: digest_size as u8,
            key: &key,
            salt: &salt,
            person: &person,
            fanout: fanout as u8,
            depth: depth as u8,
            leaf_size,
            node_offset,
            node_depth: node_depth as u8,
            inner_size: inner_size as u8,
            last_node,
        };
        let mut state = if is_b {
            HashState::new_blake2b(params)
        } else {
            HashState::new_blake2s(params)
        };
        if !data.is_empty() {
            state.update(&data);
        }
        Ok(wrap_state(state))
    }

    /// `_hash_update(state_obj, data)` -> `None`.
    fn _hash_update(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let data = extract_hash_input(&args[1].value)?;
        with_hash_state_mut(&args[0].value, |hs| hs.update(&data))?;
        Ok(Value::none())
    }

    /// `_hash_digest(state_obj)` -> `bytes`.
    fn _hash_digest(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let digest = with_hash_state(&args[0].value, HashState::digest)?;
        Ok(Value::bytes(digest))
    }

    /// `_hash_hexdigest(state_obj)` -> `str`.
    fn _hash_hexdigest(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let hex = with_hash_state(&args[0].value, |hs| hex_encode(&hs.digest()))?;
        Ok(Value::string(hex))
    }

    /// `_hash_digest_xof(state_obj, length)` -> `bytes`.
    fn _hash_digest_xof(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let len = extract_i128(_interp, &args[1].value)?;
        if len < 0 {
            return Err(PyError::named(
                "ValueError",
                "length must be non-negative".to_string(),
            ));
        }
        let len = usize::try_from(len).map_err(|_| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t".to_string(),
            )
        })?;
        let digest = with_hash_state(&args[0].value, |hs| hs.digest_xof(len))?;
        Ok(Value::bytes(digest))
    }

    /// `_hash_hexdigest_xof(state_obj, length)` -> `str`.
    fn _hash_hexdigest_xof(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let len = extract_i128(_interp, &args[1].value)?;
        if len < 0 {
            return Err(PyError::named(
                "ValueError",
                "length must be non-negative".to_string(),
            ));
        }
        let len = usize::try_from(len).map_err(|_| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t".to_string(),
            )
        })?;
        let hex = with_hash_state(&args[0].value, |hs| hex_encode(&hs.digest_xof(len)))?;
        Ok(Value::string(hex))
    }

    /// `_hash_copy(state_obj)` -> `state_obj`.
    fn _hash_copy(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let cloned = with_hash_state(&args[0].value, Clone::clone)?;
        Ok(wrap_state(cloned))
    }

    /// `_pbkdf2_hmac(hash_name, password, salt, iterations, dklen)` -> `bytes`.
    fn _pbkdf2_hmac(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let hash_name = extract_str_arg(&args[0].value, |ty| {
            format!("pbkdf2_hmac() argument 'hash_name' must be str, not {ty}")
        })?;
        let password = extract_bytes_like(&args[1].value)?;
        let salt = extract_bytes_like(&args[2].value)?;
        let iterations = extract_i128(_interp, &args[3].value)?;
        if iterations <= 0 {
            return Err(PyError::named(
                "ValueError",
                "iteration value must be greater than 0.".to_string(),
            ));
        }
        let iterations = u64::try_from(iterations).map_err(|_| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C long".to_string(),
            )
        })?;
        let base = HashState::from_name(&hash_name)
            .filter(|s| !s.is_shake())
            .ok_or_else(|| {
                PyError::named("ValueError", format!("unsupported hash type {hash_name}"))
            })?;
        let dklen: usize = if args[4].value.is_none() {
            base.digest_size()
        } else {
            let v = extract_i128(_interp, &args[4].value)?;
            if v <= 0 {
                return Err(PyError::named(
                    "ValueError",
                    "key length must be greater than 0.".to_string(),
                ));
            }
            usize::try_from(v).map_err(|_| {
                PyError::named(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t".to_string(),
                )
            })?
        };
        Ok(Value::bytes(pbkdf2_hmac(
            &base, &password, &salt, iterations, dklen,
        )))
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
