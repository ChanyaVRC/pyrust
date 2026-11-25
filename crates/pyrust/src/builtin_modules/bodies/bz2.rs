// `bz2` module — body for the `bz2 @inject` entry in
// `pyrust_builtin_modules!` (issue #3224).
//
// Pure-Rust BZip2 compression and decompression backed by `bzip2`
// (`libbz2-rs-sys`, zero C dependencies).  The public Python 3.12 surface
// (`BZ2Compressor`, `BZ2Decompressor`, `BZ2File`, `open`, `compress`,
// `decompress`) is injected from `bz2_py.py`.
//
// Reference: <https://docs.python.org/3/library/bz2.html>

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use bzip2::{
    Action as BzAction, Compress as BzCompress, Compression, Decompress as BzDecompress,
    Error as BzError, Status as BzStatus,
};
use pyrust_core::BuiltinTypeOps;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
    value_type_name_str,
};
use crate::value::{PyKey, PyToPrimitive, Value, ValueKind};

/// Python-source definitions for the public `bz2` surface.
const BZ2_PY_SOURCE: &str = include_str!("bz2_py.py");

/// Private native helpers seeded into the injected exec namespace and removed
/// from `bz2.__dict__` after injection so `dir(bz2)` matches CPython.
const BZ2_NATIVE_HELPERS: [&str; 5] = [
    "_compressor_new",
    "_compressor_compress",
    "_compressor_flush",
    "_decompressor_new",
    "_decompressor_decompress",
];

/// Public names exported from `BZ2_PY_SOURCE` onto the `bz2` module.
const BZ2_PY_EXPORTS: [&str; 6] = [
    "BZ2Compressor",
    "BZ2Decompressor",
    "BZ2File",
    "compress",
    "decompress",
    "open",
];

/// Execute `BZ2_PY_SOURCE` once, hide internal helpers, and export the
/// public CPython 3.12 `bz2` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in BZ2_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(BZ2_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("bz2: exec namespace not a dict".into()))?;
    for name in BZ2_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("bz2"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Opaque native compressor / decompressor state ────────────────────────────

struct Bz2CompressorState {
    inner: BzCompress,
    flushed: bool,
}

impl Bz2CompressorState {
    fn new(level: i32) -> Result<Self> {
        let Some(comp) = u32::try_from(level).ok().and_then(Compression::try_new) else {
            return Err(PyError::named(
                "ValueError",
                "compresslevel must be between 1 and 9".to_string(),
            ));
        };
        Ok(Self {
            inner: BzCompress::new(comp, 0),
            flushed: false,
        })
    }

    fn compress(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        if self.flushed {
            return Err(PyError::named(
                "ValueError",
                "Compressor has been flushed".to_string(),
            ));
        }
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < data.len() {
            out.reserve(8192);
            let before_in = self.inner.total_in();
            let before_out = self.inner.total_out();
            let status = self
                .inner
                .compress_vec(&data[pos..], &mut out, BzAction::Run)
                .map_err(|e| PyError::named("OSError", format!("bzip2 compression error: {e}")))?;
            let consumed = (self.inner.total_in() - before_in) as usize;
            let produced = (self.inner.total_out() - before_out) as usize;
            pos += consumed;
            if status == BzStatus::StreamEnd {
                break;
            }
            if consumed == 0 && produced == 0 {
                out.reserve(out.capacity().max(8192));
            }
        }
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<u8>> {
        if self.flushed {
            return Err(PyError::named(
                "ValueError",
                "Compressor has been flushed".to_string(),
            ));
        }
        self.flushed = true;
        let mut out = Vec::new();
        loop {
            out.reserve(8192);
            let status = self
                .inner
                .compress_vec(&[], &mut out, BzAction::Finish)
                .map_err(|e| PyError::named("OSError", format!("bzip2 flush error: {e}")))?;
            if status == BzStatus::StreamEnd {
                break;
            }
        }
        Ok(out)
    }
}

struct Bz2DecompressorState {
    inner: BzDecompress,
    pending_in: Vec<u8>,
    pending_pos: usize,
    eof: bool,
    needs_input: bool,
    unused_data: Vec<u8>,
}

impl Bz2DecompressorState {
    fn new() -> Self {
        Self {
            inner: BzDecompress::new(false),
            pending_in: Vec::new(),
            pending_pos: 0,
            eof: false,
            needs_input: true,
            unused_data: Vec::new(),
        }
    }

    fn compact_pending(&mut self) {
        if self.pending_pos == 0 {
            return;
        }
        if self.pending_pos >= self.pending_in.len() {
            self.pending_in.clear();
            self.pending_pos = 0;
        } else if self.pending_pos >= 4096 {
            self.pending_in.drain(..self.pending_pos);
            self.pending_pos = 0;
        }
    }

    fn decompress(&mut self, data: &[u8], max_length: i64) -> Result<Vec<u8>> {
        if self.eof {
            return Err(PyError::named(
                "EOFError",
                "End of stream already reached".to_string(),
            ));
        }

        if !data.is_empty() {
            self.compact_pending();
            self.pending_in.extend_from_slice(data);
        }

        if max_length == 0 {
            if self.pending_pos < self.pending_in.len() {
                self.needs_input = false;
            }
            return Ok(Vec::new());
        }

        let limit: Option<usize> = if max_length < 0 {
            None
        } else {
            Some(max_length as usize)
        };

        let mut out = Vec::new();
        let mut scratch = [0u8; 16384];

        loop {
            let step = match limit {
                Some(lim) => {
                    let rem = lim.saturating_sub(out.len());
                    if rem == 0 {
                        // Output limit reached before StreamEnd; libbz2 or pending_in may
                        // still hold buffered data.
                        self.needs_input = false;
                        self.compact_pending();
                        return Ok(out);
                    }
                    rem.min(scratch.len())
                }
                None => scratch.len(),
            };

            let input_slice = &self.pending_in[self.pending_pos..];
            let before_in = self.inner.total_in();
            let before_out = self.inner.total_out();
            let status = match self.inner.decompress(input_slice, &mut scratch[..step]) {
                Ok(s) => s,
                Err(BzError::Data | BzError::DataMagic) => {
                    return Err(PyError::named("OSError", "Invalid data stream".to_string()));
                }
                Err(e) => {
                    return Err(PyError::named(
                        "OSError",
                        format!("Invalid data stream: {e}"),
                    ));
                }
            };
            let consumed = (self.inner.total_in() - before_in) as usize;
            let produced = (self.inner.total_out() - before_out) as usize;
            self.pending_pos += consumed;
            if produced > 0 {
                out.extend_from_slice(&scratch[..produced]);
            }

            if status == BzStatus::StreamEnd {
                self.eof = true;
                self.needs_input = false;
                self.unused_data = self.pending_in[self.pending_pos..].to_vec();
                self.pending_in.clear();
                self.pending_pos = 0;
                return Ok(out);
            }

            if consumed == 0 && produced == 0 {
                // Need more input bytes to make further progress.
                self.needs_input = true;
                self.pending_in.clear();
                self.pending_pos = 0;
                return Ok(out);
            }
        }
    }
}

struct Bz2CompressorOps;
const BZ2_COMPRESSOR_OPS: &Bz2CompressorOps = &Bz2CompressorOps;

impl BuiltinTypeOps for Bz2CompressorOps {
    fn type_name(&self) -> &'static str {
        "_bz2_compressor"
    }
}

struct Bz2DecompressorOps;
const BZ2_DECOMPRESSOR_OPS: &Bz2DecompressorOps = &Bz2DecompressorOps;

impl BuiltinTypeOps for Bz2DecompressorOps {
    fn type_name(&self) -> &'static str {
        "_bz2_decompressor"
    }
}

