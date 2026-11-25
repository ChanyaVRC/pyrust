// `zlib` module — body for the `zlib @inject` entry in
// `pyrust_builtin_modules!` (issue #3186).
//
// Pure-Rust RFC 1951 (DEFLATE), RFC 1950 (zlib), and RFC 1952 (gzip)
// compression/decompression plus CRC-32 and Adler-32 live under `bodies/zlib/`.
// The public Python 3.12 surface (`error`, constants, functions, `Compress`,
// `Decompress`) is injected from `zlib_py.py`.
//
// Reference: <https://docs.python.org/3/library/zlib.html>

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
    value_type_name_str,
};
use crate::value::{PyBigInt, PyBigIntSign, PyKey, PyToPrimitive, Value, ValueKind};
use pyrust_core::BuiltinTypeOps;
use pyrust_derive::pyrust_module;

#[path = "zlib/checksum.rs"]
mod checksum;
#[path = "zlib/deflate.rs"]
mod deflate;
#[path = "zlib/inflate.rs"]
mod inflate;
#[path = "zlib/tables.rs"]
mod tables;

use self::checksum::{adler32, crc32};
use self::deflate::Deflater;
use self::inflate::Inflater;

/// Compute IEEE 802.3 CRC-32 checksum over `data`.
pub(crate) fn crc32_bytes(data: &[u8], init: u32) -> u32 {
    crc32(data, init)
}

/// Compress `data` with raw DEFLATE (RFC 1951, `wbits = -15`) for ZIP entries.
pub(crate) fn raw_deflate(data: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let mut def = Deflater::new(-1, 8, -15, 8, 0, Vec::new())?;
    let mut out = def.compress(data)?;
    let tail = def.flush(4)?;
    out.extend_from_slice(&tail);
    Ok(out)
}

/// Decompress raw DEFLATE (RFC 1951, `wbits = -15`) data from ZIP entries.
pub(crate) fn raw_inflate(data: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let mut inf = Inflater::new(-15, Vec::new())?;
    let out = inf.decompress(data, 0)?;
    if !inf.eof {
        return Err("incomplete or truncated deflate stream".to_string());
    }
    Ok(out)
}

/// Python-source definitions for the public `zlib` surface.
const ZLIB_PY_SOURCE: &str = include_str!("zlib_py.py");

/// Private native helpers seeded into the injected exec namespace and removed
/// from `zlib.__dict__` after injection so `dir(zlib)` matches CPython.
const ZLIB_NATIVE_HELPERS: [&str; 12] = [
    "_crc32",
    "_adler32",
    "_compress",
    "_decompress",
    "_compressobj_new",
    "_compressobj_compress",
    "_compressobj_flush",
    "_compressobj_copy",
    "_decompressobj_new",
    "_decompressobj_decompress",
    "_decompressobj_flush",
    "_decompressobj_copy",
];

/// Public names exported from `ZLIB_PY_SOURCE` onto the `zlib` module.
const ZLIB_PY_EXPORTS: [&str; 31] = [
    "DEFLATED",
    "DEF_BUF_SIZE",
    "DEF_MEM_LEVEL",
    "MAX_WBITS",
    "ZLIB_RUNTIME_VERSION",
    "ZLIB_VERSION",
    "Z_BEST_COMPRESSION",
    "Z_BEST_SPEED",
    "Z_BLOCK",
    "Z_DEFAULT_COMPRESSION",
    "Z_DEFAULT_STRATEGY",
    "Z_FILTERED",
    "Z_FINISH",
    "Z_FIXED",
    "Z_FULL_FLUSH",
    "Z_HUFFMAN_ONLY",
    "Z_NO_COMPRESSION",
    "Z_NO_FLUSH",
    "Z_PARTIAL_FLUSH",
    "Z_RLE",
    "Z_SYNC_FLUSH",
    "Z_TREES",
    "adler32",
    "compress",
    "compressobj",
    "crc32",
    "decompress",
    "decompressobj",
    "error",
    "_Compress",
    "_Decompress",
];

