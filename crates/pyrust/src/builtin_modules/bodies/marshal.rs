// `marshal` module — 100% native Rust implementation of Python 3.12's `marshal`
// module with CPython binary wire-format compatibility across versions 0–4
// (issue #3246).
//
// Supports: `version = 4`, `dumps`, `loads`, `dump`, `load` for `None`,
// `False`, `True`, `StopIteration`, `Ellipsis`, `int` (32-bit `TYPE_INT` &
// bigint `TYPE_LONG`), `float` (`TYPE_FLOAT` & `TYPE_BINARY_FLOAT`), `complex`
// (`TYPE_COMPLEX` & `TYPE_BINARY_COMPLEX`), `bytes` / `bytearray` /
// `memoryview`, `str` (ASCII / UTF-8 / interned / short), `tuple` (full &
// small), `list`, `dict`, `set`, `frozenset`, `FLAG_REF` / `TYPE_REF`
// reference interning (v3/v4), and `CodeType` objects (CPython 3.12 layout).
//
// Reference: <https://docs.python.org/3.12/library/marshal.html>

use std::collections::HashMap;
use std::rc::Rc;

use num_bigint::{BigInt, Sign};
use num_traits::ToPrimitive;
use pyrust_builtins::code::{CodeBuild, CodeState, is_code, with_code_state_ref};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBytesLike, PyIndex, PyValue};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, key_to_value, lookup_exc_class, value_type_name_str,
};
use crate::value::{PyDict, PySet, Value, ValueKind};

const MARSHAL_VERSION: i64 = 4;
const MAX_MARSHAL_STACK_DEPTH: usize = 2000;

// ── CPython 3.12 marshal type tags ───────────────────────────────────────────

const FLAG_REF: u8 = 0x80;

const TYPE_NULL: u8 = b'0';
const TYPE_NONE: u8 = b'N';
const TYPE_FALSE: u8 = b'F';
const TYPE_TRUE: u8 = b'T';
const TYPE_STOPITER: u8 = b'S';
const TYPE_ELLIPSIS: u8 = b'.';
const TYPE_INT: u8 = b'i';
const TYPE_INT64: u8 = b'I';
const TYPE_FLOAT: u8 = b'f';
const TYPE_BINARY_FLOAT: u8 = b'g';
const TYPE_COMPLEX: u8 = b'x';
const TYPE_BINARY_COMPLEX: u8 = b'y';
const TYPE_LONG: u8 = b'l';
const TYPE_STRING: u8 = b's';
const TYPE_INTERNED: u8 = b't';
const TYPE_REF: u8 = b'r';
const TYPE_TUPLE: u8 = b'(';
const TYPE_LIST: u8 = b'[';
const TYPE_DICT: u8 = b'{';
const TYPE_CODE: u8 = b'c';
const TYPE_UNICODE: u8 = b'u';
const TYPE_UNKNOWN: u8 = b'?';
const TYPE_SET: u8 = b'<';
const TYPE_FROZENSET: u8 = b'>';
const TYPE_ASCII: u8 = b'a';
const TYPE_ASCII_INTERNED: u8 = b'A';
const TYPE_SMALL_TUPLE: u8 = b')';
const TYPE_SHORT_ASCII: u8 = b'z';
const TYPE_SHORT_ASCII_INTERNED: u8 = b'Z';
const TYPE_STRINGREF: u8 = b'R';

// CPython 3.12 `co_localspluskinds` bit flags (`Include/internal/pycore_code.h`).
const CO_FAST_LOCAL: u8 = 0x20;
const CO_FAST_CELL: u8 = 0x40;
const CO_FAST_FREE: u8 = 0x80;

// ── Encoder ──────────────────────────────────────────────────────────────────

#[derive(Hash, PartialEq, Eq)]
enum RefKey {
    ObjId(i64),
    Int(i64),
    Float(u64),
    Complex(u64, u64),
    Str(String),
}

struct MarshalWriter {
    buf: Vec<u8>,
    version: i64,
    depth: usize,
    refs: HashMap<RefKey, u32>,
}

impl MarshalWriter {
    fn new(version: i64) -> Self {
        Self {
            buf: Vec::with_capacity(64),
            version,
            depth: 0,
            refs: HashMap::new(),
        }
    }

    fn write_u8(&mut self, b: u8) {
        self.buf.push(b);
    }

