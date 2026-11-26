// `zlib` — 100% native Rust implementation of CPython 3.12's `zlib` extension
// module (issues #3186, #3324).
//
// Pure-Rust RFC 1951 (DEFLATE), RFC 1950 (zlib), and RFC 1952 (gzip)
// compression/decompression plus CRC-32 and Adler-32 live under `bodies/zlib/`.
// The public Python 3.12 surface (`error`, constants, the checksum and
// one-shot functions, and the `Compress` / `Decompress` stream objects) is
// declared directly in the `pyrust_module!` block below; there is no
// Python-source injection.
//
// Reference: <https://docs.python.org/3/library/zlib.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBytesLike, PyIndex, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{
    InstanceAttrs, PyBigInt, PyBigIntSign, PyClass, PyInstance, PyModule, Value, ValueKind,
};
use pyrust_derive::pyrust_module;

#[path = "zlib/checksum.rs"]
mod checksum;
#[path = "zlib/deflate.rs"]
mod deflate;
#[path = "zlib/inflate.rs"]
mod inflate;
#[path = "zlib/tables.rs"]
mod tables;

use self::checksum::{adler32 as adler32_sum, crc32 as crc32_sum};
use self::deflate::Deflater;
use self::inflate::Inflater;

/// Compute IEEE 802.3 CRC-32 checksum over `data`.
pub(crate) fn crc32_bytes(data: &[u8], init: u32) -> u32 {
    crc32_sum(data, init)
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

const MODULE_DOC: &str =
    "The functions in this module allow compression and decompression using the
zlib library, which is based on GNU zip.

adler32(string[, start]) -- Compute an Adler-32 checksum.
compress(data[, level]) -- Compress data, with compression level 0-9 or -1.
compressobj([level[, ...]]) -- Return a compressor object.
crc32(string[, start]) -- Compute a CRC-32 checksum.
decompress(string,[wbits],[bufsize]) -- Decompresses a compressed string.
decompressobj([wbits[, zdict]]) -- Return a decompressor object.

'wbits' is window buffer size and container format.
Compressor objects support compress() and flush() methods; decompressor
objects support decompress() and flush().";

const MAX_WBITS: i32 = 15;
const Z_DEFAULT_COMPRESSION: i32 = -1;
const Z_FINISH: i32 = 4;
const DEFLATED: i32 = 8;
const DEF_MEM_LEVEL: i32 = 8;
const DEF_BUF_SIZE: i64 = 16384;

// ── Per-interpreter class registry ───────────────────────────────────────────

#[derive(Default)]
struct ZlibRegistry {
    /// `zlib.error`.  Every generation of the module shares one class so
    /// exceptions raised by an earlier import still match a later one.
    error: Option<Rc<RefCell<PyClass>>>,
    /// `zlib.Compress` — the type returned by `compressobj()`; CPython does
    /// not export it from the module.
    compress: Option<Rc<RefCell<PyClass>>>,
    /// `zlib.Decompress` — the type returned by `decompressobj()`; likewise
    /// not exported.
    decompress: Option<Rc<RefCell<PyClass>>>,
}

// `gil_local!` justification:
// (a) Why: the registry stores `!Send` `Rc` class handles shared by every
//     Python thread of one interpreter under the GIL.
// (b) Isolation: independent interpreters on separate OS threads each get
//     their own registry, so classes never leak between them.
pyrust_core::gil_local! {
    static REGISTRY: RefCell<ZlibRegistry> = RefCell::new(ZlibRegistry::default());
}

fn registered(
    pick: impl Fn(&ZlibRegistry) -> Option<Rc<RefCell<PyClass>>>,
) -> Option<Rc<RefCell<PyClass>>> {
    REGISTRY.with(|registry| pick(&registry.borrow()))
}

/// Resolve a registry class, importing `zlib` first if it is not loaded yet.
fn registry_class(
    interp: &mut Interpreter,
    what: &str,
    pick: impl Fn(&ZlibRegistry) -> Option<Rc<RefCell<PyClass>>>,
) -> Result<Rc<RefCell<PyClass>>> {
    if let Some(cls) = registered(&pick) {
        return Ok(cls);
    }
    interp.load_module("zlib")?;
    registered(&pick)
        .ok_or_else(|| PyError::runtime(format!("zlib: {what} class is not initialised")))
}

/// Publish the freshly built module's classes.
///
/// The first generation seen by this interpreter wins: later generations of
/// the module are re-pointed at its `error`.  `Compress` / `Decompress` are
/// removed from the module namespace (CPython does not export them) and kept
/// only in the registry.
fn adopt_module_classes(module: &Rc<RefCell<PyModule>>) {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let mut module = module.borrow_mut();
        match registry.error.clone() {
            Some(shared) => {
                module.insert_attr("error".to_string(), Value::py_class(shared));
            }
            None => {
                if let Some(ValueKind::PyClass(cls)) = module.attrs.get("error").map(Value::kind) {
                    registry.error = Some(Rc::clone(cls));
                }
            }
        }
        for name in ["Compress", "Decompress"] {
            let Some(hidden) = module.attrs.shift_remove(name) else {
                continue;
            };
            let ValueKind::PyClass(cls) = hidden.kind() else {
                continue;
            };
            let slot = if name == "Compress" {
                &mut registry.compress
            } else {
                &mut registry.decompress
            };
            if slot.is_none() {
                *slot = Some(Rc::clone(cls));
            }
        }
    });
}