/// Execute `ZLIB_PY_SOURCE` once, hide internal helpers, and export the
/// public CPython 3.12 `zlib` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in ZLIB_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(ZLIB_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("zlib: exec namespace not a dict".into()))?;
    for name in ZLIB_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("zlib"));
                if name == "error" {
                    class.error_name = Some("zlib.error");
                }
            }
            if !name.starts_with('_') {
                module
                    .borrow_mut()
                    .attrs
                    .insert(name.to_string(), val.clone());
            }
        }
    }
    Ok(Some(ns.clone()))
}

// ── Opaque native compressor / decompressor state ────────────────────────────

struct CompressorOps;
const COMPRESSOR_OPS: &CompressorOps = &CompressorOps;

impl BuiltinTypeOps for CompressorOps {
    fn type_name(&self) -> &'static str {
        "_zlib_compressor"
    }
}

struct DecompressorOps;
const DECOMPRESSOR_OPS: &DecompressorOps = &DecompressorOps;

impl BuiltinTypeOps for DecompressorOps {
    fn type_name(&self) -> &'static str {
        "_zlib_decompressor"
    }
}

fn wrap_deflater(state: Deflater) -> Value {
    let boxed: Box<dyn Any> = Box::new(state);
    Value::builtin_object(COMPRESSOR_OPS, boxed)
}

fn wrap_inflater(state: Inflater) -> Value {
    let boxed: Box<dyn Any> = Box::new(state);
    Value::builtin_object(DECOMPRESSOR_OPS, boxed)
}

fn with_deflater<R>(val: &Value, action: impl FnOnce(&Deflater) -> R) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid zlib compressor state".into()));
    };
    if !pyrust_core::builtin_ops_is::<CompressorOps>(ops) {
        return Err(PyError::runtime("invalid zlib compressor state".into()));
    }
    let borrow = state.borrow();
    let d = borrow
        .downcast_ref::<Deflater>()
        .ok_or_else(|| PyError::runtime("invalid zlib compressor state".into()))?;
    Ok(action(d))
}

fn with_deflater_mut<R>(val: &Value, action: impl FnOnce(&mut Deflater) -> R) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid zlib compressor state".into()));
    };
    if !pyrust_core::builtin_ops_is::<CompressorOps>(ops) {
        return Err(PyError::runtime("invalid zlib compressor state".into()));
    }
    let mut borrow = state.borrow_mut();
    let d = borrow
        .downcast_mut::<Deflater>()
        .ok_or_else(|| PyError::runtime("invalid zlib compressor state".into()))?;
    Ok(action(d))
}

fn with_inflater<R>(val: &Value, action: impl FnOnce(&Inflater) -> R) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid zlib decompressor state".into()));
    };
    if !pyrust_core::builtin_ops_is::<DecompressorOps>(ops) {
        return Err(PyError::runtime("invalid zlib decompressor state".into()));
    }
    let borrow = state.borrow();
    let d = borrow
        .downcast_ref::<Inflater>()
        .ok_or_else(|| PyError::runtime("invalid zlib decompressor state".into()))?;
    Ok(action(d))
}

fn with_inflater_mut<R>(val: &Value, action: impl FnOnce(&mut Inflater) -> R) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid zlib decompressor state".into()));
    };
    if !pyrust_core::builtin_ops_is::<DecompressorOps>(ops) {
        return Err(PyError::runtime("invalid zlib decompressor state".into()));
    }
    let mut borrow = state.borrow_mut();
    let d = borrow
        .downcast_mut::<Inflater>()
        .ok_or_else(|| PyError::runtime("invalid zlib decompressor state".into()))?;
    Ok(action(d))
}

// ── Error, buffer & integer helpers ──────────────────────────────────────────

fn raise_zlib_error(error_cls: &Value, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    if let ValueKind::PyClass(cls_rc) = error_cls.kind() {
        return PyError::raised(crate::interpreter::instantiate_exception(
            Rc::clone(cls_rc),
            vec![Value::string(&msg)],
        ));
    }
    PyError::runtime(msg)
}