fn wrap_compressor(state: Bz2CompressorState) -> Value {
    let boxed: Box<dyn Any> = Box::new(state);
    Value::builtin_object(BZ2_COMPRESSOR_OPS, boxed)
}

fn wrap_decompressor(state: Bz2DecompressorState) -> Value {
    let boxed: Box<dyn Any> = Box::new(state);
    Value::builtin_object(BZ2_DECOMPRESSOR_OPS, boxed)
}

fn with_compressor_mut<R>(
    val: &Value,
    action: impl FnOnce(&mut Bz2CompressorState) -> R,
) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid bz2 compressor state".into()));
    };
    if !pyrust_core::builtin_ops_is::<Bz2CompressorOps>(ops) {
        return Err(PyError::runtime("invalid bz2 compressor state".into()));
    }
    let mut borrow = state.borrow_mut();
    let c = borrow
        .downcast_mut::<Bz2CompressorState>()
        .ok_or_else(|| PyError::runtime("invalid bz2 compressor state".into()))?;
    Ok(action(c))
}

fn with_decompressor_mut<R>(
    val: &Value,
    action: impl FnOnce(&mut Bz2DecompressorState) -> R,
) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid bz2 decompressor state".into()));
    };
    if !pyrust_core::builtin_ops_is::<Bz2DecompressorOps>(ops) {
        return Err(PyError::runtime("invalid bz2 decompressor state".into()));
    }
    let mut borrow = state.borrow_mut();
    let d = borrow
        .downcast_mut::<Bz2DecompressorState>()
        .ok_or_else(|| PyError::runtime("invalid bz2 decompressor state".into()))?;
    Ok(action(d))
}

// ── Buffer & integer extraction helpers ──────────────────────────────────────

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

fn extract_ssize_t(interp: &mut Interpreter, val: &Value) -> Result<i64> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Bool(b) => Ok(i64::from(b)),
        ValueKind::Int(n) => Ok(n),
        ValueKind::BigInt(bi) => bi.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t".to_string(),
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

pyrust_module! {
    fn _compressor_new(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_compressor_new expects 1 arg".into()));
        }
        let level = extract_c_int(_interp, &args[0].value)?;
        let state = Bz2CompressorState::new(level)?;
        Ok(wrap_compressor(state))
    }

    fn _compressor_compress(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::runtime("_compressor_compress expects 2 args".into()));
        }
        let data = extract_bytes_like(&args[1].value)?;
        let out = with_compressor_mut(&args[0].value, |c| c.compress(&data))??;
        Ok(Value::bytes(out))
    }

    fn _compressor_flush(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_compressor_flush expects 1 arg".into()));
        }
        let out = with_compressor_mut(&args[0].value, Bz2CompressorState::flush)??;
        Ok(Value::bytes(out))
    }

    fn _decompressor_new(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::runtime("_decompressor_new expects 0 args".into()));
        }
        Ok(wrap_decompressor(Bz2DecompressorState::new()))
    }

    fn _decompressor_decompress(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::runtime("_decompressor_decompress expects 3 args".into()));
        }
        let data = extract_bytes_like(&args[1].value)?;
        let max_length = extract_ssize_t(_interp, &args[2].value)?;
        let result = with_decompressor_mut(&args[0].value, |d| {
            let out = d.decompress(&data, max_length)?;
            Ok::<Value, PyError>(Value::tuple(vec![
                Value::bytes(out),
                Value::bool_(d.eof),
                Value::bool_(d.needs_input),
                Value::bytes(d.unused_data.clone()),
            ]))
        })??;
        Ok(result)
    }
}