/// Post-import wiring: share `error` across module generations, hide the
/// stream classes, and give the classes CPython's `__doc__ = None`.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    for name in ["error", "Compress", "Decompress"] {
        let class = module.borrow().attrs.get(name).cloned();
        if let Some(class) = class
            && let ValueKind::PyClass(class) = class.kind()
        {
            let mut class = class.borrow_mut();
            class
                .attrs
                .entry("__doc__".to_string())
                .or_insert_with(Value::none);
            if name == "error" {
                class.error_name = Some("zlib.error");
            }
        }
    }
    adopt_module_classes(module);
}

// ── Error, buffer & integer helpers ──────────────────────────────────────────

/// Instantiate `zlib.error(msg)` from the shared exception class.
fn raise_zlib_error(interp: &mut Interpreter, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    match registry_class(interp, "error", |r| r.error.clone()) {
        Ok(cls) => PyError::raised(crate::interpreter::instantiate_exception(
            cls,
            vec![Value::string(&msg)],
        )),
        Err(_) => PyError::runtime(msg),
    }
}

fn type_error(msg: String) -> PyError {
    PyError::named("TypeError", msg)
}

fn value_error(msg: &str) -> PyError {
    PyError::named("ValueError", msg.to_string())
}

/// Resolve a `Py_buffer` argument (`bytes`, `bytearray`, `memoryview`,
/// `array`, PEP 688 exporters, and subclasses), borrowing `bytes` zero-copy.
fn extract_bytes_like<'a>(
    interp: &mut Interpreter,
    val: &'a Value,
    arg_name: &str,
) -> Result<PyBytesLike<'a>> {
    if !PyBytesLike::matches(val) {
        return Err(type_error(format!(
            "a bytes-like object is required, not '{}'",
            value_type_name_str(val)
        )));
    }
    PyBytesLike::try_from_value_with_interp(interp, val, "zlib", arg_name)
}

fn resolve_index(interp: &mut Interpreter, val: &Value, arg_name: &str) -> Result<PyIndex> {
    PyIndex::try_from_value_with_interp(interp, val, "zlib", arg_name)
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

/// `unsigned int` converter with `bitwise=True`: any integer, wrapped to 32 bits.
fn index_to_u32_mask(idx: &PyIndex) -> u32 {
    match idx.as_i64() {
        Some(n) => n as u32,
        None => bigint_low_u32(&idx.to_bigint()),
    }
}

/// `int` (C `int`) converter: `OverflowError` outside the 32-bit signed range.
fn index_to_c_int(idx: &PyIndex) -> Result<i32> {
    idx.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        })
}