fn try_extract_bytes_like(val: &Value) -> Option<Vec<u8>> {
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

fn extract_bytes_like(val: &Value) -> Result<Vec<u8>> {
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

fn bigint_low_u32(bi: &PyBigInt) -> u32 {
    let (sign, digits) = bi.to_u64_digits();
    let low = digits.first().copied().unwrap_or(0) as u32;
    if sign == PyBigIntSign::Minus {
        low.wrapping_neg()
    } else {
        low
    }
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

fn extract_u32_mask(interp: &mut Interpreter, val: &Value) -> Result<u32> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Int(n) => Ok(n as u32),
        ValueKind::Bool(b) => Ok(u32::from(b)),
        ValueKind::BigInt(bi) => Ok(bigint_low_u32(bi)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_c_int(interp: &mut Interpreter, val: &Value) -> Result<i32> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Bool(b) => Ok(i32::from(b)),
        ValueKind::Int(n) => i32::try_from(n).map_err(|_| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        }),
        ValueKind::BigInt(bi) => bi.to_i32().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
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

fn extract_nonneg_usize(interp: &mut Interpreter, val: &Value, err_msg: &str) -> Result<usize> {
    let idx = resolve_index_value(interp, val)?;
    let n: i64 = match idx.kind() {
        ValueKind::Bool(b) => i64::from(b),
        ValueKind::Int(n) => n,
        ValueKind::BigInt(bi) => bi.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t".to_string(),
            )
        })?,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    value_type_name_str(val)
                ),
            ));
        }
    };
    if n < 0 {
        return Err(PyError::named("ValueError", err_msg.to_string()));
    }
    Ok(n as usize)
}

fn inflater_result_tuple(out: Vec<u8>, inf: &Inflater) -> Value {
    Value::tuple(vec![
        Value::bytes(out),
        Value::bool_(inf.eof),
        Value::bytes(inf.unused_data.clone()),
        Value::bytes(inf.unconsumed_tail.clone()),
    ])
}

