// `multiprocessing.sharedctypes` — 100% native-Rust Python 3.12 shared-memory
// `Value`, `Array`, `RawValue`, `RawArray`, `Synchronized`, `SynchronizedArray`,
// `SynchronizedString`, `synchronized`, `copy` (issue #3264).
//
// Backed by real OS shared memory (`OsSharedMem`) so cross-process mutations
// in child processes are immediately visible in the parent and sibling processes.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::builtin_modules::mp_rt::call_pos;
use crate::builtin_modules::mp_rt::shm::OsSharedMem;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, NativeIterFrame, value_type_name_str};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, PyModule, Value, ValueKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SharedElemType {
    Char,     // 'c' / c_char (1B bytes)
    WChar,    // 'u' / c_wchar (4B UTF-32 code point)
    Bool,     // '?' / c_bool (1B)
    Int8,     // 'b' / c_byte
    UInt8,    // 'B' / c_ubyte
    Int16,    // 'h' / c_short
    UInt16,   // 'H' / c_ushort
    Int32,    // 'i' / c_int
    UInt32,   // 'I' / c_uint
    CLong,    // 'l' / c_long (i64 on LP64, i32 on Windows LLP64)
    CULong,   // 'L' / c_ulong
    Int64,    // 'q' / c_longlong
    UInt64,   // 'Q' / c_ulonglong
    Float32,  // 'f' / c_float
    Float64,  // 'd' / c_double
}

impl SharedElemType {
    fn from_code(code: &str) -> Result<Self> {
        match code {
            "c" => Ok(Self::Char),
            "u" => Ok(Self::WChar),
            "?" => Ok(Self::Bool),
            "b" => Ok(Self::Int8),
            "B" => Ok(Self::UInt8),
            "h" => Ok(Self::Int16),
            "H" => Ok(Self::UInt16),
            "i" => Ok(Self::Int32),
            "I" => Ok(Self::UInt32),
            "l" => Ok(Self::CLong),
            "L" => Ok(Self::CULong),
            "q" => Ok(Self::Int64),
            "Q" => Ok(Self::UInt64),
            "f" => Ok(Self::Float32),
            "d" => Ok(Self::Float64),
            _ => Err(PyError::named("KeyError", format!("'{code}'"))),
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Char => "c",
            Self::WChar => "u",
            Self::Bool => "?",
            Self::Int8 => "b",
            Self::UInt8 => "B",
            Self::Int16 => "h",
            Self::UInt16 => "H",
            Self::Int32 => "i",
            Self::UInt32 => "I",
            Self::CLong => "l",
            Self::CULong => "L",
            Self::Int64 => "q",
            Self::UInt64 => "Q",
            Self::Float32 => "f",
            Self::Float64 => "d",
        }
    }

    fn itemsize(self) -> usize {
        match self {
            Self::Char | Self::Bool | Self::Int8 | Self::UInt8 => 1,
            Self::Int16 | Self::UInt16 => 2,
            Self::WChar | Self::Int32 | Self::UInt32 | Self::Float32 => 4,
            Self::CLong | Self::CULong => std::mem::size_of::<std::ffi::c_long>(),
            Self::Int64 | Self::UInt64 | Self::Float64 => 8,
        }
    }