/// `Py_ssize_t` converter: `OverflowError` outside the signed 64-bit range.
fn index_to_ssize(idx: &PyIndex) -> Result<i64> {
    idx.as_i64().ok_or_else(|| {
        PyError::named(
            "OverflowError",
            "Python int too large to convert to C ssize_t".to_string(),
        )
    })
}

fn opt_c_int(
    interp: &mut Interpreter,
    arg: &OptValueRef<'_>,
    default: i32,
    arg_name: &str,
) -> Result<i32> {
    match arg {
        OptionalArg::Provided(v) => index_to_c_int(&resolve_index(interp, v.0, arg_name)?),
        OptionalArg::Omitted => Ok(default),
    }
}

fn opt_ssize(
    interp: &mut Interpreter,
    arg: &OptValueRef<'_>,
    default: i64,
    arg_name: &str,
) -> Result<i64> {
    match arg {
        OptionalArg::Provided(v) => index_to_ssize(&resolve_index(interp, v.0, arg_name)?),
        OptionalArg::Omitted => Ok(default),
    }
}

/// Shared body of `crc32` / `adler32`: parse `(data, value)` in CPython's order.
fn checksum_args<'a>(
    interp: &mut Interpreter,
    data: &'a Value,
    value: &OptValueRef<'_>,
    default: u32,
) -> Result<(PyBytesLike<'a>, u32)> {
    let data = extract_bytes_like(interp, data, "data")?;
    let init = match value {
        OptionalArg::Provided(v) => index_to_u32_mask(&resolve_index(interp, v.0, "value")?),
        OptionalArg::Omitted => default,
    };
    Ok((data, init))
}

// ── Stream object construction ───────────────────────────────────────────────

/// Allocate a hidden-class instance carrying `state` in its native slot.
fn new_native_instance<T: 'static>(class: Rc<RefCell<PyClass>>, state: T) -> Result<Value> {
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::default(),
    })));
    set_native_state(&inst, state)?;
    Ok(inst)
}

fn new_compressor(interp: &mut Interpreter, state: Deflater) -> Result<Value> {
    let class = registry_class(interp, "Compress", |r| r.compress.clone())?;
    new_native_instance(class, state)
}

fn new_decompressor(interp: &mut Interpreter, state: Inflater) -> Result<Value> {
    let class = registry_class(interp, "Decompress", |r| r.decompress.clone())?;
    new_native_instance(class, state)
}

fn copy_compressor(interp: &mut Interpreter, state: &Deflater) -> Result<Value> {
    if state.is_finished() {
        return Err(value_error("Inconsistent stream state"));
    }
    new_compressor(interp, state.clone())
}

fn copy_decompressor(interp: &mut Interpreter, state: &Inflater) -> Result<Value> {
    new_decompressor(interp, state.clone())
}

fn cannot_create(name: &str) -> PyError {
    type_error(format!("cannot create 'zlib.{name}' instances"))
}

fn cannot_pickle(name: &str) -> PyError {
    type_error(format!("cannot pickle 'zlib.{name}' object"))
}

fn readonly_attribute() -> PyError {
    PyError::named("AttributeError", "readonly attribute".to_string())
}