    fn write_u16_le(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_i32_le(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32_le(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_f64_le(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_short_ascii_float(&mut self, f: f64) {
        let s = format_float_v0(f);
        let bytes = s.as_bytes();
        self.write_u8(bytes.len() as u8);
        self.buf.extend_from_slice(bytes);
    }

    /// Check reference table (versions >= 3). If `key` was already written,
    /// emits `TYPE_REF` and returns `Ok(true)`. Otherwise reserves a ref slot
    /// and returns `Ok(false)` along with the `FLAG_REF` bit to OR onto the
    /// type byte.
    fn try_write_ref(&mut self, key: Option<RefKey>) -> (bool, u8) {
        if self.version < 3 {
            return (false, 0);
        }
        let Some(k) = key else {
            return (false, 0);
        };
        if let Some(&idx) = self.refs.get(&k) {
            self.write_u8(TYPE_REF);
            self.write_u32_le(idx);
            return (true, 0);
        }
        let idx = self.refs.len() as u32;
        self.refs.insert(k, idx);
        (false, FLAG_REF)
    }

    fn ref_key_for(val: &Value) -> Option<RefKey> {
        match val.kind() {
            ValueKind::Int(n) => Some(RefKey::Int(n)),
            ValueKind::Float(f) => Some(RefKey::Float(f.to_bits())),
            ValueKind::Complex(re, im) => Some(RefKey::Complex(re.to_bits(), im.to_bits())),
            ValueKind::Str(s) => Some(RefKey::Str(s.to_owned())),
            ValueKind::BigInt(_)
            | ValueKind::Bytes(_)
            | ValueKind::Tuple(_)
            | ValueKind::List(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. } => val.object_id().as_int().map(RefKey::ObjId),
            _ => None,
        }
    }

    fn write_bigint_body(&mut self, bi: &BigInt) {
        let (sign, digits_u32) = bi.to_u32_digits();
        if sign == Sign::NoSign {
            self.write_i32_le(0);
            return;
        }
        // Convert base-2^32 little-endian digits to base-2^15 little-endian digits.
        let mut digits_15: Vec<u16> = Vec::new();
        let mut bit_buf: u64 = 0;
        let mut bit_count: u32 = 0;
        for &word in &digits_u32 {
            bit_buf |= (word as u64) << bit_count;
            bit_count += 32;
            while bit_count >= 15 {
                digits_15.push((bit_buf & 0x7FFF) as u16);
                bit_buf >>= 15;
                bit_count -= 15;
            }
        }
        if bit_buf > 0 {
            digits_15.push(bit_buf as u16);
        }
        while digits_15.last() == Some(&0) {
            digits_15.pop();
        }
        let count = digits_15.len() as i32;
        let signed_count = if sign == Sign::Minus { -count } else { count };
        self.write_i32_le(signed_count);
        for d in digits_15 {
            self.write_u16_le(d);
        }
    }

    fn write_str_val(&mut self, s: &str, flag: u8) {
        let bytes = s.as_bytes();
        if self.version >= 4 && s.is_ascii() {
            if bytes.len() < 256 {
                self.write_u8(TYPE_SHORT_ASCII | flag);
                self.write_u8(bytes.len() as u8);
            } else {
                self.write_u8(TYPE_ASCII | flag);
                self.write_u32_le(bytes.len() as u32);
            }
            self.buf.extend_from_slice(bytes);
        } else {
            self.write_u8(TYPE_UNICODE | flag);
            self.write_u32_le(bytes.len() as u32);
            self.buf.extend_from_slice(bytes);
        }
    }

    fn write_code_object(&mut self, cs: &CodeState, flag: u8) -> Result<()> {
        self.write_u8(TYPE_CODE | flag);
        self.write_i32_le(cs.argcount as i32);
        self.write_i32_le(cs.posonlyargcount as i32);
        self.write_i32_le(cs.kwonlyargcount as i32);
        self.write_i32_le(cs.stacksize as i32);
        self.write_i32_le(cs.flags as i32);

        // co_code
        self.write_object(&Value::bytes(cs.code_bytes.clone()))?;
        // co_consts
        self.write_object(&cs.consts)?;
        // co_names
        self.write_object(&cs.names)?;

        // co_localsplusnames + co_localspluskinds (CPython 3.11+ / 3.12 layout).
        let (lp_names, lp_kinds) = build_localsplus(cs);
        self.write_object(&Value::tuple(lp_names))?;
        self.write_object(&Value::bytes(lp_kinds))?;

        // co_filename, co_name, co_qualname
        self.write_object(&Value::string(&cs.filename))?;
        self.write_object(&Value::string(&cs.name))?;
        self.write_object(&Value::string(&cs.qualname))?;

        // co_firstlineno
        self.write_i32_le(cs.firstlineno as i32);
        // co_linetable, co_exceptiontable
        self.write_object(&Value::bytes(cs.linetable.clone()))?;
        self.write_object(&Value::bytes(cs.exceptiontable.clone()))?;
        Ok(())
    }

    fn write_object(&mut self, val: &Value) -> Result<()> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.write_object_inner(val))
    }

    fn write_object_inner(&mut self, val: &Value) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_MARSHAL_STACK_DEPTH {
            self.depth -= 1;
            return Err(PyError::named(
                "ValueError",
                "object too deeply nested to marshal",
            ));
        }
        let res = self.write_object_body(val);
        self.depth -= 1;
        res
    }

    fn write_object_body(&mut self, val: &Value) -> Result<()> {
        // Singletons (never tracked in refs table).
        match val.kind() {
            ValueKind::None => {
                self.write_u8(TYPE_NONE);
                return Ok(());
            }
            ValueKind::Bool(false) => {
                self.write_u8(TYPE_FALSE);
                return Ok(());
            }
            ValueKind::Bool(true) => {
                self.write_u8(TYPE_TRUE);
                return Ok(());
            }
            ValueKind::Ellipsis => {
                self.write_u8(TYPE_ELLIPSIS);
                return Ok(());
            }
            ValueKind::PyClass(cls) => {
                if let Some(stop_cls) = lookup_exc_class("StopIteration")
                    && Rc::ptr_eq(cls, &stop_cls)
                {
                    self.write_u8(TYPE_STOPITER);
                    return Ok(());
                }
                return Err(PyError::named("ValueError", "unmarshallable object"));
            }
            _ => {}
        }

        // Check reference interning (versions >= 3).
        // Avoid interning empty tuple or unhashable types only when unmarshallable.
        let can_marshal = is_marshallable(val);
        if !can_marshal {
            return Err(PyError::named("ValueError", "unmarshallable object"));
        }

        let (already_ref, flag) = self.try_write_ref(Self::ref_key_for(val));
        if already_ref {
            return Ok(());
        }

        match val.kind() {
            ValueKind::Int(n) => {
                if n >= i32::MIN as i64 && n <= i32::MAX as i64 {
                    self.write_u8(TYPE_INT | flag);
                    self.write_i32_le(n as i32);
                } else {
                    self.write_u8(TYPE_LONG | flag);
                    self.write_bigint_body(&BigInt::from(n));
                }
            }
            ValueKind::BigInt(bi) => {
                if let Some(n) = bi.to_i32() {
                    self.write_u8(TYPE_INT | flag);
                    self.write_i32_le(n);
                } else {
                    self.write_u8(TYPE_LONG | flag);
                    self.write_bigint_body(bi);
                }
            }
            ValueKind::Float(f) => {
                if self.version > 1 {
                    self.write_u8(TYPE_BINARY_FLOAT | flag);
                    self.write_f64_le(f);
                } else {
                    self.write_u8(TYPE_FLOAT | flag);
                    self.write_short_ascii_float(f);
                }
            }
            ValueKind::Complex(re, im) => {
                if self.version > 1 {
                    self.write_u8(TYPE_BINARY_COMPLEX | flag);
                    self.write_f64_le(re);
                    self.write_f64_le(im);
                } else {
                    self.write_u8(TYPE_COMPLEX | flag);
                    self.write_short_ascii_float(re);
                    self.write_short_ascii_float(im);
                }
            }
            ValueKind::Bytes(b) => {
                self.write_u8(TYPE_STRING | flag);
                self.write_u32_le(b.len() as u32);
                self.buf.extend_from_slice(b);
            }
            ValueKind::Str(s) => {
                self.write_str_val(s, flag);
            }
            ValueKind::Tuple(items) => {
                if self.version >= 4 && items.len() < 256 {
                    self.write_u8(TYPE_SMALL_TUPLE | flag);
                    self.write_u8(items.len() as u8);
                } else {
                    self.write_u8(TYPE_TUPLE | flag);
                    self.write_u32_le(items.len() as u32);
                }
                for item in items {
                    self.write_object(item)?;
                }
            }
            ValueKind::List(items) => {
                let snapshot: Vec<Value> = items.clone();
                drop(items);
                self.write_u8(TYPE_LIST | flag);
                self.write_u32_le(snapshot.len() as u32);
                for item in &snapshot {
                    self.write_object(item)?;
                }
            }
            ValueKind::Dict(dict) => {
                let pairs: Vec<(Value, Value)> = dict
                    .iter()
                    .map(|(k, v)| (key_to_value(k.clone()), v.clone()))
                    .collect();
                drop(dict);
                self.write_u8(TYPE_DICT | flag);
                for (k, v) in &pairs {
                    self.write_object(k)?;
                    self.write_object(v)?;
                }
                self.write_u8(TYPE_NULL);
            }
            ValueKind::Set(set) => {
                let items: Vec<Value> = set.iter().map(|k| key_to_value(k.clone())).collect();
                drop(set);
                self.write_u8(TYPE_SET | flag);
                self.write_u32_le(items.len() as u32);
                for item in &items {
                    self.write_object(item)?;
                }
            }
            ValueKind::BuiltinObject { .. } => {
                if let Some(items_rc) = pyrust_builtins::frozenset::as_items(val) {
                    self.write_u8(TYPE_FROZENSET | flag);
                    self.write_u32_le(items_rc.len() as u32);
                    for k in items_rc.iter() {
                        self.write_object(&key_to_value(k.clone()))?;
                    }
                } else if let Some(raw) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
                    self.write_u8(TYPE_STRING | flag);
                    self.write_u32_le(raw.len() as u32);
                    self.buf.extend_from_slice(&raw);
                } else if pyrust_builtins::memoryview::is_memoryview(val) {
                    let raw = pyrust_builtins::memoryview::to_bytes_checked(val)?;
                    self.write_u8(TYPE_STRING | flag);
                    self.write_u32_le(raw.len() as u32);
                    self.buf.extend_from_slice(&raw);
                } else if is_code(val) {
                    let res = with_code_state_ref(val, |cs| self.write_code_object(cs, flag));
                    match res {
                        Some(r) => r?,
                        None => {
                            return Err(PyError::named("ValueError", "unmarshallable object"));
                        }
                    }
                } else {
                    return Err(PyError::named("ValueError", "unmarshallable object"));
                }
            }
            _ => {
                return Err(PyError::named("ValueError", "unmarshallable object"));
            }
        }
        Ok(())
    }
}