    fn read_from(self, shm: &OsSharedMem, idx: usize) -> Result<Value> {
        let off = idx * self.itemsize();
        let mut buf = [0u8; 8];
        shm.read_bytes(off, &mut buf[..self.itemsize()])?;
        Ok(match self {
            Self::Char => Value::bytes(vec![buf[0]]),
            Self::WChar => {
                let cp = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
                let ch = char::from_u32(cp).unwrap_or('\0');
                Value::string(ch.to_string())
            }
            Self::Bool => Value::bool_(buf[0] != 0),
            Self::Int8 => Value::int((buf[0] as i8) as i64),
            Self::UInt8 => Value::int(buf[0] as i64),
            Self::Int16 => Value::int(i16::from_ne_bytes([buf[0], buf[1]]) as i64),
            Self::UInt16 => Value::int(u16::from_ne_bytes([buf[0], buf[1]]) as i64),
            Self::Int32 => Value::int(i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64),
            Self::UInt32 => Value::int(u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64),
            Self::CLong => {
                if self.itemsize() == 4 {
                    Value::int(i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64)
                } else {
                    Value::int(i64::from_ne_bytes(buf))
                }
            }
            Self::CULong => {
                if self.itemsize() == 4 {
                    Value::int(u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64)
                } else {
                    let u = u64::from_ne_bytes(buf);
                    if u <= i64::MAX as u64 {
                        Value::int(u as i64)
                    } else {
                        Value::bigint(num_bigint::BigInt::from(u))
                    }
                }
            }
            Self::Int64 => Value::int(i64::from_ne_bytes(buf)),
            Self::UInt64 => {
                let u = u64::from_ne_bytes(buf);
                if u <= i64::MAX as u64 {
                    Value::int(u as i64)
                } else {
                    Value::bigint(num_bigint::BigInt::from(u))
                }
            }
            Self::Float32 => {
                Value::float(f32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as f64)
            }
            Self::Float64 => Value::float(f64::from_ne_bytes(buf)),
        })
    }

    fn write_to(self, interp: &mut Interpreter, shm: &OsSharedMem, idx: usize, val: &Value) -> Result<()> {
        let off = idx * self.itemsize();
        match self {
            Self::Char => {
                let b = match val.kind() {
                    ValueKind::Bytes(bytes) if bytes.len() == 1 => bytes[0],
                    ValueKind::Int(n) => n as u8,
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "one character bytes, bytearray or integer expected",
                        ));
                    }
                };
                shm.write_byte(off, b)
            }
            Self::WChar => {
                let cp = match val.kind() {
                    ValueKind::Str(s) if s.chars().count() == 1 => {
                        s.chars().next().unwrap_or('\0') as u32
                    }
                    _ => return Err(PyError::named("TypeError", "a unicode character is required")),
                };
                shm.write_bytes(off, &cp.to_ne_bytes())
            }
            Self::Bool => {
                let b = interp.truthy_value(val)?;
                shm.write_byte(off, u8::from(b))
            }
            Self::Int8 => shm.write_byte(off, (val_as_i64(val)? as i8) as u8),
            Self::UInt8 => shm.write_byte(off, val_as_i64(val)? as u8),
            Self::Int16 => shm.write_bytes(off, &(val_as_i64(val)? as i16).to_ne_bytes()),
            Self::UInt16 => shm.write_bytes(off, &(val_as_i64(val)? as u16).to_ne_bytes()),
            Self::Int32 => shm.write_bytes(off, &(val_as_i64(val)? as i32).to_ne_bytes()),
            Self::UInt32 => shm.write_bytes(off, &(val_as_i64(val)? as u32).to_ne_bytes()),
            Self::CLong => {
                let n = val_as_i64(val)?;
                if self.itemsize() == 4 {
                    shm.write_bytes(off, &(n as i32).to_ne_bytes())
                } else {
                    shm.write_bytes(off, &n.to_ne_bytes())
                }
            }
            Self::CULong => {
                let n = val_as_u64(val)?;
                if self.itemsize() == 4 {
                    shm.write_bytes(off, &(n as u32).to_ne_bytes())
                } else {
                    shm.write_bytes(off, &n.to_ne_bytes())
                }
            }
            Self::Int64 => shm.write_bytes(off, &val_as_i64(val)?.to_ne_bytes()),
            Self::UInt64 => shm.write_bytes(off, &val_as_u64(val)?.to_ne_bytes()),
            Self::Float32 => shm.write_bytes(off, &(val_as_f64(val)? as f32).to_ne_bytes()),
            Self::Float64 => shm.write_bytes(off, &val_as_f64(val)?.to_ne_bytes()),
        }
    }
}

fn val_as_i64(val: &Value) -> Result<i64> {
    match val.kind() {
        ValueKind::Int(i) => Ok(i),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            format!("an integer is required (got type {})", value_type_name_str(val)),
        )),
    }
}

fn val_as_u64(val: &Value) -> Result<u64> {
    match val.kind() {
        ValueKind::Int(i) => Ok(i as u64),
        ValueKind::Bool(b) => Ok(u64::from(b)),
        ValueKind::BigInt(bi) => {
            use pyrust_core::PyToPrimitive;
            Ok(bi.to_u64().unwrap_or(0))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("an integer is required (got type {})", value_type_name_str(val)),
        )),
    }
}