// ── Module definition ────────────────────────────────────────────────────────

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__version__" => Value::string("1.0"),
        "MAX_WBITS" => Value::int(i64::from(MAX_WBITS)),
        "DEFLATED" => Value::int(i64::from(DEFLATED)),
        "DEF_MEM_LEVEL" => Value::int(i64::from(DEF_MEM_LEVEL)),
        "DEF_BUF_SIZE" => Value::int(DEF_BUF_SIZE),
        "Z_NO_COMPRESSION" => Value::int(0),
        "Z_BEST_SPEED" => Value::int(1),
        "Z_BEST_COMPRESSION" => Value::int(9),
        "Z_DEFAULT_COMPRESSION" => Value::int(i64::from(Z_DEFAULT_COMPRESSION)),
        "Z_DEFAULT_STRATEGY" => Value::int(0),
        "Z_FILTERED" => Value::int(1),
        "Z_HUFFMAN_ONLY" => Value::int(2),
        "Z_RLE" => Value::int(3),
        "Z_FIXED" => Value::int(4),
        "Z_NO_FLUSH" => Value::int(0),
        "Z_PARTIAL_FLUSH" => Value::int(1),
        "Z_SYNC_FLUSH" => Value::int(2),
        "Z_FULL_FLUSH" => Value::int(3),
        "Z_FINISH" => Value::int(i64::from(Z_FINISH)),
        "Z_BLOCK" => Value::int(5),
        "Z_TREES" => Value::int(6),
        "ZLIB_VERSION" => Value::string("1.3.1"),
        "ZLIB_RUNTIME_VERSION" => Value::string("1.3.1"),
    }

    // No docstring: CPython 3.12 `zlib.error.__doc__` is `None`.
    exception error;

    fn crc32(
        #[positional_only] data: PyValueRef<'_>,
        #[positional_only] #[default(OptionalArg::Omitted)] value: OptValueRef<'_>,
    ) -> Result<Value> {
        let (data, init) = checksum_args(_interp, data.0, &value, 0)?;
        Ok(Value::int(i64::from(crc32_sum(&data, init))))
    }

    fn adler32(
        #[positional_only] data: PyValueRef<'_>,
        #[positional_only] #[default(OptionalArg::Omitted)] value: OptValueRef<'_>,
    ) -> Result<Value> {
        let (data, init) = checksum_args(_interp, data.0, &value, 1)?;
        Ok(Value::int(i64::from(adler32_sum(&data, init))))
    }

    fn compress(
        #[positional_only] data: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] level: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] wbits: OptValueRef<'_>,
    ) -> Result<Value> {
        let data = extract_bytes_like(_interp, data.0, "data")?;
        let level = opt_c_int(_interp, &level, Z_DEFAULT_COMPRESSION, "level")?;
        let wbits = opt_c_int(_interp, &wbits, MAX_WBITS, "wbits")?;
        let mut def = match Deflater::new(level, DEFLATED, wbits, DEF_MEM_LEVEL, 0, Vec::new()) {
            Ok(def) => def,
            Err(_) => return Err(raise_zlib_error(_interp, "Bad compression level")),
        };
        let mut out = def
            .compress(&data)
            .map_err(|e| raise_zlib_error(_interp, e))?;
        let tail = def.flush(Z_FINISH).map_err(|e| raise_zlib_error(_interp, e))?;
        out.extend_from_slice(&tail);
        Ok(Value::bytes(out))
    }

    fn decompress(
        #[positional_only] data: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] wbits: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] bufsize: OptValueRef<'_>,
    ) -> Result<Value> {
        let data = extract_bytes_like(_interp, data.0, "data")?;
        let wbits = opt_c_int(_interp, &wbits, MAX_WBITS, "wbits")?;
        let bufsize = opt_ssize(_interp, &bufsize, DEF_BUF_SIZE, "bufsize")?;
        if bufsize < 0 {
            return Err(value_error("bufsize must be non-negative"));
        }
        let mut inf = match Inflater::new(wbits, Vec::new()) {
            Ok(inf) => inf,
            Err(_) => {
                return Err(raise_zlib_error(
                    _interp,
                    "Error -2 while preparing to decompress data: inconsistent stream state",
                ));
            }
        };
        let out = inf
            .decompress(&data, 0)
            .map_err(|e| raise_zlib_error(_interp, e))?;
        if !inf.eof {
            return Err(raise_zlib_error(
                _interp,
                "Error -5 while decompressing data: incomplete or truncated stream",
            ));
        }
        Ok(Value::bytes(out))
    }

    #[allow(non_snake_case)]
    fn compressobj(
        #[default(OptionalArg::Omitted)] level: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] method: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] wbits: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] memLevel: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] strategy: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] zdict: OptValueRef<'_>,
    ) -> Result<Value> {
        let level = opt_c_int(_interp, &level, Z_DEFAULT_COMPRESSION, "level")?;
        let method = opt_c_int(_interp, &method, DEFLATED, "method")?;
        let wbits = opt_c_int(_interp, &wbits, MAX_WBITS, "wbits")?;
        let mem_level = opt_c_int(_interp, &memLevel, DEF_MEM_LEVEL, "memLevel")?;
        let strategy = opt_c_int(_interp, &strategy, 0, "strategy")?;
        let zdict = match &zdict {
            OptionalArg::Provided(v) => extract_bytes_like(_interp, v.0, "zdict")?.into_vec(),
            OptionalArg::Omitted => Vec::new(),
        };
        let has_zdict = !zdict.is_empty();
        let def = Deflater::new(level, method, wbits, mem_level, strategy, zdict)
            .map_err(|_| value_error("Invalid initialization option"))?;
        // `deflateSetDictionary` is rejected for the gzip wrapper (`wbits > 15`).
        if has_zdict && wbits > MAX_WBITS {
            return Err(value_error("Invalid dictionary"));
        }
        new_compressor(_interp, def)
    }

    fn decompressobj(
        #[default(OptionalArg::Omitted)] wbits: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] zdict: OptValueRef<'_>,
    ) -> Result<Value> {
        let wbits = opt_c_int(_interp, &wbits, MAX_WBITS, "wbits")?;
        let zdict = match &zdict {
            OptionalArg::Provided(v) => {
                if !PyBytesLike::matches(v.0) {
                    return Err(type_error(
                        "zdict argument must support the buffer protocol".to_string(),
                    ));
                }
                extract_bytes_like(_interp, v.0, "zdict")?.into_vec()
            }
            OptionalArg::Omitted => Vec::new(),
        };
        let inf = Inflater::new(wbits, zdict)
            .map_err(|_| value_error("Invalid initialization option"))?;
        new_decompressor(_interp, inf)
    }

    // The type returned by `compressobj()`; not exported by `zlib`.
    #[native_state(Deflater)]
    class Compress {
        fn __init__(_args) -> Result<Value> {
            Err(cannot_create("Compress"))
        }

        fn compress(
            #[state_mut] state: &mut Deflater,
            #[positional_only] data: PyValueRef<'_>,
        ) -> Result<Value> {
            let data = extract_bytes_like(_interp, data.0, "data")?;
            let out = state
                .compress(&data)
                .map_err(|e| raise_zlib_error(_interp, e))?;
            Ok(Value::bytes(out))
        }

        fn flush(
            #[state_mut] state: &mut Deflater,
            #[positional_only] #[default(OptionalArg::Omitted)] mode: OptValueRef<'_>,
        ) -> Result<Value> {
            let mode = opt_c_int(_interp, &mode, Z_FINISH, "mode")?;
            // CPython answers `Z_NO_FLUSH` before touching the stream, so it
            // succeeds even after the stream has been finished.
            if mode == 0 {
                return Ok(Value::bytes(Vec::new()));
            }
            let out = state.flush(mode).map_err(|e| raise_zlib_error(_interp, e))?;
            Ok(Value::bytes(out))
        }

        fn __reduce__(#[positional_only] _this: PyValueRef<'_>) -> Result<Value> {
            Err(cannot_pickle("Compress"))
        }

        fn __reduce_ex__(
            #[positional_only] _this: PyValueRef<'_>,
            #[positional_only] _protocol: PyValueRef<'_>,
        ) -> Result<Value> {
            Err(cannot_pickle("Compress"))
        }

        fn copy(#[state] state: &Deflater) -> Result<Value> {
            copy_compressor(_interp, state)
        }

        fn __copy__(#[state] state: &Deflater) -> Result<Value> {
            copy_compressor(_interp, state)
        }

        fn __deepcopy__(
            #[state] state: &Deflater,
            #[positional_only] _memo: PyValueRef<'_>,
        ) -> Result<Value> {
            copy_compressor(_interp, state)
        }
    }

    // The type returned by `decompressobj()`; not exported by `zlib`.
    #[native_state(Inflater)]
    class Decompress {
        fn __init__(_args) -> Result<Value> {
            Err(cannot_create("Decompress"))
        }

        fn decompress(
            #[state_mut] state: &mut Inflater,
            #[positional_only] data: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] max_length: OptValueRef<'_>,
        ) -> Result<Value> {
            let data = extract_bytes_like(_interp, data.0, "data")?;
            let max_length = opt_ssize(_interp, &max_length, 0, "max_length")?;
            if max_length < 0 {
                return Err(value_error("max_length must be non-negative"));
            }
            let out = state
                .decompress(&data, max_length as usize)
                .map_err(|e| raise_zlib_error(_interp, e))?;
            Ok(Value::bytes(out))
        }

        fn flush(
            #[state_mut] state: &mut Inflater,
            #[positional_only] #[default(OptionalArg::Omitted)] length: OptValueRef<'_>,
        ) -> Result<Value> {
            let length = opt_ssize(_interp, &length, DEF_BUF_SIZE, "length")?;
            if length <= 0 {
                return Err(value_error("length must be greater than zero"));
            }
            let out = state.flush().map_err(|e| raise_zlib_error(_interp, e))?;
            Ok(Value::bytes(out))
        }

        fn __reduce__(#[positional_only] _this: PyValueRef<'_>) -> Result<Value> {
            Err(cannot_pickle("Decompress"))
        }

        fn __reduce_ex__(
            #[positional_only] _this: PyValueRef<'_>,
            #[positional_only] _protocol: PyValueRef<'_>,
        ) -> Result<Value> {
            Err(cannot_pickle("Decompress"))
        }

        fn copy(#[state] state: &Inflater) -> Result<Value> {
            copy_decompressor(_interp, state)
        }

        fn __copy__(#[state] state: &Inflater) -> Result<Value> {
            copy_decompressor(_interp, state)
        }

        fn __deepcopy__(
            #[state] state: &Inflater,
            #[positional_only] _memo: PyValueRef<'_>,
        ) -> Result<Value> {
            copy_decompressor(_interp, state)
        }

        #[getter]
        fn eof(#[state] state: &Inflater) -> Result<Value> {
            Ok(Value::bool_(state.eof))
        }

        #[setter("eof")]
        fn set_eof(
            #[positional_only] _this: PyValueRef<'_>,
            #[positional_only] _value: PyValueRef<'_>,
        ) -> Result<Value> {
            Err(readonly_attribute())
        }

        #[getter]
        fn unused_data(#[state] state: &Inflater) -> Result<Value> {
            Ok(Value::bytes(state.unused_data.clone()))
        }

        #[setter("unused_data")]
        fn set_unused_data(
            #[positional_only] _this: PyValueRef<'_>,
            #[positional_only] _value: PyValueRef<'_>,
        ) -> Result<Value> {
            Err(readonly_attribute())
        }

        #[getter]
        fn unconsumed_tail(#[state] state: &Inflater) -> Result<Value> {
            Ok(Value::bytes(state.unconsumed_tail.clone()))
        }

        #[setter("unconsumed_tail")]
        fn set_unconsumed_tail(
            #[positional_only] _this: PyValueRef<'_>,
            #[positional_only] _value: PyValueRef<'_>,
        ) -> Result<Value> {
            Err(readonly_attribute())
        }
    }
}