fn is_marshallable(val: &Value) -> bool {
    match val.kind() {
        ValueKind::None
        | ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Complex(_, _)
        | ValueKind::Bytes(_)
        | ValueKind::Str(_)
        | ValueKind::Tuple(_)
        | ValueKind::List(_)
        | ValueKind::Dict(_)
        | ValueKind::Set(_)
        | ValueKind::Ellipsis => true,
        ValueKind::PyClass(cls) => {
            lookup_exc_class("StopIteration").is_some_and(|stop_cls| Rc::ptr_eq(cls, &stop_cls))
        }
        ValueKind::BuiltinObject { .. } => {
            pyrust_builtins::frozenset::as_items(val).is_some()
                || pyrust_builtins::bytearray::as_bytearray_snapshot(val).is_some()
                || pyrust_builtins::memoryview::is_memoryview(val)
                || is_code(val)
        }
        _ => false,
    }
}

fn format_float_v0(f: f64) -> String {
    if f.is_nan() {
        "nan".to_string()
    } else if f.is_infinite() {
        if f.is_sign_negative() {
            "-inf".to_string()
        } else {
            "inf".to_string()
        }
    } else {
        let s = format!("{f:?}");
        s
    }
}

fn build_localsplus(cs: &CodeState) -> (Vec<Value>, Vec<u8>) {
    let mut names: Vec<Value> = Vec::new();
    let mut kinds: Vec<u8> = Vec::new();
    let mut index_by_name: HashMap<String, usize> = HashMap::new();

    if let ValueKind::Tuple(vars) = cs.varnames.kind() {
        for v in vars {
            if let Some(s) = v.as_str() {
                index_by_name.insert(s.to_owned(), names.len());
                names.push(v.clone());
                kinds.push(CO_FAST_LOCAL);
            }
        }
    }
    if let ValueKind::Tuple(cells) = cs.cellvars.kind() {
        for v in cells {
            if let Some(s) = v.as_str() {
                if let Some(&idx) = index_by_name.get(s) {
                    kinds[idx] |= CO_FAST_CELL;
                } else {
                    index_by_name.insert(s.to_owned(), names.len());
                    names.push(v.clone());
                    kinds.push(CO_FAST_CELL);
                }
            }
        }
    }
    if let ValueKind::Tuple(frees) = cs.freevars.kind() {
        for v in frees {
            if let Some(s) = v.as_str() {
                if let Some(&idx) = index_by_name.get(s) {
                    kinds[idx] |= CO_FAST_FREE;
                } else {
                    index_by_name.insert(s.to_owned(), names.len());
                    names.push(v.clone());
                    kinds.push(CO_FAST_FREE);
                }
            }
        }
    }
    (names, kinds)
}