fn val_as_f64(val: &Value) -> Result<f64> {
    match val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(i) => Ok(i as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        _ => Err(PyError::named(
            "TypeError",
            format!("a float is required (got type {})", value_type_name_str(val)),
        )),
    }
}

fn resolve_elem_type(interp: &mut Interpreter, typecode_or_type: &Value) -> Result<SharedElemType> {
    if let Some(s) = typecode_or_type.as_str() {
        return SharedElemType::from_code(s);
    }
    if let Ok(tc) = interp.get_attr(typecode_or_type, "_type_")
        && let Some(s) = tc.as_str()
    {
        return SharedElemType::from_code(s);
    }
    Err(PyError::named(
        "TypeError",
        "typecode_or_type must be a typecode string or ctypes type",
    ))
}

struct RawValueState {
    shm: Arc<OsSharedMem>,
    elem: SharedElemType,
}

struct RawArrayState {
    shm: Arc<OsSharedMem>,
    elem: SharedElemType,
    len: usize,
}

struct SynchronizedState {
    obj: Value,
    lock: Value,
}

fn make_instance(interp: &mut Interpreter, cls_name: &str) -> Result<Value> {
    let m = interp.load_module("multiprocessing.sharedctypes")?;
    let cls = interp.get_attr(&m, cls_name)?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("RuntimeError", format!("{cls_name} invalid")));
    };
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    }))))
}