pyrust_module! {
    fn _crc32(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::runtime("_crc32 expects 2 args".into()));
        }
        let data = extract_bytes_like(&args[0].value)?;
        let init = extract_u32_mask(_interp, &args[1].value)?;
        Ok(Value::int(i64::from(crc32(&data, init))))
    }

    fn _adler32(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::runtime("_adler32 expects 2 args".into()));
        }
        let data = extract_bytes_like(&args[0].value)?;
        let init = extract_u32_mask(_interp, &args[1].value)?;
        Ok(Value::int(i64::from(adler32(&data, init))))
    }

    fn _compress(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 4 {
            return Err(PyError::runtime("_compress expects 4 args".into()));
        }
        let error_cls = &args[0].value;
        let data = extract_bytes_like(&args[1].value)?;
        let level = extract_c_int(_interp, &args[2].value)?;
        let wbits = extract_c_int(_interp, &args[3].value)?;
        let mut def = Deflater::new(level, 8, wbits, 8, 0, Vec::new())
            .map_err(|_| raise_zlib_error(error_cls, "Bad compression level"))?;
        let mut out = def
            .compress(&data)
            .map_err(|e| raise_zlib_error(error_cls, e))?;
        let tail = def.flush(4).map_err(|e| raise_zlib_error(error_cls, e))?;
        out.extend_from_slice(&tail);
        Ok(Value::bytes(out))
    }

    fn _decompress(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 4 {
            return Err(PyError::runtime("_decompress expects 4 args".into()));
        }
        let error_cls = &args[0].value;
        let data = extract_bytes_like(&args[1].value)?;
        let wbits = extract_c_int(_interp, &args[2].value)?;
        let _bufsize = extract_nonneg_usize(_interp, &args[3].value, "bufsize must be non-negative")?;
        let mut inf = Inflater::new(wbits, Vec::new()).map_err(|_| {
            raise_zlib_error(
                error_cls,
                "Error -2 while preparing to decompress data: inconsistent stream state",
            )
        })?;
        let out = inf
            .decompress(&data, 0)
            .map_err(|e| raise_zlib_error(error_cls, e))?;
        if !inf.eof {
            return Err(raise_zlib_error(
                error_cls,
                "Error -5 while decompressing data: incomplete or truncated stream",
            ));
        }
        Ok(Value::bytes(out))
    }

    fn _compressobj_new(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 6 {
            return Err(PyError::runtime("_compressobj_new expects 6 args".into()));
        }
        let level = extract_c_int(_interp, &args[0].value)?;
        let method = extract_c_int(_interp, &args[1].value)?;
        let wbits = extract_c_int(_interp, &args[2].value)?;
        let mem_level = extract_c_int(_interp, &args[3].value)?;
        let strategy = extract_c_int(_interp, &args[4].value)?;
        let zdict = if matches!(args[5].value.kind(), ValueKind::None) {
            Vec::new()
        } else {
            extract_bytes_like(&args[5].value)?
        };
        let def = Deflater::new(level, method, wbits, mem_level, strategy, zdict)
            .map_err(|_| PyError::named("ValueError", "Invalid initialization option".to_string()))?;
        Ok(wrap_deflater(def))
    }

    fn _compressobj_compress(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::runtime("_compressobj_compress expects 3 args".into()));
        }
        let error_cls = &args[0].value;
        let data = extract_bytes_like(&args[2].value)?;
        let res = with_deflater_mut(&args[1].value, |d| d.compress(&data))?;
        let out = res.map_err(|e| raise_zlib_error(error_cls, e))?;
        Ok(Value::bytes(out))
    }

    fn _compressobj_flush(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::runtime("_compressobj_flush expects 3 args".into()));
        }
        let error_cls = &args[0].value;
        let mode = extract_c_int(_interp, &args[2].value)?;
        let res = with_deflater_mut(&args[1].value, |d| d.flush(mode))?;
        let out = res.map_err(|e| raise_zlib_error(error_cls, e))?;
        Ok(Value::bytes(out))
    }

    fn _compressobj_copy(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_compressobj_copy expects 1 arg".into()));
        }
        let cloned = with_deflater(&args[0].value, |d| {
            if d.is_finished() {
                None
            } else {
                Some(d.clone())
            }
        })?;
        match cloned {
            Some(c) => Ok(wrap_deflater(c)),
            None => Err(PyError::named(
                "ValueError",
                "Inconsistent stream state".to_string(),
            )),
        }
    }

    fn _decompressobj_new(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::runtime("_decompressobj_new expects 2 args".into()));
        }
        let wbits = extract_c_int(_interp, &args[0].value)?;
        let zdict = extract_bytes_like(&args[1].value)?;
        let inf = Inflater::new(wbits, zdict)
            .map_err(|_| PyError::named("ValueError", "Invalid initialization option".to_string()))?;
        Ok(wrap_inflater(inf))
    }

    fn _decompressobj_decompress(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 4 {
            return Err(PyError::runtime("_decompressobj_decompress expects 4 args".into()));
        }
        let error_cls = &args[0].value;
        let data = extract_bytes_like(&args[2].value)?;
        let max_length =
            extract_nonneg_usize(_interp, &args[3].value, "max_length must be non-negative")?;
        let res = with_inflater_mut(&args[1].value, |inf| {
            let out = inf.decompress(&data, max_length)?;
            Ok::<Value, String>(inflater_result_tuple(out, inf))
        })?;
        res.map_err(|e| raise_zlib_error(error_cls, e))
    }

    fn _decompressobj_flush(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::runtime("_decompressobj_flush expects 3 args".into()));
        }
        let error_cls = &args[0].value;
        let length =
            extract_nonneg_usize(_interp, &args[2].value, "length must be greater than zero")?;
        if length == 0 {
            return Err(PyError::named(
                "ValueError",
                "length must be greater than zero".to_string(),
            ));
        }
        let res = with_inflater_mut(&args[1].value, |inf| {
            let out = inf.flush()?;
            Ok::<Value, String>(inflater_result_tuple(out, inf))
        })?;
        res.map_err(|e| raise_zlib_error(error_cls, e))
    }

    fn _decompressobj_copy(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_decompressobj_copy expects 1 arg".into()));
        }
        let (cloned, eof, unused, tail) = with_inflater(&args[0].value, |inf| {
            (
                inf.clone(),
                inf.eof,
                inf.unused_data.clone(),
                inf.unconsumed_tail.clone(),
            )
        })?;
        Ok(Value::tuple(vec![
            wrap_inflater(cloned),
            Value::bool_(eof),
            Value::bytes(unused),
            Value::bytes(tail),
        ]))
    }
}