// ── Decoder ──────────────────────────────────────────────────────────────────

trait ByteSource {
    fn read_byte(&mut self, interp: &mut Interpreter) -> Result<u8>;
    fn read_exact(&mut self, interp: &mut Interpreter, n: usize) -> Result<Vec<u8>>;
}

struct SliceSource<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> ByteSource for SliceSource<'a> {
    fn read_byte(&mut self, _interp: &mut Interpreter) -> Result<u8> {
        if self.pos >= self.data.len() {
            return Err(PyError::named("EOFError", "marshal data too short"));
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_exact(&mut self, _interp: &mut Interpreter, n: usize) -> Result<Vec<u8>> {
        if self.data.len().saturating_sub(self.pos) < n {
            return Err(PyError::named("EOFError", "marshal data too short"));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice.to_vec())
    }
}

struct FileSource {
    read_fn: Value,
}

impl ByteSource for FileSource {
    fn read_byte(&mut self, interp: &mut Interpreter) -> Result<u8> {
        let bytes = self.read_exact(interp, 1)?;
        Ok(bytes[0])
    }

    fn read_exact(&mut self, interp: &mut Interpreter, n: usize) -> Result<Vec<u8>> {
        if n == 0 {
            return Ok(Vec::new());
        }
        let chunk = interp.call_function_expanded(
            self.read_fn.clone(),
            &[ExpandedCallArg {
                name: None,
                value: Value::int(n as i64),
            }],
        )?;
        let raw = match chunk.kind() {
            ValueKind::Bytes(b) => b.as_slice().to_vec(),
            ValueKind::BuiltinObject { .. } => {
                if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_snapshot(&chunk) {
                    ba
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!("read() returned {}, not bytes", value_type_name_str(&chunk)),
                    ));
                }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("read() returned {}, not bytes", value_type_name_str(&chunk)),
                ));
            }
        };
        if raw.len() < n {
            return Err(PyError::named("EOFError", "marshal data too short"));
        }
        Ok(raw)
    }
}