fn resolve_lock_arg(interp: &mut Interpreter, lock_opt: Option<&Value>) -> Result<Option<Value>> {
    match lock_opt {
        None => {
            let sync = interp.load_module("multiprocessing.synchronize")?;
            let rlock = interp.get_attr(&sync, "RLock")?;
            Ok(Some(call_pos(interp, rlock, &[])?))
        }
        Some(v) => match v.kind() {
            ValueKind::Bool(false) => Ok(None),
            ValueKind::Bool(true) => {
                let sync = interp.load_module("multiprocessing.synchronize")?;
                let rlock = interp.get_attr(&sync, "RLock")?;
                Ok(Some(call_pos(interp, rlock, &[])?))
            }
            _ => Ok(Some(v.clone())),
        },
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let ctypes = interp.load_module("ctypes")?;
    let map = Value::dict(PyDict::default());
    for (code, cname) in [
        ("c", "c_char"),
        ("u", "c_wchar"),
        ("b", "c_byte"),
        ("B", "c_ubyte"),
        ("h", "c_short"),
        ("H", "c_ushort"),
        ("i", "c_int"),
        ("I", "c_uint"),
        ("l", "c_long"),
        ("L", "c_ulong"),
        ("q", "c_longlong"),
        ("Q", "c_ulonglong"),
        ("f", "c_float"),
        ("d", "c_double"),
    ] {
        if let Ok(t) = interp.get_attr(&ctypes, cname) {
            map.dict_insert(PyKey::str_from(code), t)?;
        }
    }
    module
        .borrow_mut()
        .insert_attr("typecode_to_type".to_string(), map);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("RawValue"),
            Value::string("RawArray"),
            Value::string("Value"),
            Value::string("Array"),
            Value::string("copy"),
            Value::string("synchronized"),
        ]),
    }

    fn _rebuild_raw_value(
        #[positional_only] shm_name: PyStr<'_>,
        #[positional_only] code: PyStr<'_>,
    ) -> Result<Value> {
        let elem = SharedElemType::from_code(&code.0)?;
        let shm = OsSharedMem::open(Some(&shm_name.0), false, elem.itemsize(), false)?;
        let inst = make_instance(_interp, "_RawValueObj")?;
        set_native_state(&inst, RawValueState { shm, elem })?;
        Ok(inst)
    }

    fn _rebuild_raw_array(
        #[positional_only] shm_name: PyStr<'_>,
        #[positional_only] code: PyStr<'_>,
        #[positional_only] len: PyIndex,
    ) -> Result<Value> {
        let elem = SharedElemType::from_code(&code.0)?;
        let count = len.expect_i64(FN_NAME, "len")? as usize;
        let byte_size = (count * elem.itemsize()).max(1);
        let shm = OsSharedMem::open(Some(&shm_name.0), false, byte_size, false)?;
        let inst = make_instance(_interp, "_RawArrayObj")?;
        set_native_state(
            &inst,
            RawArrayState {
                shm,
                elem,
                len: count,
            },
        )?;
        Ok(inst)
    }

    fn _rebuild_synchronized(
        #[positional_only] cls_name: PyStr<'_>,
        #[positional_only] obj: PyValueRef<'_>,
        #[positional_only] lock: PyValueRef<'_>,
    ) -> Result<Value> {
        let inst = make_instance(_interp, &cls_name.0)?;
        set_native_state(
            &inst,
            SynchronizedState {
                obj: obj.0.clone(),
                lock: lock.0.clone(),
            },
        )?;
        Ok(inst)
    }

    #[native_state(RawValueState)]
    class _RawValueObj {
        #[getter]
        fn value(#[state] st: &RawValueState) -> Result<Value> {
            st.elem.read_from(&st.shm, 0)
        }

        #[setter("value")]
        fn set_value(
            #[positional_only] self_val: PyValueRef<'_>,
            v: PyValue,
        ) -> Result<Value> {
            let (shm, elem) = with_state(self_val.0, |st| Ok((Arc::clone(&st.shm), st.elem)))?;
            elem.write_to(_interp, &shm, 0, &v.0)?;
            Ok(Value::none())
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (name, code) = with_state(self_val.0, |st| {
                Ok((st.shm.name.clone(), st.elem.code()))
            })?;
            let m = _interp.load_module("multiprocessing.sharedctypes")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_raw_value")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![Value::string(&name), Value::string(code)]),
            ]))
        }
    }

    #[native_state(RawArrayState)]
    class _RawArrayObj {
        fn __len__(#[state] st: &RawArrayState) -> Result<Value> {
            Ok(Value::int(st.len as i64))
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            let (shm, elem, len) =
                with_state(self_val.0, |st| Ok((Arc::clone(&st.shm), st.elem, st.len)))?;
            if let Some((lo, hi, stp)) = pyrust_builtins::slice::slice_fields(&key.0) {
                let (start, stop, step) =
                    pyrust_builtins::slice::compute_indices(len as i64, &lo, &hi, &stp)?;
                let mut out = Vec::new();
                let mut cur = start;
                if step > 0 {
                    while cur < stop {
                        out.push(elem.read_from(&shm, cur as usize)?);
                        cur += step;
                    }
                } else {
                    while cur > stop {
                        out.push(elem.read_from(&shm, cur as usize)?);
                        cur += step;
                    }
                }
                if elem == SharedElemType::Char {
                    let bytes: Vec<u8> = out
                        .into_iter()
                        .filter_map(|v| match v.kind() {
                            ValueKind::Bytes(b) => b.first().copied(),
                            _ => None,
                        })
                        .collect();
                    return Ok(Value::bytes(bytes));
                }
                return Ok(Value::list(out));
            }
            let raw_idx = _interp.call_index_protocol(&key.0, "array indices")?;
            let idx_i64 = raw_idx
                .as_int()
                .ok_or_else(|| PyError::named("IndexError", "array index out of range"))?;
            let norm = if idx_i64 < 0 {
                idx_i64 + len as i64
            } else {
                idx_i64
            };
            if norm < 0 || norm >= len as i64 {
                return Err(PyError::named("IndexError", "array index out of range"));
            }
            elem.read_from(&shm, norm as usize)
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] val: PyValue,
        ) -> Result<Value> {
            let (shm, elem, len) =
                with_state(self_val.0, |st| Ok((Arc::clone(&st.shm), st.elem, st.len)))?;
            if let Some((lo, hi, stp)) = pyrust_builtins::slice::slice_fields(&key.0) {
                let (start, stop, step) =
                    pyrust_builtins::slice::compute_indices(len as i64, &lo, &hi, &stp)?;
                let mut indices = Vec::new();
                let mut cur = start;
                if step > 0 {
                    while cur < stop {
                        indices.push(cur as usize);
                        cur += step;
                    }
                } else {
                    while cur > stop {
                        indices.push(cur as usize);
                        cur += step;
                    }
                }
                let rhs = if elem == SharedElemType::Char {
                    if let ValueKind::Bytes(b) = val.0.kind() {
                        b.iter().map(|&byte| Value::bytes(vec![byte])).collect()
                    } else {
                        _interp.collect_iterable(&val.0)?
                    }
                } else {
                    _interp.collect_iterable(&val.0)?
                };
                if indices.len() != rhs.len() {
                    return Err(PyError::named(
                        "ValueError",
                        "slice assignment has wrong length",
                    ));
                }
                for (i, item) in indices.into_iter().zip(rhs.iter()) {
                    elem.write_to(_interp, &shm, i, item)?;
                }
                return Ok(Value::none());
            }
            let raw_idx = _interp.call_index_protocol(&key.0, "array indices")?;
            let idx_i64 = raw_idx
                .as_int()
                .ok_or_else(|| PyError::named("IndexError", "array index out of range"))?;
            let norm = if idx_i64 < 0 {
                idx_i64 + len as i64
            } else {
                idx_i64
            };
            if norm < 0 || norm >= len as i64 {
                return Err(PyError::named("IndexError", "array index out of range"));
            }
            elem.write_to(_interp, &shm, norm as usize, &val.0)?;
            Ok(Value::none())
        }

        fn __iter__(#[state] st: &RawArrayState) -> Result<Value> {
            let mut items = Vec::with_capacity(st.len);
            for i in 0..st.len {
                items.push(st.elem.read_from(&st.shm, i)?);
            }
            Ok(Value::generator(Box::new(NativeIterFrame::generator(items))))
        }

        #[getter]
        fn raw(#[state] st: &RawArrayState) -> Result<Value> {
            if st.elem != SharedElemType::Char {
                return Err(PyError::named(
                    "AttributeError",
                    "raw attribute only available for c_char arrays",
                ));
            }
            let mut buf = vec![0u8; st.len];
            st.shm.read_bytes(0, &mut buf)?;
            Ok(Value::bytes(buf))
        }

        #[setter("raw")]
        fn set_raw(
            #[state] st: &RawArrayState,
            v: crate::interpreter::builtin_args::PyBytesLike<'_>,
        ) -> Result<Value> {
            if st.elem != SharedElemType::Char {
                return Err(PyError::named(
                    "AttributeError",
                    "raw attribute only available for c_char arrays",
                ));
            }
            let bytes = v.as_bytes();
            if bytes.len() > st.len {
                return Err(PyError::named("ValueError", "byte string too long"));
            }
            st.shm.write_bytes(0, bytes)?;
            if bytes.len() < st.len {
                let zeros = vec![0u8; st.len - bytes.len()];
                st.shm.write_bytes(bytes.len(), &zeros)?;
            }
            Ok(Value::none())
        }

        #[getter]
        fn value(#[state] st: &RawArrayState) -> Result<Value> {
            if st.elem != SharedElemType::Char {
                return Err(PyError::named(
                    "AttributeError",
                    "value attribute only available for c_char arrays",
                ));
            }
            let mut buf = vec![0u8; st.len];
            st.shm.read_bytes(0, &mut buf)?;
            let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            Ok(Value::bytes(buf[..nul].to_vec()))
        }

        #[setter("value")]
        fn set_value(
            #[state] st: &RawArrayState,
            v: crate::interpreter::builtin_args::PyBytesLike<'_>,
        ) -> Result<Value> {
            if st.elem != SharedElemType::Char {
                return Err(PyError::named(
                    "AttributeError",
                    "value attribute only available for c_char arrays",
                ));
            }
            let bytes = v.as_bytes();
            if bytes.len() > st.len {
                return Err(PyError::named("ValueError", "byte string too long"));
            }
            st.shm.write_bytes(0, bytes)?;
            if bytes.len() < st.len {
                let zeros = vec![0u8; st.len - bytes.len()];
                st.shm.write_bytes(bytes.len(), &zeros)?;
            }
            Ok(Value::none())
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (name, code, len) = with_state(self_val.0, |st| {
                Ok((st.shm.name.clone(), st.elem.code(), st.len))
            })?;
            let m = _interp.load_module("multiprocessing.sharedctypes")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_raw_array")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    Value::string(&name),
                    Value::string(code),
                    Value::int(len as i64),
                ]),
            ]))
        }
    }

    #[native_state(SynchronizedState)]
    class SynchronizedBase {
        fn get_obj(#[state] st: &SynchronizedState) -> Result<Value> {
            Ok(st.obj.clone())
        }

        fn get_lock(#[state] st: &SynchronizedState) -> Result<Value> {
            Ok(st.lock.clone())
        }

        fn acquire(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let f = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, f, args)
        }

        fn release(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let f = _interp.get_attr(&lock, "release")?;
            call_pos(_interp, f, &[])
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let f = _interp.get_attr(&lock, "__enter__")?;
            call_pos(_interp, f, &[])
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let f = _interp.get_attr(&lock, "__exit__")?;
            call_pos(_interp, f, args)
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls_name = match self_val.0.kind() {
                ValueKind::PyInstance(rc) => rc.borrow().class.borrow().name.clone(),
                _ => "Synchronized".to_string(),
            };
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let m = _interp.load_module("multiprocessing.sharedctypes")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_synchronized")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![Value::string(&cls_name), obj, lock]),
            ]))
        }
    }

    #[native_state(SynchronizedState)]
    class Synchronized : "SynchronizedBase" {
        #[getter]
        fn value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp.get_attr(&obj, "value");
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res
        }

        #[setter("value")]
        fn set_value(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp.assign_attr(obj, "value", val.0);
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res?;
            Ok(Value::none())
        }
    }

    #[native_state(SynchronizedState)]
    class SynchronizedArray : "SynchronizedBase" {
        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let obj = with_state(self_val.0, |st| Ok(st.obj.clone()))?;
            let f = _interp.get_attr(&obj, "__len__")?;
            call_pos(_interp, f, &[])
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp
                .get_attr(&obj, "__getitem__")
                .and_then(|f| call_pos(_interp, f, &[key.0]));
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] val: PyValue,
        ) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp
                .get_attr(&obj, "__setitem__")
                .and_then(|f| call_pos(_interp, f, &[key.0, val.0]));
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp
                .get_attr(&obj, "__iter__")
                .and_then(|f| call_pos(_interp, f, &[]));
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res
        }
    }

    #[native_state(SynchronizedState)]
    class SynchronizedString : "SynchronizedArray" {
        #[getter]
        fn value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp.get_attr(&obj, "value");
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res
        }

        #[setter("value")]
        fn set_value(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp.assign_attr(obj, "value", val.0);
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res?;
            Ok(Value::none())
        }

        #[getter]
        fn raw(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp.get_attr(&obj, "raw");
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res
        }

        #[setter("raw")]
        fn set_raw(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let (obj, lock) = with_state(self_val.0, |st| Ok((st.obj.clone(), st.lock.clone())))?;
            let acq = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, acq, &[])?;
            let res = _interp.assign_attr(obj, "raw", val.0);
            let rel = _interp.get_attr(&lock, "release")?;
            let _ = call_pos(_interp, rel, &[]);
            res?;
            Ok(Value::none())
        }
    }

    #[py_name = "RawValue"]
    fn raw_value(
        #[positional_only] typecode_or_type: PyValueRef<'_>,
        #[varargs] args: &[Value],
    ) -> Result<Value> {
        let elem = resolve_elem_type(_interp, typecode_or_type.0)?;
        let shm = OsSharedMem::open(None, true, elem.itemsize().max(1), true)?;
        if let Some(init_val) = args.first() {
            elem.write_to(_interp, &shm, 0, init_val)?;
        }
        let inst = make_instance(_interp, "_RawValueObj")?;
        set_native_state(&inst, RawValueState { shm, elem })?;
        Ok(inst)
    }

    #[py_name = "RawArray"]
    fn raw_array(
        #[positional_only] typecode_or_type: PyValueRef<'_>,
        #[positional_only] size_or_initializer: PyValueRef<'_>,
    ) -> Result<Value> {
        let elem = resolve_elem_type(_interp, typecode_or_type.0)?;
        let (len, init_items) = match size_or_initializer.0.kind() {
            ValueKind::Int(n) => {
                if n < 0 {
                    return Err(PyError::named("ValueError", "array size must be >= 0"));
                }
                (n as usize, None)
            }
            ValueKind::Bytes(b) if elem == SharedElemType::Char => {
                let items: Vec<Value> = b.iter().map(|&byte| Value::bytes(vec![byte])).collect();
                (items.len(), Some(items))
            }
            _ => {
                let items = _interp.collect_iterable(size_or_initializer.0)?;
                (items.len(), Some(items))
            }
        };
        let byte_size = (len * elem.itemsize()).max(1);
        let shm = OsSharedMem::open(None, true, byte_size, true)?;
        if let Some(items) = init_items {
            for (i, item) in items.iter().enumerate() {
                elem.write_to(_interp, &shm, i, item)?;
            }
        }
        let inst = make_instance(_interp, "_RawArrayObj")?;
        set_native_state(&inst, RawArrayState { shm, elem, len })?;
        Ok(inst)
    }

    fn synchronized(
        #[positional_only] obj: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] lock: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] _ctx: OptValueRef<'_>,
    ) -> Result<Value> {
        let Some(lock_val) = resolve_lock_arg(_interp, lock.as_value())? else {
            return Ok(obj.0.clone());
        };
        let cls_name = if let Ok(elem) =
            with_native_state::<RawArrayState, _>(obj.0, "_RawArrayObj", |st| Ok(st.elem))
        {
            if elem == SharedElemType::Char {
                "SynchronizedString"
            } else {
                "SynchronizedArray"
            }
        } else {
            "Synchronized"
        };
        let inst = make_instance(_interp, cls_name)?;
        set_native_state(
            &inst,
            SynchronizedState {
                obj: obj.0.clone(),
                lock: lock_val,
            },
        )?;
        Ok(inst)
    }

    #[py_name = "Value"]
    fn value_fn(
        #[positional_only] typecode_or_type: PyValueRef<'_>,
        #[varargs] args: &[Value],
        #[default(OptionalArg::Omitted)] lock: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
    ) -> Result<Value> {
        let _ = ctx;
        let m = _interp.load_module("multiprocessing.sharedctypes")?;
        let raw_fn = _interp.get_attr(&m, "RawValue")?;
        let mut raw_args = Vec::with_capacity(args.len() + 1);
        raw_args.push(typecode_or_type.0.clone());
        raw_args.extend(args.iter().cloned());
        let raw_obj = call_pos(_interp, raw_fn, &raw_args)?;
        let Some(lock_val) = resolve_lock_arg(_interp, lock.as_value())? else {
            return Ok(raw_obj);
        };
        let inst = make_instance(_interp, "Synchronized")?;
        set_native_state(
            &inst,
            SynchronizedState {
                obj: raw_obj,
                lock: lock_val,
            },
        )?;
        Ok(inst)
    }

    #[py_name = "Array"]
    fn array_fn(
        #[positional_only] typecode_or_type: PyValueRef<'_>,
        #[positional_only] size_or_initializer: PyValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] lock: OptValueRef<'_>,
        #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
    ) -> Result<Value> {
        let _ = ctx;
        let m = _interp.load_module("multiprocessing.sharedctypes")?;
        let raw_fn = _interp.get_attr(&m, "RawArray")?;
        let raw_obj = call_pos(
            _interp,
            raw_fn,
            &[typecode_or_type.0.clone(), size_or_initializer.0.clone()],
        )?;
        let Some(lock_val) = resolve_lock_arg(_interp, lock.as_value())? else {
            return Ok(raw_obj);
        };
        let elem =
            with_native_state::<RawArrayState, _>(&raw_obj, "_RawArrayObj", |st| Ok(st.elem))?;
        let cls_name = if elem == SharedElemType::Char {
            "SynchronizedString"
        } else {
            "SynchronizedArray"
        };
        let inst = make_instance(_interp, cls_name)?;
        set_native_state(
            &inst,
            SynchronizedState {
                obj: raw_obj,
                lock: lock_val,
            },
        )?;
        Ok(inst)
    }

    fn copy(#[positional_only] obj: PyValueRef<'_>) -> Result<Value> {
        let copy_mod = _interp.load_module("copy")?;
        let copy_fn = _interp.get_attr(&copy_mod, "copy")?;
        call_pos(_interp, copy_fn, std::slice::from_ref(obj.0))
    }
}