struct MarshalReader<S: ByteSource> {
    src: S,
    depth: usize,
    refs: Vec<Value>,
    interned: Vec<Value>,
}

impl<S: ByteSource> MarshalReader<S> {
    fn new(src: S) -> Self {
        Self {
            src,
            depth: 0,
            refs: Vec::new(),
            interned: Vec::new(),
        }
    }

    fn read_u8(&mut self, interp: &mut Interpreter) -> Result<u8> {
        self.src.read_byte(interp)
    }

    fn read_u16_le(&mut self, interp: &mut Interpreter) -> Result<u16> {
        let b = self.src.read_exact(interp, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_i32_le(&mut self, interp: &mut Interpreter) -> Result<i32> {
        let b = self.src.read_exact(interp, 4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u32_le(&mut self, interp: &mut Interpreter) -> Result<u32> {
        let b = self.src.read_exact(interp, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i64_le(&mut self, interp: &mut Interpreter) -> Result<i64> {
        let b = self.src.read_exact(interp, 8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_f64_le(&mut self, interp: &mut Interpreter) -> Result<f64> {
        let b = self.src.read_exact(interp, 8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_ascii_float(&mut self, interp: &mut Interpreter) -> Result<f64> {
        let len = self.read_u8(interp)? as usize;
        let bytes = self.src.read_exact(interp, len)?;
        let s = std::str::from_utf8(&bytes)
            .map_err(|_| PyError::named("ValueError", "bad marshal data (invalid float)"))?;
        s.trim()
            .parse::<f64>()
            .map_err(|_| PyError::named("ValueError", "bad marshal data (invalid float)"))
    }

    fn reserve_ref(&mut self, is_ref: bool) -> Option<usize> {
        if is_ref {
            let idx = self.refs.len();
            self.refs.push(Value::none());
            Some(idx)
        } else {
            None
        }
    }

    fn finish_ref(&mut self, slot: Option<usize>, val: Value) -> Value {
        if let Some(idx) = slot {
            self.refs[idx] = val.clone();
        }
        val
    }

    fn read_object(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.read_object_inner(interp))
    }

    fn read_object_inner(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        self.depth += 1;
        if self.depth > MAX_MARSHAL_STACK_DEPTH {
            self.depth -= 1;
            return Err(PyError::named("ValueError", "recursion limit exceeded"));
        }
        let res = self.read_object_body(interp);
        self.depth -= 1;
        res
    }

    fn read_object_required(&mut self, interp: &mut Interpreter) -> Result<Value> {
        self.read_object(interp)?
            .ok_or_else(|| PyError::named("ValueError", "bad marshal data (NULL object)"))
    }

    fn read_object_body(&mut self, interp: &mut Interpreter) -> Result<Option<Value>> {
        let code = self.read_u8(interp)?;
        let is_ref = (code & FLAG_REF) != 0;
        let tag = code & !FLAG_REF;

        match tag {
            TYPE_NULL => Ok(None),
            TYPE_NONE => Ok(Some(Value::none())),
            TYPE_FALSE => Ok(Some(Value::bool_(false))),
            TYPE_TRUE => Ok(Some(Value::bool_(true))),
            TYPE_ELLIPSIS => Ok(Some(Value::ellipsis())),
            TYPE_STOPITER => {
                let cls = lookup_exc_class("StopIteration").ok_or_else(|| {
                    PyError::runtime("built-in exception 'StopIteration' is not defined".into())
                })?;
                Ok(Some(Value::py_class(cls)))
            }
            TYPE_INT => {
                let n = self.read_i32_le(interp)? as i64;
                let slot = self.reserve_ref(is_ref);
                Ok(Some(self.finish_ref(slot, Value::int(n))))
            }
            TYPE_INT64 => {
                let n = self.read_i64_le(interp)?;
                let slot = self.reserve_ref(is_ref);
                Ok(Some(self.finish_ref(slot, Value::int(n))))
            }
            TYPE_LONG => {
                let slot = self.reserve_ref(is_ref);
                let n = self.read_i32_le(interp)?;
                let size = n.unsigned_abs() as usize;
                let mut digits = Vec::with_capacity(size);
                for _ in 0..size {
                    let d = self.read_u16_le(interp)?;
                    if d > 0x7FFF {
                        return Err(PyError::named(
                            "ValueError",
                            "bad marshal data (digit out of range in long)",
                        ));
                    }
                    digits.push(d);
                }
                let mut acc = BigInt::from(0);
                for &d in digits.iter().rev() {
                    acc = (acc << 15) | BigInt::from(d);
                }
                if n < 0 {
                    acc = -acc;
                }
                let val = match acc.to_i64() {
                    Some(i) => Value::int(i),
                    None => Value::bigint(acc),
                };
                Ok(Some(self.finish_ref(slot, val)))
            }
            TYPE_FLOAT => {
                let slot = self.reserve_ref(is_ref);
                let f = self.read_ascii_float(interp)?;
                Ok(Some(self.finish_ref(slot, Value::float(f))))
            }
            TYPE_BINARY_FLOAT => {
                let slot = self.reserve_ref(is_ref);
                let f = self.read_f64_le(interp)?;
                Ok(Some(self.finish_ref(slot, Value::float(f))))
            }
            TYPE_COMPLEX => {
                let slot = self.reserve_ref(is_ref);
                let re = self.read_ascii_float(interp)?;
                let im = self.read_ascii_float(interp)?;
                Ok(Some(self.finish_ref(slot, Value::complex(re, im))))
            }
            TYPE_BINARY_COMPLEX => {
                let slot = self.reserve_ref(is_ref);
                let re = self.read_f64_le(interp)?;
                let im = self.read_f64_le(interp)?;
                Ok(Some(self.finish_ref(slot, Value::complex(re, im))))
            }
            TYPE_STRING => {
                let slot = self.reserve_ref(is_ref);
                let len = self.read_u32_le(interp)? as usize;
                let bytes = self.src.read_exact(interp, len)?;
                Ok(Some(self.finish_ref(slot, Value::bytes(bytes))))
            }
            TYPE_UNICODE | TYPE_INTERNED | TYPE_ASCII | TYPE_ASCII_INTERNED => {
                let slot = self.reserve_ref(is_ref);
                let len = self.read_u32_le(interp)? as usize;
                let bytes = self.src.read_exact(interp, len)?;
                let s = String::from_utf8(bytes).map_err(|_| {
                    PyError::named("ValueError", "bad marshal data (invalid utf-8)")
                })?;
                let val = Value::string(s);
                if tag == TYPE_INTERNED {
                    self.interned.push(val.clone());
                }
                Ok(Some(self.finish_ref(slot, val)))
            }
            TYPE_SHORT_ASCII | TYPE_SHORT_ASCII_INTERNED => {
                let slot = self.reserve_ref(is_ref);
                let len = self.read_u8(interp)? as usize;
                let bytes = self.src.read_exact(interp, len)?;
                let s = String::from_utf8(bytes).map_err(|_| {
                    PyError::named("ValueError", "bad marshal data (invalid ascii)")
                })?;
                Ok(Some(self.finish_ref(slot, Value::string(s))))
            }
            TYPE_STRINGREF => {
                let idx = self.read_u32_le(interp)? as usize;
                let val = self.interned.get(idx).cloned().ok_or_else(|| {
                    PyError::named("ValueError", "bad marshal data (string ref out of range)")
                })?;
                Ok(Some(val))
            }
            TYPE_REF => {
                let idx = self.read_u32_le(interp)? as usize;
                let val = self.refs.get(idx).cloned().ok_or_else(|| {
                    PyError::named("ValueError", "bad marshal data (invalid reference)")
                })?;
                Ok(Some(val))
            }
            TYPE_TUPLE | TYPE_SMALL_TUPLE => {
                let slot = self.reserve_ref(is_ref);
                let len = if tag == TYPE_SMALL_TUPLE {
                    self.read_u8(interp)? as usize
                } else {
                    self.read_u32_le(interp)? as usize
                };
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.read_object_required(interp)?);
                }
                Ok(Some(self.finish_ref(slot, Value::tuple(items))))
            }
            TYPE_LIST => {
                let list_val = Value::list(Vec::new());
                let slot = self.reserve_ref(is_ref);
                if let Some(idx) = slot {
                    self.refs[idx] = list_val.clone();
                }
                let len = self.read_u32_le(interp)? as usize;
                let mut items = Vec::with_capacity(len.min(1024));
                for _ in 0..len {
                    items.push(self.read_object_required(interp)?);
                }
                for item in items {
                    list_val.list_push(item)?;
                }
                Ok(Some(list_val))
            }
            TYPE_DICT => {
                let dict_val = Value::dict(PyDict::default());
                let slot = self.reserve_ref(is_ref);
                if let Some(idx) = slot {
                    self.refs[idx] = dict_val.clone();
                }
                loop {
                    let Some(key_val) = self.read_object(interp)? else {
                        break;
                    };
                    let val = self.read_object_required(interp)?;
                    let py_key = interp.value_to_pykey(&key_val)?;
                    dict_val.dict_insert(py_key, val)?;
                }
                Ok(Some(dict_val))
            }
            TYPE_SET | TYPE_FROZENSET => {
                let slot = self.reserve_ref(is_ref);
                let len = self.read_u32_le(interp)? as usize;
                let mut set = PySet::default();
                for _ in 0..len {
                    let item = self.read_object_required(interp)?;
                    let py_key = interp.value_to_pykey(&item)?;
                    set.insert(py_key);
                }
                let val = if tag == TYPE_SET {
                    Value::set(set)
                } else {
                    pyrust_builtins::frozenset::frozenset(set)
                };
                Ok(Some(self.finish_ref(slot, val)))
            }
            TYPE_CODE => {
                let slot = self.reserve_ref(is_ref);
                let argcount = self.read_i32_le(interp)? as i64;
                let posonlyargcount = self.read_i32_le(interp)? as i64;
                let kwonlyargcount = self.read_i32_le(interp)? as i64;
                let stacksize = self.read_i32_le(interp)? as i64;
                let flags = self.read_i32_le(interp)? as i64;

                let code_val = self.read_object_required(interp)?;
                let code_bytes = match code_val.kind() {
                    ValueKind::Bytes(b) => b.as_slice().to_vec(),
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            "bad marshal data (code object co_code not bytes)",
                        ));
                    }
                };
                let consts_val = self.read_object_required(interp)?;
                let consts = match consts_val.kind() {
                    ValueKind::Tuple(t) => t.to_vec(),
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            "bad marshal data (code object co_consts not tuple)",
                        ));
                    }
                };
                let names_val = self.read_object_required(interp)?;
                let names = match names_val.kind() {
                    ValueKind::Tuple(t) => t.to_vec(),
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            "bad marshal data (code object co_names not tuple)",
                        ));
                    }
                };
                let lp_names_val = self.read_object_required(interp)?;
                let lp_names = match lp_names_val.kind() {
                    ValueKind::Tuple(t) => t.to_vec(),
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            "bad marshal data (code object localsplusnames not tuple)",
                        ));
                    }
                };
                let lp_kinds_val = self.read_object_required(interp)?;
                let lp_kinds = match lp_kinds_val.kind() {
                    ValueKind::Bytes(b) => b.as_slice().to_vec(),
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            "bad marshal data (code object localspluskinds not bytes)",
                        ));
                    }
                };

                let mut varnames = Vec::new();
                let mut cellvars = Vec::new();
                let mut freevars = Vec::new();
                for (name_v, &kind) in lp_names.iter().zip(lp_kinds.iter()) {
                    if kind & CO_FAST_LOCAL != 0 {
                        varnames.push(name_v.clone());
                    }
                    if kind & CO_FAST_CELL != 0 {
                        cellvars.push(name_v.clone());
                    }
                    if kind & CO_FAST_FREE != 0 {
                        freevars.push(name_v.clone());
                    }
                }
                let nlocals = varnames.len() as i64;

                let filename_val = self.read_object_required(interp)?;
                let filename = filename_val
                    .as_str()
                    .ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            "bad marshal data (code object co_filename not str)",
                        )
                    })?
                    .to_owned();
                let name_val = self.read_object_required(interp)?;
                let name = name_val
                    .as_str()
                    .ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            "bad marshal data (code object co_name not str)",
                        )
                    })?
                    .to_owned();
                let qualname_val = self.read_object_required(interp)?;
                let qualname = qualname_val
                    .as_str()
                    .ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            "bad marshal data (code object co_qualname not str)",
                        )
                    })?
                    .to_owned();

                let firstlineno = self.read_i32_le(interp)? as i64;
                let linetable_val = self.read_object_required(interp)?;
                let linetable = match linetable_val.kind() {
                    ValueKind::Bytes(b) => b.as_slice().to_vec(),
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            "bad marshal data (code object co_linetable not bytes)",
                        ));
                    }
                };
                let exctable_val = self.read_object_required(interp)?;
                let exceptiontable = match exctable_val.kind() {
                    ValueKind::Bytes(b) => b.as_slice().to_vec(),
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            "bad marshal data (code object co_exceptiontable not bytes)",
                        ));
                    }
                };

                let ln_opt = (firstlineno > 0).then_some(firstlineno);
                let co = CodeBuild {
                    name,
                    qualname,
                    argcount,
                    posonlyargcount,
                    kwonlyargcount,
                    nlocals,
                    stacksize,
                    varnames,
                    flags,
                    filename,
                    firstlineno,
                    consts,
                    names,
                    freevars,
                    cellvars,
                    code_bytes,
                    linetable,
                    exceptiontable,
                    lines: vec![(0, 2, ln_opt)],
                    positions: vec![(ln_opt, ln_opt, None, None)],
                    raw_code: None,
                    exec_meta: None,
                }
                .build();
                Ok(Some(self.finish_ref(slot, co)))
            }
            TYPE_UNKNOWN => Err(PyError::named(
                "ValueError",
                "bad marshal data (unknown type code)",
            )),
            other => Err(PyError::named(
                "ValueError",
                format!("bad marshal data (unknown type code 0x{other:02x})"),
            )),
        }
    }
}

fn marshal_dumps_impl(val: &Value, version: i64) -> Result<Vec<u8>> {
    let mut writer = MarshalWriter::new(version);
    writer.write_object(val)?;
    Ok(writer.buf)
}

fn marshal_loads_impl(interp: &mut Interpreter, data: &[u8]) -> Result<Value> {
    let mut reader = MarshalReader::new(SliceSource { data, pos: 0 });
    reader.read_object_required(interp)
}

pyrust_module! {
    constants {
        "version" => Value::int(MARSHAL_VERSION),
    }

    fn dumps(
        #[positional_only] value: PyValue,
        #[positional_only] #[default(None)] version: Option<PyIndex>,
    ) -> Result<Value> {
        let ver = match version {
            Some(v) => v.expect_i64("dumps", "version")?,
            None => MARSHAL_VERSION,
        };
        let bytes = marshal_dumps_impl(&value.0, ver)?;
        Ok(Value::bytes(bytes))
    }

    fn loads(#[positional_only] bytes: PyBytesLike<'_>) -> Result<Value> {
        marshal_loads_impl(_interp, bytes.as_bytes())
    }

    fn dump(
        #[positional_only] value: PyValue,
        #[positional_only] file: PyValue,
        #[positional_only] #[default(None)] version: Option<PyIndex>,
    ) -> Result<Value> {
        let ver = match version {
            Some(v) => v.expect_i64("dump", "version")?,
            None => MARSHAL_VERSION,
        };
        let write_fn = _interp.get_attr(&file.0, "write").map_err(|_| {
            PyError::named(
                "TypeError",
                "file.write() must be a callable accepting bytes",
            )
        })?;
        let bytes = marshal_dumps_impl(&value.0, ver)?;
        _interp.call_function_expanded(
            write_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::bytes(bytes),
            }],
        )?;
        Ok(Value::none())
    }

    fn load(#[positional_only] file: PyValue) -> Result<Value> {
        let read_fn = _interp.get_attr(&file.0, "read").map_err(|_| {
            PyError::named(
                "TypeError",
                "file must have a 'read' method",
            )
        })?;
        let mut reader = MarshalReader::new(FileSource { read_fn });
        reader.read_object_required(_interp)
    }
}
