//! Core runtime engine for PyRust's 100% native-Rust `ctypes` implementation.
//!
//! Provides:
//! - [`CDataStorage`] and [`CDataState`] (backing memory, subviews, keep-alives)
//! - Primitive / bitfield / compound marshaling with native, big, and little endianness
//! - Dynamic `Array`, `POINTER`, `Structure`/`Union`, and `CFUNCTYPE` class constructors
//! - FFI argument/return marshaling bridging Python `Value`s and [`abi`]

pub(crate) mod abi;
pub(crate) mod dynlib;
pub(crate) mod types;

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use num_bigint::BigInt;
use num_traits::ToPrimitive;

use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, class_is_subclass_of, value_type_name_str};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};

use abi::{AbiRet, AbiRetKind, AbiVal, CallbackSlotHandle};
use types::{
    ByteOrder, CTypeInfo, FieldLayout, LONGDOUBLE_SIZE, PTR_SIZE, PrimKind, StructLayout,
    WCHAR_SIZE, alloc_pyobject_handle, compute_struct_layout, core_classes, insert_array_cache,
    insert_pointer_cache, insert_swapped_cache, lookup_array_cache, lookup_ctype,
    lookup_ctype_exact, lookup_pointer_cache, lookup_pyobject_handle, lookup_swapped_cache,
    parse_pack_attr, register_ctype,
};

// ── Function Flag Constants ──────────────────────────────────────────────────

pub(crate) const FUNCFLAG_STDCALL: i32 = 0x0;
pub(crate) const FUNCFLAG_CDECL: i32 = 0x1;
pub(crate) const FUNCFLAG_HRESULT: i32 = 0x2;
pub(crate) const FUNCFLAG_PYTHONAPI: i32 = 0x4;
pub(crate) const FUNCFLAG_USE_ERRNO: i32 = 0x8;
pub(crate) const FUNCFLAG_USE_LASTERROR: i32 = 0x10;

// ── Memory Storage ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) enum CDataStorage {
    Managed {
        buf: Rc<RefCell<Vec<u8>>>,
        offset: usize,
        size: usize,
        keep_alive: Option<Value>,
    },
    Raw {
        ptr: *mut u8,
        size: usize,
        keep_alive: Option<Value>,
    },
}

impl CDataStorage {
    pub(crate) fn allocate_zeroed(size: usize) -> Self {
        // Reserve at least 16 bytes so system malloc returns a naturally aligned
        // pointer for C FFI even for small structs/scalars.
        let mut v = Vec::with_capacity(size.max(16));
        v.resize(size, 0u8);
        Self::Managed {
            buf: Rc::new(RefCell::new(v)),
            offset: 0,
            size,
            keep_alive: None,
        }
    }

    pub(crate) fn from_raw(ptr: *mut u8, size: usize, keep_alive: Option<Value>) -> Self {
        Self::Raw {
            ptr,
            size,
            keep_alive,
        }
    }

    pub(crate) fn from_shared_vec(
        buf: Rc<RefCell<Vec<u8>>>,
        offset: usize,
        size: usize,
        keep_alive: Option<Value>,
    ) -> Self {
        Self::Managed {
            buf,
            offset,
            size,
            keep_alive,
        }
    }

    #[inline]
    pub(crate) fn size(&self) -> usize {
        match self {
            Self::Managed { size, .. } | Self::Raw { size, .. } => *size,
        }
    }

    pub(crate) fn as_ptr(&self) -> *mut u8 {
        match self {
            Self::Managed { buf, offset, .. } => {
                let mut b = buf.borrow_mut();
                // SAFETY: `offset <= b.len()` is maintained as an invariant.
                unsafe { b.as_mut_ptr().add(*offset) }
            }
            Self::Raw { ptr, .. } => *ptr,
        }
    }

    pub(crate) fn address(&self) -> usize {
        self.as_ptr() as usize
    }

    pub(crate) fn read_bytes(&self, rel_off: usize, dst: &mut [u8]) -> Result<()> {
        if dst.is_empty() {
            return Ok(());
        }
        match self {
            Self::Managed { buf, offset, .. } => {
                let b = buf.borrow();
                let start = offset
                    .checked_add(rel_off)
                    .ok_or_else(|| PyError::named("ValueError", "buffer offset overflow"))?;
                let end = start
                    .checked_add(dst.len())
                    .ok_or_else(|| PyError::named("ValueError", "buffer read overflow"))?;
                if end > b.len() {
                    return Err(PyError::named("ValueError", "buffer read out of bounds"));
                }
                dst.copy_from_slice(&b[start..end]);
                Ok(())
            }
            Self::Raw { ptr, .. } => {
                if ptr.is_null() {
                    return Err(PyError::named("ValueError", "NULL pointer access"));
                }
                // SAFETY: Non-null raw pointer backed by live C memory or ctypes object.
                unsafe {
                    std::ptr::copy_nonoverlapping(ptr.add(rel_off), dst.as_mut_ptr(), dst.len());
                }
                Ok(())
            }
        }
    }

    pub(crate) fn write_bytes(&self, rel_off: usize, src: &[u8]) -> Result<()> {
        if src.is_empty() {
            return Ok(());
        }
        match self {
            Self::Managed { buf, offset, .. } => {
                let mut b = buf.borrow_mut();
                let start = offset
                    .checked_add(rel_off)
                    .ok_or_else(|| PyError::named("ValueError", "buffer offset overflow"))?;
                let end = start
                    .checked_add(src.len())
                    .ok_or_else(|| PyError::named("ValueError", "buffer write overflow"))?;
                if end > b.len() {
                    return Err(PyError::named("ValueError", "buffer write out of bounds"));
                }
                b[start..end].copy_from_slice(src);
                Ok(())
            }
            Self::Raw { ptr, .. } => {
                if ptr.is_null() {
                    return Err(PyError::named("ValueError", "NULL pointer access"));
                }
                // SAFETY: Non-null raw pointer backed by live writable memory.
                unsafe {
                    std::ptr::copy_nonoverlapping(src.as_ptr(), ptr.add(rel_off), src.len());
                }
                Ok(())
            }
        }
    }

    pub(crate) fn read_all(&self) -> Result<Vec<u8>> {
        let mut out = vec![0u8; self.size()];
        self.read_bytes(0, &mut out)?;
        Ok(out)
    }

    pub(crate) fn subview(&self, rel_off: usize, sub_size: usize, owner: Option<Value>) -> Self {
        match self {
            Self::Managed {
                buf,
                offset,
                keep_alive,
                ..
            } => Self::Managed {
                buf: Rc::clone(buf),
                offset: offset + rel_off,
                size: sub_size,
                keep_alive: owner.or_else(|| keep_alive.clone()),
            },
            Self::Raw {
                ptr, keep_alive, ..
            } => {
                let new_ptr = if ptr.is_null() {
                    std::ptr::null_mut()
                } else {
                    // SAFETY: `rel_off` is within the enclosing object layout.
                    unsafe { ptr.add(rel_off) }
                };
                Self::Raw {
                    ptr: new_ptr,
                    size: sub_size,
                    keep_alive: owner.or_else(|| keep_alive.clone()),
                }
            }
        }
    }

    pub(crate) fn resize(&mut self, new_size: usize) -> Result<()> {
        match self {
            Self::Managed {
                buf, offset, size, ..
            } => {
                if *offset != 0 {
                    return Err(PyError::named(
                        "ValueError",
                        "Memory cannot be resized because this object doesn't own it",
                    ));
                }
                let mut b = buf.borrow_mut();
                b.resize(new_size, 0u8);
                *size = new_size;
                Ok(())
            }
            Self::Raw { .. } => Err(PyError::named(
                "ValueError",
                "Memory cannot be resized because this object doesn't own it",
            )),
        }
    }

    pub(crate) fn export_memoryview(&self, owner: Value, writable: bool) -> Result<Value> {
        match self {
            Self::Managed {
                buf, offset, size, ..
            } if *offset == 0 && buf.borrow().len() == *size => {
                pyrust_builtins::memoryview::from_mutable_buffer(
                    Rc::clone(buf),
                    None,
                    owner,
                    "B",
                    false,
                )
            }
            _ => {
                if writable {
                    let bytes = self.read_all()?;
                    let shared = Rc::new(RefCell::new(bytes));
                    pyrust_builtins::memoryview::from_mutable_buffer(
                        shared, None, owner, "B", false,
                    )
                } else {
                    let bytes = self.read_all()?;
                    Ok(pyrust_builtins::memoryview::from_bytes(
                        Rc::new(bytes),
                        owner,
                    ))
                }
            }
        }
    }
}

// ── Native State Structs ─────────────────────────────────────────────────────

pub(crate) struct FuncPtrMeta {
    pub(crate) restype: Option<Value>,
    pub(crate) argtypes: Option<Vec<Value>>,
    pub(crate) errcheck: Option<Value>,
    pub(crate) flags: i32,
    pub(crate) name: Option<String>,
    pub(crate) callable: Option<Value>,
    pub(crate) _callback_handle: Option<CallbackSlotHandle>,
}

pub(crate) struct CDataState {
    pub(crate) storage: CDataStorage,
    pub(crate) keep_alive_bufs: IndexMap<String, Box<[u8]>>,
    pub(crate) keep_alive_vals: IndexMap<String, Value>,
    pub(crate) func_ptr: Option<FuncPtrMeta>,
}

impl CDataState {
    pub(crate) fn new(storage: CDataStorage) -> Self {
        Self {
            storage,
            keep_alive_bufs: IndexMap::new(),
            keep_alive_vals: IndexMap::new(),
            func_ptr: None,
        }
    }
}

pub(crate) struct CArgObjectState {
    pub(crate) obj: Value,
    pub(crate) _offset: isize,
    pub(crate) addr: usize,
    pub(crate) tag: String,
}

pub(crate) struct CFieldState {
    pub(crate) field: FieldLayout,
    pub(crate) owner_swapped: bool,
}

// ── Integer / Float Conversion Helpers ───────────────────────────────────────

pub(crate) fn value_to_u64_wrapping(interp: &mut Interpreter, val: &Value) -> Result<u64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n as u64),
        ValueKind::Bool(b) => Ok(u64::from(b)),
        ValueKind::BigInt(bi) => {
            // Two's complement modulo 2^64 matching CPython ctypes integer wrapping
            let mask = (BigInt::from(1u8) << 64usize) - 1u8;
            let wrapped = bi & &mask;
            Ok(wrapped.to_u64().unwrap_or(0))
        }
        _ => {
            let idx = interp.call_index_protocol(val, "an integer is required")?;
            value_to_u64_wrapping(interp, &idx)
        }
    }
}

pub(crate) fn value_to_f64(interp: &mut Interpreter, val: &Value) -> Result<f64> {
    match val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(n) => Ok(n as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        ValueKind::BigInt(bi) => bi
            .to_f64()
            .ok_or_else(|| PyError::named("OverflowError", "int too large to convert to float")),
        _ => {
            if let Some(f_method) =
                crate::interpreter::lookup_value_special_method(val, "__float__").transpose()?
            {
                let res =
                    crate::interpreter::invoke_class_method(interp, f_method, val.clone(), &[])?;
                if let ValueKind::Float(f) = res.kind() {
                    return Ok(f);
                }
            }
            if let Ok(idx) = interp.call_index_protocol(val, "must be real number") {
                return value_to_f64(interp, &idx);
            }
            Err(PyError::named(
                "TypeError",
                format!("must be real number, not {}", value_type_name_str(val)),
            ))
        }
    }
}

pub(crate) fn u64_to_py_value(v: u64) -> Value {
    if let Ok(i) = i64::try_from(v) {
        Value::int(i)
    } else {
        Value::bigint(BigInt::from(v))
    }
}

// ── Primitive Memory Read / Write ────────────────────────────────────────────

fn read_u16(bytes: &[u8], bo: ByteOrder) -> u16 {
    let arr: [u8; 2] = [bytes[0], bytes[1]];
    match bo {
        ByteOrder::Native => u16::from_ne_bytes(arr),
        ByteOrder::Little => u16::from_le_bytes(arr),
        ByteOrder::Big => u16::from_be_bytes(arr),
    }
}

fn write_u16(val: u16, bo: ByteOrder) -> [u8; 2] {
    match bo {
        ByteOrder::Native => val.to_ne_bytes(),
        ByteOrder::Little => val.to_le_bytes(),
        ByteOrder::Big => val.to_be_bytes(),
    }
}

fn read_u32(bytes: &[u8], bo: ByteOrder) -> u32 {
    let arr: [u8; 4] = [bytes[0], bytes[1], bytes[2], bytes[3]];
    match bo {
        ByteOrder::Native => u32::from_ne_bytes(arr),
        ByteOrder::Little => u32::from_le_bytes(arr),
        ByteOrder::Big => u32::from_be_bytes(arr),
    }
}

fn write_u32(val: u32, bo: ByteOrder) -> [u8; 4] {
    match bo {
        ByteOrder::Native => val.to_ne_bytes(),
        ByteOrder::Little => val.to_le_bytes(),
        ByteOrder::Big => val.to_be_bytes(),
    }
}

fn read_u64(bytes: &[u8], bo: ByteOrder) -> u64 {
    let arr: [u8; 8] = [
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ];
    match bo {
        ByteOrder::Native => u64::from_ne_bytes(arr),
        ByteOrder::Little => u64::from_le_bytes(arr),
        ByteOrder::Big => u64::from_be_bytes(arr),
    }
}

fn write_u64(val: u64, bo: ByteOrder) -> [u8; 8] {
    match bo {
        ByteOrder::Native => val.to_ne_bytes(),
        ByteOrder::Little => val.to_le_bytes(),
        ByteOrder::Big => val.to_be_bytes(),
    }
}

pub(crate) fn read_ptr_usize(
    storage: &CDataStorage,
    offset: usize,
    bo: ByteOrder,
) -> Result<usize> {
    let mut buf = [0u8; PTR_SIZE];
    storage.read_bytes(offset, &mut buf)?;
    if PTR_SIZE == 8 {
        Ok(read_u64(&buf, bo) as usize)
    } else {
        Ok(read_u32(&buf, bo) as usize)
    }
}

pub(crate) fn write_ptr_usize(
    storage: &CDataStorage,
    offset: usize,
    addr: usize,
    bo: ByteOrder,
) -> Result<()> {
    if PTR_SIZE == 8 {
        storage.write_bytes(offset, &write_u64(addr as u64, bo))
    } else {
        storage.write_bytes(offset, &write_u32(addr as u32, bo))
    }
}

/// Read a null-terminated C byte string from raw memory address.
pub(crate) fn read_c_string(addr: usize, max_len: Option<usize>) -> Result<Vec<u8>> {
    if addr == 0 {
        return Err(PyError::named("ValueError", "NULL pointer access"));
    }
    let ptr = addr as *const u8;
    let mut out = Vec::new();
    if let Some(len) = max_len {
        out.resize(len, 0u8);
        // SAFETY: caller supplied valid readable C buffer of `len` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), len);
        }
    } else {
        let mut i = 0usize;
        loop {
            // SAFETY: caller supplied a null-terminated C string pointer.
            let b = unsafe { *ptr.add(i) };
            if b == 0 {
                break;
            }
            out.push(b);
            i += 1;
        }
    }
    Ok(out)
}

/// Read a null-terminated platform wide string (`wchar_t*`) from raw memory address.
pub(crate) fn read_c_wstring(addr: usize, max_chars: Option<usize>) -> Result<String> {
    if addr == 0 {
        return Err(PyError::named("ValueError", "NULL pointer access"));
    }
    if WCHAR_SIZE == 2 {
        let ptr = addr as *const u16;
        let mut units = Vec::new();
        if let Some(n) = max_chars {
            for i in 0..n {
                // SAFETY: caller supplied valid wide string of `n` UTF-16 units.
                units.push(unsafe { std::ptr::read_unaligned(ptr.add(i)) });
            }
        } else {
            let mut i = 0usize;
            loop {
                // SAFETY: caller supplied null-terminated UTF-16 wide string.
                let u = unsafe { std::ptr::read_unaligned(ptr.add(i)) };
                if u == 0 {
                    break;
                }
                units.push(u);
                i += 1;
            }
        }
        Ok(String::from_utf16_lossy(&units))
    } else {
        let ptr = addr as *const u32;
        let mut out = String::new();
        if let Some(n) = max_chars {
            for i in 0..n {
                // SAFETY: caller supplied valid wide string of `n` UTF-32 code points.
                let cp = unsafe { std::ptr::read_unaligned(ptr.add(i)) };
                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
            }
        } else {
            let mut i = 0usize;
            loop {
                // SAFETY: caller supplied null-terminated UTF-32 wide string.
                let cp = unsafe { std::ptr::read_unaligned(ptr.add(i)) };
                if cp == 0 {
                    break;
                }
                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                i += 1;
            }
        }
        Ok(out)
    }
}

pub(crate) fn encode_wchar_null_terminated(s: &str) -> Box<[u8]> {
    if WCHAR_SIZE == 2 {
        let mut bytes = Vec::with_capacity((s.len() + 1) * 2);
        for u in s.encode_utf16() {
            bytes.extend_from_slice(&u.to_ne_bytes());
        }
        bytes.extend_from_slice(&0u16.to_ne_bytes());
        bytes.into_boxed_slice()
    } else {
        let mut bytes = Vec::with_capacity((s.chars().count() + 1) * 4);
        for ch in s.chars() {
            bytes.extend_from_slice(&(ch as u32).to_ne_bytes());
        }
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.into_boxed_slice()
    }
}

pub(crate) fn read_prim_value(
    storage: &CDataStorage,
    offset: usize,
    kind: PrimKind,
    bo: ByteOrder,
) -> Result<Value> {
    let mut buf = [0u8; 16];
    let sz = kind.size();
    storage.read_bytes(offset, &mut buf[..sz])?;

    match kind {
        PrimKind::Bool => Ok(Value::bool_(buf[0] != 0)),
        PrimKind::VariantBool => {
            let raw = read_u16(&buf[..2], bo) as i16;
            Ok(Value::bool_(raw != 0))
        }
        PrimKind::Char => Ok(Value::bytes(vec![buf[0]])),
        PrimKind::WChar => {
            let cp = if WCHAR_SIZE == 2 {
                u32::from(read_u16(&buf[..2], bo))
            } else {
                read_u32(&buf[..4], bo)
            };
            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
            Ok(Value::string(ch.to_string()))
        }
        PrimKind::Byte => Ok(Value::int(i64::from(buf[0] as i8))),
        PrimKind::UByte => Ok(Value::int(i64::from(buf[0]))),
        PrimKind::Short => Ok(Value::int(i64::from(read_u16(&buf[..2], bo) as i16))),
        PrimKind::UShort => Ok(Value::int(i64::from(read_u16(&buf[..2], bo)))),
        PrimKind::Int => Ok(Value::int(i64::from(read_u32(&buf[..4], bo) as i32))),
        PrimKind::UInt => Ok(Value::int(i64::from(read_u32(&buf[..4], bo)))),
        PrimKind::Long => {
            if sz == 4 {
                Ok(Value::int(i64::from(read_u32(&buf[..4], bo) as i32)))
            } else {
                Ok(Value::int(read_u64(&buf[..8], bo) as i64))
            }
        }
        PrimKind::ULong => {
            if sz == 4 {
                Ok(Value::int(i64::from(read_u32(&buf[..4], bo))))
            } else {
                Ok(u64_to_py_value(read_u64(&buf[..8], bo)))
            }
        }
        PrimKind::LongLong => Ok(Value::int(read_u64(&buf[..8], bo) as i64)),
        PrimKind::ULongLong => Ok(u64_to_py_value(read_u64(&buf[..8], bo))),
        PrimKind::Float => {
            let bits = read_u32(&buf[..4], bo);
            Ok(Value::float(f64::from(f32::from_bits(bits))))
        }
        PrimKind::Double => {
            let bits = read_u64(&buf[..8], bo);
            Ok(Value::float(f64::from_bits(bits)))
        }
        PrimKind::LongDouble => {
            if LONGDOUBLE_SIZE == 8 {
                let bits = read_u64(&buf[..8], bo);
                Ok(Value::float(f64::from_bits(bits)))
            } else {
                // Store/read as IEEE f64 in the low 8 bytes for portable round-trip.
                let bits = read_u64(&buf[..8], bo);
                Ok(Value::float(f64::from_bits(bits)))
            }
        }
        PrimKind::CharP => {
            let addr = read_ptr_usize(storage, offset, bo)?;
            if addr == 0 {
                Ok(Value::none())
            } else {
                Ok(Value::bytes(read_c_string(addr, None)?))
            }
        }
        PrimKind::WCharP => {
            let addr = read_ptr_usize(storage, offset, bo)?;
            if addr == 0 {
                Ok(Value::none())
            } else {
                Ok(Value::string(read_c_wstring(addr, None)?))
            }
        }
        PrimKind::VoidP => {
            let addr = read_ptr_usize(storage, offset, bo)?;
            if addr == 0 {
                Ok(Value::none())
            } else {
                Ok(u64_to_py_value(addr as u64))
            }
        }
        PrimKind::PyObject => {
            let id = read_ptr_usize(storage, offset, bo)?;
            lookup_pyobject_handle(id)
                .ok_or_else(|| PyError::named("ValueError", "PyObject is NULL"))
        }
    }
}

pub(crate) fn write_prim_value(
    interp: &mut Interpreter,
    state: &mut CDataState,
    offset: usize,
    kind: PrimKind,
    bo: ByteOrder,
    val: &Value,
    keep_key: &str,
) -> Result<()> {
    match kind {
        PrimKind::Bool => {
            let b = interp.truthy_value(val)?;
            state.storage.write_bytes(offset, &[u8::from(b)])
        }
        PrimKind::VariantBool => {
            let b = interp.truthy_value(val)?;
            let raw: i16 = if b { -1 } else { 0 };
            state
                .storage
                .write_bytes(offset, &write_u16(raw as u16, bo))
        }
        PrimKind::Char => match val.kind() {
            ValueKind::Bytes(b) => {
                if b.len() != 1 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "one character bytes, bytearray or integer expected, got bytes of length {}",
                            b.len()
                        ),
                    ));
                }
                state.storage.write_bytes(offset, &[b[0]])
            }
            ValueKind::Int(n) => {
                if !(0..=255).contains(&n) {
                    return Err(PyError::named(
                        "TypeError",
                        "one character bytes, bytearray or integer expected",
                    ));
                }
                state.storage.write_bytes(offset, &[n as u8])
            }
            _ => {
                if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
                    if snap.len() != 1 {
                        return Err(PyError::named(
                            "TypeError",
                            "one character bytes, bytearray or integer expected",
                        ));
                    }
                    return state.storage.write_bytes(offset, &[snap[0]]);
                }
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "one character bytes, bytearray or integer expected, got {}",
                        value_type_name_str(val)
                    ),
                ))
            }
        },
        PrimKind::WChar => {
            let ValueKind::Str(s) = val.kind() else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "unicode string expected instead of {} instance",
                        value_type_name_str(val)
                    ),
                ));
            };
            let mut chars = s.chars();
            let Some(ch) = chars.next() else {
                return Err(PyError::named(
                    "TypeError",
                    "one character unicode string expected",
                ));
            };
            if chars.next().is_some() {
                return Err(PyError::named(
                    "TypeError",
                    "one character unicode string expected",
                ));
            }
            if WCHAR_SIZE == 2 {
                let mut tmp = [0u16; 2];
                let enc = ch.encode_utf16(&mut tmp);
                state.storage.write_bytes(offset, &write_u16(enc[0], bo))
            } else {
                state.storage.write_bytes(offset, &write_u32(ch as u32, bo))
            }
        }
        PrimKind::Byte => {
            let v = value_to_u64_wrapping(interp, val)? as u8;
            state.storage.write_bytes(offset, &[v])
        }
        PrimKind::UByte => {
            let v = value_to_u64_wrapping(interp, val)? as u8;
            state.storage.write_bytes(offset, &[v])
        }
        PrimKind::Short | PrimKind::UShort => {
            let v = value_to_u64_wrapping(interp, val)? as u16;
            state.storage.write_bytes(offset, &write_u16(v, bo))
        }
        PrimKind::Int | PrimKind::UInt => {
            let v = value_to_u64_wrapping(interp, val)? as u32;
            state.storage.write_bytes(offset, &write_u32(v, bo))
        }
        PrimKind::Long | PrimKind::ULong => {
            let v = value_to_u64_wrapping(interp, val)?;
            if kind.size() == 4 {
                state.storage.write_bytes(offset, &write_u32(v as u32, bo))
            } else {
                state.storage.write_bytes(offset, &write_u64(v, bo))
            }
        }
        PrimKind::LongLong | PrimKind::ULongLong => {
            let v = value_to_u64_wrapping(interp, val)?;
            state.storage.write_bytes(offset, &write_u64(v, bo))
        }
        PrimKind::Float => {
            let f = value_to_f64(interp, val)? as f32;
            state
                .storage
                .write_bytes(offset, &write_u32(f.to_bits(), bo))
        }
        PrimKind::Double => {
            let f = value_to_f64(interp, val)?;
            state
                .storage
                .write_bytes(offset, &write_u64(f.to_bits(), bo))
        }
        PrimKind::LongDouble => {
            let f = value_to_f64(interp, val)?;
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&write_u64(f.to_bits(), bo));
            state.storage.write_bytes(offset, &bytes[..LONGDOUBLE_SIZE])
        }
        PrimKind::CharP => {
            if val.is_none() {
                state.keep_alive_bufs.shift_remove(keep_key);
                state.keep_alive_vals.shift_remove(keep_key);
                return write_ptr_usize(&state.storage, offset, 0, bo);
            }
            if let ValueKind::Bytes(b) = val.kind() {
                let mut owned = Vec::with_capacity(b.len() + 1);
                owned.extend_from_slice(b);
                owned.push(0);
                let boxed = owned.into_boxed_slice();
                let addr = boxed.as_ptr() as usize;
                state.keep_alive_bufs.insert(keep_key.to_string(), boxed);
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            if matches!(
                val.kind(),
                ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
            ) {
                let addr = value_to_u64_wrapping(interp, val)? as usize;
                state.keep_alive_bufs.shift_remove(keep_key);
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            // Accept c_char array (e.g. create_string_buffer) or c_char_p instance
            if let Ok((addr, keep)) = extract_char_p_compatible(val) {
                if let Some(kv) = keep {
                    state.keep_alive_vals.insert(keep_key.to_string(), kv);
                }
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "bytes or integer address expected instead of {} instance",
                    value_type_name_str(val)
                ),
            ))
        }
        PrimKind::WCharP => {
            if val.is_none() {
                state.keep_alive_bufs.shift_remove(keep_key);
                state.keep_alive_vals.shift_remove(keep_key);
                return write_ptr_usize(&state.storage, offset, 0, bo);
            }
            if let ValueKind::Str(s) = val.kind() {
                let boxed = encode_wchar_null_terminated(s);
                let addr = boxed.as_ptr() as usize;
                state.keep_alive_bufs.insert(keep_key.to_string(), boxed);
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            if matches!(
                val.kind(),
                ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
            ) {
                let addr = value_to_u64_wrapping(interp, val)? as usize;
                state.keep_alive_bufs.shift_remove(keep_key);
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            if let Ok((addr, keep)) = extract_wchar_p_compatible(val) {
                if let Some(kv) = keep {
                    state.keep_alive_vals.insert(keep_key.to_string(), kv);
                }
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "unicode string or integer address expected instead of {} instance",
                    value_type_name_str(val)
                ),
            ))
        }
        PrimKind::VoidP => {
            if val.is_none() {
                state.keep_alive_bufs.shift_remove(keep_key);
                state.keep_alive_vals.shift_remove(keep_key);
                return write_ptr_usize(&state.storage, offset, 0, bo);
            }
            if matches!(
                val.kind(),
                ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
            ) {
                let addr = value_to_u64_wrapping(interp, val)? as usize;
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            if let ValueKind::Bytes(b) = val.kind() {
                let mut owned = Vec::with_capacity(b.len() + 1);
                owned.extend_from_slice(b);
                owned.push(0);
                let boxed = owned.into_boxed_slice();
                let addr = boxed.as_ptr() as usize;
                state.keep_alive_bufs.insert(keep_key.to_string(), boxed);
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            if let Ok(addr) = extract_pointer_or_cdata_addr(val) {
                state
                    .keep_alive_vals
                    .insert(keep_key.to_string(), val.clone());
                return write_ptr_usize(&state.storage, offset, addr, bo);
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "cannot be converted to pointer: {}",
                    value_type_name_str(val)
                ),
            ))
        }
        PrimKind::PyObject => {
            let handle = alloc_pyobject_handle(val.clone());
            state
                .keep_alive_vals
                .insert(keep_key.to_string(), val.clone());
            write_ptr_usize(&state.storage, offset, handle, bo)
        }
    }
}

// ── Bitfield Read / Write ────────────────────────────────────────────────────

pub(crate) fn read_bitfield(
    storage: &CDataStorage,
    field: &FieldLayout,
    kind: PrimKind,
    bo: ByteOrder,
) -> Result<Value> {
    let bit_off = field.bit_offset.unwrap_or(0);
    let bit_w = field.bit_width.unwrap_or(1);
    let mut buf = [0u8; 8];
    storage.read_bytes(field.offset, &mut buf[..field.size])?;
    let unit: u64 = match field.size {
        1 => u64::from(buf[0]),
        2 => u64::from(read_u16(&buf[..2], bo)),
        4 => u64::from(read_u32(&buf[..4], bo)),
        8 => read_u64(&buf[..8], bo),
        _ => return Err(PyError::named("ValueError", "invalid bitfield unit size")),
    };
    let mask = if bit_w >= 64 {
        u64::MAX
    } else {
        (1u64 << bit_w) - 1
    };
    let raw = (unit >> bit_off) & mask;

    if kind == PrimKind::Bool {
        return Ok(Value::bool_(raw != 0));
    }
    if kind.is_signed_int() && bit_w < 64 {
        let sign_bit = 1u64 << (bit_w - 1);
        if (raw & sign_bit) != 0 {
            let signed = (raw | !mask) as i64;
            return Ok(Value::int(signed));
        }
    }
    Ok(u64_to_py_value(raw))
}

pub(crate) fn write_bitfield(
    interp: &mut Interpreter,
    storage: &CDataStorage,
    field: &FieldLayout,
    kind: PrimKind,
    bo: ByteOrder,
    val: &Value,
) -> Result<()> {
    let bit_off = field.bit_offset.unwrap_or(0);
    let bit_w = field.bit_width.unwrap_or(1);
    let new_bits = if kind == PrimKind::Bool {
        u64::from(interp.truthy_value(val)?)
    } else {
        value_to_u64_wrapping(interp, val)?
    };
    let mut buf = [0u8; 8];
    storage.read_bytes(field.offset, &mut buf[..field.size])?;
    let mut unit: u64 = match field.size {
        1 => u64::from(buf[0]),
        2 => u64::from(read_u16(&buf[..2], bo)),
        4 => u64::from(read_u32(&buf[..4], bo)),
        8 => read_u64(&buf[..8], bo),
        _ => return Err(PyError::named("ValueError", "invalid bitfield unit size")),
    };
    let mask = if bit_w >= 64 {
        u64::MAX
    } else {
        (1u64 << bit_w) - 1
    };
    unit = (unit & !(mask << bit_off)) | ((new_bits & mask) << bit_off);
    match field.size {
        1 => storage.write_bytes(field.offset, &[unit as u8]),
        2 => storage.write_bytes(field.offset, &write_u16(unit as u16, bo)),
        4 => storage.write_bytes(field.offset, &write_u32(unit as u32, bo)),
        8 => storage.write_bytes(field.offset, &write_u64(unit, bo)),
        _ => Err(PyError::named("ValueError", "invalid bitfield unit size")),
    }
}

// ── Address Extraction Helpers ───────────────────────────────────────────────

fn extract_char_p_compatible(val: &Value) -> Result<(usize, Option<Value>)> {
    let ValueKind::PyInstance(inst) = val.kind() else {
        return Err(PyError::named("TypeError", "not a ctypes instance"));
    };
    let cls = Rc::clone(&inst.borrow().class);
    let Some(info) = lookup_ctype(&cls) else {
        return Err(PyError::named("TypeError", "not a ctypes instance"));
    };
    match info {
        CTypeInfo::Simple {
            kind: PrimKind::CharP,
            byte_order,
        } => {
            let addr = with_native_state::<CDataState, _>(val, "_CData", |st| {
                read_ptr_usize(&st.storage, 0, byte_order)
            })?;
            Ok((addr, Some(val.clone())))
        }
        CTypeInfo::Array { elem_cls, .. } => {
            if matches!(
                lookup_ctype(&elem_cls),
                Some(CTypeInfo::Simple {
                    kind: PrimKind::Char,
                    ..
                })
            ) {
                let addr = with_native_state::<CDataState, _>(val, "_CData", |st| {
                    Ok(st.storage.address())
                })?;
                return Ok((addr, Some(val.clone())));
            }
            Err(PyError::named("TypeError", "incompatible ctypes array"))
        }
        _ => Err(PyError::named("TypeError", "incompatible ctypes type")),
    }
}

fn extract_wchar_p_compatible(val: &Value) -> Result<(usize, Option<Value>)> {
    let ValueKind::PyInstance(inst) = val.kind() else {
        return Err(PyError::named("TypeError", "not a ctypes instance"));
    };
    let cls = Rc::clone(&inst.borrow().class);
    let Some(info) = lookup_ctype(&cls) else {
        return Err(PyError::named("TypeError", "not a ctypes instance"));
    };
    match info {
        CTypeInfo::Simple {
            kind: PrimKind::WCharP,
            byte_order,
        } => {
            let addr = with_native_state::<CDataState, _>(val, "_CData", |st| {
                read_ptr_usize(&st.storage, 0, byte_order)
            })?;
            Ok((addr, Some(val.clone())))
        }
        CTypeInfo::Array { elem_cls, .. } => {
            if matches!(
                lookup_ctype(&elem_cls),
                Some(CTypeInfo::Simple {
                    kind: PrimKind::WChar,
                    ..
                })
            ) {
                let addr = with_native_state::<CDataState, _>(val, "_CData", |st| {
                    Ok(st.storage.address())
                })?;
                return Ok((addr, Some(val.clone())));
            }
            Err(PyError::named("TypeError", "incompatible ctypes array"))
        }
        _ => Err(PyError::named("TypeError", "incompatible ctypes type")),
    }
}

/// Extract a raw pointer value from a `_CData` (pointer/void_p/char_p/wchar_p/array/funcptr)
/// or `CArgObject` (`byref`).
pub(crate) fn extract_pointer_or_cdata_addr(val: &Value) -> Result<usize> {
    if let Ok(addr) = with_native_state::<CArgObjectState, _>(val, "CArgObject", |st| Ok(st.addr)) {
        return Ok(addr);
    }
    let ValueKind::PyInstance(inst) = val.kind() else {
        return Err(PyError::named(
            "TypeError",
            "not a pointer or ctypes instance",
        ));
    };
    let cls = Rc::clone(&inst.borrow().class);
    let info =
        lookup_ctype(&cls).ok_or_else(|| PyError::named("TypeError", "not a ctypes instance"))?;
    with_native_state::<CDataState, _>(val, "_CData", |st| match info {
        CTypeInfo::Pointer { .. } | CTypeInfo::FuncPtr { .. } => {
            read_ptr_usize(&st.storage, 0, ByteOrder::Native)
        }
        CTypeInfo::Simple {
            kind: PrimKind::VoidP | PrimKind::CharP | PrimKind::WCharP,
            byte_order,
        } => read_ptr_usize(&st.storage, 0, byte_order),
        _ => Ok(st.storage.address()),
    })
}

/// Extract a memory address for `memmove`, `memset`, `string_at`, `wstring_at`.
/// Returns `(addr, optional_keep_alive_bytes)` so temporary `bytes` buffers stay alive.
pub(crate) fn extract_memory_address(
    interp: &mut Interpreter,
    val: &Value,
) -> Result<(usize, Option<Box<[u8]>>)> {
    if val.is_none() {
        return Ok((0, None));
    }
    if matches!(
        val.kind(),
        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
    ) {
        let a = value_to_u64_wrapping(interp, val)? as usize;
        return Ok((a, None));
    }
    if let ValueKind::Bytes(b) = val.kind() {
        let mut owned = Vec::with_capacity(b.len() + 1);
        owned.extend_from_slice(b);
        owned.push(0);
        let boxed = owned.into_boxed_slice();
        let addr = boxed.as_ptr() as usize;
        return Ok((addr, Some(boxed)));
    }
    if let Some((buf_rc, _)) = pyrust_builtins::bytearray::as_bytearray_buffer(val) {
        let addr = buf_rc.borrow_mut().as_mut_ptr() as usize;
        return Ok((addr, None));
    }
    if let Ok(addr) = extract_pointer_or_cdata_addr(val) {
        return Ok((addr, None));
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "cannot convert '{}' object to memory address",
            value_type_name_str(val)
        ),
    ))
}

// ── Dynamic Class Creation & Layout Finalization ─────────────────────────────

pub(crate) fn create_cdata_instance(
    cls: Rc<RefCell<PyClass>>,
    storage: CDataStorage,
) -> Result<Value> {
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: cls,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(&inst, CDataState::new(storage))?;
    Ok(inst)
}

pub(crate) fn create_carg_instance(
    obj: Value,
    offset: isize,
    addr: usize,
    tag: String,
) -> Result<Value> {
    let core = core_classes()?;
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: core.carg_cls,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        CArgObjectState {
            obj,
            _offset: offset,
            addr,
            tag,
        },
    )?;
    Ok(inst)
}

pub(crate) fn create_cfield_descriptor(field: FieldLayout, owner_swapped: bool) -> Result<Value> {
    let core = core_classes()?;
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: core.cfield_cls,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        CFieldState {
            field,
            owner_swapped,
        },
    )?;
    Ok(inst)
}

/// Ensure a Structure or Union subclass has its C layout computed and field
/// descriptors installed. Handles both class-body `_fields_` and deferred
/// `Cls._fields_ = [...]` definitions.
pub(crate) fn ensure_struct_layout(
    interp: &mut Interpreter,
    cls: &Rc<RefCell<PyClass>>,
) -> Result<StructLayout> {
    let core = core_classes()?;
    let is_union = class_is_subclass_of(cls, &core.union_base);

    // Check if already finalized
    if let Some(CTypeInfo::Struct(layout)) = lookup_ctype_exact(cls)
        && layout.finalized
    {
        return Ok(layout);
    }

    // Check `_swappedbytes_` anywhere in MRO (e.g. BigEndianStructure)
    let swapped_bytes = crate::interpreter::lookup_class_attr(cls, "_swappedbytes_").is_some();

    // Base struct layout if inheriting from a concrete Structure/Union subclass
    let base_layout = {
        let base_opt = cls.borrow().base.clone();
        if let Some(base) = base_opt {
            if !Rc::ptr_eq(&base, &core.struct_base)
                && !Rc::ptr_eq(&base, &core.union_base)
                && (class_is_subclass_of(&base, &core.struct_base)
                    || class_is_subclass_of(&base, &core.union_base))
            {
                Some(ensure_struct_layout(interp, &base)?)
            } else {
                None
            }
        } else {
            None
        }
    };

    let own_fields = cls.borrow().attrs.get("_fields_").cloned();
    let Some(fields_val) = own_fields else {
        // No `_fields_` on this class yet — inherit base layout (or empty layout)
        let layout = base_layout.unwrap_or_else(|| StructLayout::empty(is_union, swapped_bytes));
        register_ctype(cls, CTypeInfo::Struct(layout.clone()));
        return Ok(layout);
    };

    finalize_struct_fields(interp, cls, &fields_val)
}

/// Validate `_fields_` on a Structure/Union subclass, compute C layout, install
/// `CField` descriptors (including anonymous subfield descriptors), and freeze.
pub(crate) fn finalize_struct_fields(
    interp: &mut Interpreter,
    cls: &Rc<RefCell<PyClass>>,
    fields_val: &Value,
) -> Result<StructLayout> {
    let core = core_classes()?;
    let is_union = class_is_subclass_of(cls, &core.union_base);

    if let Some(CTypeInfo::Struct(existing)) = lookup_ctype_exact(cls)
        && existing.finalized
    {
        return Err(PyError::named("AttributeError", "_fields_ is final"));
    }

    let swapped_bytes = crate::interpreter::lookup_class_attr(cls, "_swappedbytes_").is_some();
    let pack = parse_pack_attr(cls)?;

    let base_layout = {
        let base_opt = cls.borrow().base.clone();
        if let Some(base) = base_opt {
            if !Rc::ptr_eq(&base, &core.struct_base)
                && !Rc::ptr_eq(&base, &core.union_base)
                && (class_is_subclass_of(&base, &core.struct_base)
                    || class_is_subclass_of(&base, &core.union_base))
            {
                Some(ensure_struct_layout(interp, &base)?)
            } else {
                None
            }
        } else {
            None
        }
    };

    let anonymous: Vec<String> =
        if let Some(anon_val) = cls.borrow().attrs.get("_anonymous_").cloned() {
            let items = interp.collect_iterable(&anon_val)?;
            let mut names = Vec::with_capacity(items.len());
            for it in items {
                match it.kind() {
                    ValueKind::Str(s) => names.push(s.to_string()),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "_anonymous_ items must be strings",
                        ));
                    }
                }
            }
            names
        } else {
            Vec::new()
        };

    let layout = compute_struct_layout(
        interp,
        is_union,
        swapped_bytes,
        pack,
        base_layout.as_ref(),
        fields_val,
        anonymous.clone(),
    )?;

    // Install CField descriptors for every field (and anonymous subfields)
    let mut descriptors: Vec<(String, Value)> = Vec::new();
    for f in &layout.fields {
        descriptors.push((
            f.name.clone(),
            create_cfield_descriptor(f.clone(), swapped_bytes)?,
        ));
    }
    for anon_name in &anonymous {
        let Some(anon_field) = layout.fields.iter().find(|f| f.name == *anon_name) else {
            return Err(PyError::named(
                "AttributeError",
                format!(
                    "type object '{}' has no attribute '{anon_name}'",
                    cls.borrow().name
                ),
            ));
        };
        if let Ok(CTypeInfo::Struct(sub_layout)) = ensure_ctype_info(interp, &anon_field.field_cls)
        {
            for sf in &sub_layout.fields {
                let mut promoted = sf.clone();
                promoted.offset += anon_field.offset;
                descriptors.push((
                    sf.name.clone(),
                    create_cfield_descriptor(promoted, swapped_bytes || sub_layout.swapped_bytes)?,
                ));
            }
        }
    }

    {
        let mut cls_mut = cls.borrow_mut();
        cls_mut
            .attrs
            .insert("_fields_".to_string(), fields_val.clone());
        for (name, desc) in descriptors {
            cls_mut.attrs.insert(name, desc);
        }
        cls_mut.bump_mutation_version();
    }
    pyrust_core::bump_class_epoch();

    register_ctype(cls, CTypeInfo::Struct(layout.clone()));
    Ok(layout)
}

/// Ensure any ctypes class has a resolved `CTypeInfo`.
pub(crate) fn ensure_ctype_info(
    interp: &mut Interpreter,
    cls: &Rc<RefCell<PyClass>>,
) -> Result<CTypeInfo> {
    let core = core_classes()?;
    if class_is_subclass_of(cls, &core.struct_base) || class_is_subclass_of(cls, &core.union_base) {
        if Rc::ptr_eq(cls, &core.struct_base) || Rc::ptr_eq(cls, &core.union_base) {
            return Ok(CTypeInfo::Struct(StructLayout::empty(
                Rc::ptr_eq(cls, &core.union_base),
                false,
            )));
        }
        let layout = ensure_struct_layout(interp, cls)?;
        return Ok(CTypeInfo::Struct(layout));
    }
    if let Some(info) = lookup_ctype(cls) {
        return Ok(info);
    }
    Err(PyError::named(
        "TypeError",
        format!("must be a ctypes type, not '{}'", cls.borrow().name),
    ))
}

/// Return the opposite-endian version of `cls` (`__ctype_be__` on LE, `__ctype_le__` on BE).
pub(crate) fn other_endian_type(
    interp: &mut Interpreter,
    cls: &Rc<RefCell<PyClass>>,
) -> Result<Rc<RefCell<PyClass>>> {
    let info = ensure_ctype_info(interp, cls)?;
    match info {
        CTypeInfo::Simple { kind, byte_order } => {
            if !kind.supports_byte_swap() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "This type does not support other endian: {}",
                        cls.borrow().name
                    ),
                ));
            }
            if kind.size() <= 1 {
                return Ok(Rc::clone(cls));
            }
            if let Some(cached) = lookup_swapped_cache(cls) {
                return Ok(cached);
            }
            let new_bo = if byte_order.is_swapped() {
                ByteOrder::native_endian()
            } else {
                ByteOrder::other_endian()
            };
            let suffix = match new_bo {
                ByteOrder::Big => "_be",
                ByteOrder::Little => "_le",
                ByteOrder::Native => "",
            };
            let orig_name = cls.borrow().name.clone();
            let base_name = orig_name
                .strip_suffix("_be")
                .or_else(|| orig_name.strip_suffix("_le"))
                .unwrap_or(&orig_name);
            let new_name = format!("{base_name}{suffix}");
            let core = core_classes()?;
            let mut attrs = IndexMap::new();
            attrs.insert("__module__".to_string(), Value::string("ctypes"));
            attrs.insert("_type_".to_string(), Value::string(kind.code()));
            let mut new_cls = PyClass::new(
                &new_name,
                &new_name,
                Some(Rc::clone(&core.simple_base)),
                attrs,
            );
            new_cls.metatype = Some(Rc::clone(&core.simple_meta));
            let new_rc = Rc::new(RefCell::new(new_cls));
            core.simple_base
                .borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&new_rc));

            register_ctype(
                &new_rc,
                CTypeInfo::Simple {
                    kind,
                    byte_order: new_bo,
                },
            );
            insert_swapped_cache(cls, Rc::clone(&new_rc));
            insert_swapped_cache(&new_rc, Rc::clone(cls));

            // Wire `__ctype_be__` and `__ctype_le__` on the swapped class
            {
                let mut m = new_rc.borrow_mut();
                match new_bo {
                    ByteOrder::Big => {
                        m.attrs.insert(
                            "__ctype_be__".to_string(),
                            Value::py_class(Rc::clone(&new_rc)),
                        );
                        m.attrs
                            .insert("__ctype_le__".to_string(), Value::py_class(Rc::clone(cls)));
                    }
                    ByteOrder::Little | ByteOrder::Native => {
                        m.attrs.insert(
                            "__ctype_le__".to_string(),
                            Value::py_class(Rc::clone(&new_rc)),
                        );
                        m.attrs
                            .insert("__ctype_be__".to_string(), Value::py_class(Rc::clone(cls)));
                    }
                }
            }
            Ok(new_rc)
        }
        CTypeInfo::Array {
            elem_cls, length, ..
        } => {
            let swapped_elem = other_endian_type(interp, &elem_cls)?;
            make_array_type(interp, &swapped_elem, length)
        }
        CTypeInfo::Struct(_) => Ok(Rc::clone(cls)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "This type does not support other endian: {}",
                cls.borrow().name
            ),
        )),
    }
}

/// Dynamically create or return cached `c_type * length` Array subclass.
pub(crate) fn make_array_type(
    interp: &mut Interpreter,
    elem_cls: &Rc<RefCell<PyClass>>,
    length: usize,
) -> Result<Rc<RefCell<PyClass>>> {
    if let Some(cached) = lookup_array_cache(elem_cls, length) {
        return Ok(cached);
    }
    let elem_info = ensure_ctype_info(interp, elem_cls)?;
    let elem_size = elem_info.size();
    let elem_align = elem_info.align();
    let core = core_classes()?;

    let elem_name = elem_cls.borrow().name.clone();
    let arr_name = format!("{elem_name}_Array_{length}");

    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("ctypes"));
    attrs.insert("_type_".to_string(), Value::py_class(Rc::clone(elem_cls)));
    attrs.insert("_length_".to_string(), Value::int(length as i64));

    let mut cls = PyClass::new(
        &arr_name,
        &arr_name,
        Some(Rc::clone(&core.array_base)),
        attrs,
    );
    cls.metatype = Some(Rc::clone(&core.array_meta));
    let cls_rc = Rc::new(RefCell::new(cls));
    core.array_base
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&cls_rc));

    register_ctype(
        &cls_rc,
        CTypeInfo::Array {
            elem_cls: Rc::clone(elem_cls),
            length,
            elem_size,
            elem_align,
        },
    );
    insert_array_cache(elem_cls, length, Rc::clone(&cls_rc));
    Ok(cls_rc)
}

/// Dynamically create or return cached `POINTER(target_cls)` subclass.
pub(crate) fn make_pointer_type(
    interp: &mut Interpreter,
    target_cls: Option<&Rc<RefCell<PyClass>>>,
) -> Result<Rc<RefCell<PyClass>>> {
    let core = core_classes()?;
    if let Some(tcls) = target_cls {
        // Validate `tcls` is a ctypes type (note: incomplete Structure without `_fields_` is allowed!)
        let is_ctype = class_is_subclass_of(tcls, &core.cdata_base);
        if !is_ctype {
            return Err(PyError::named(
                "TypeError",
                format!("must have storage info: {}", tcls.borrow().name),
            ));
        }
        // Only finalize non-struct or already-defined types; forward-declared structs stay lazy!
        if !class_is_subclass_of(tcls, &core.struct_base)
            && !class_is_subclass_of(tcls, &core.union_base)
        {
            let _ = ensure_ctype_info(interp, tcls)?;
        }
        if let Some(cached) = lookup_pointer_cache(tcls) {
            return Ok(cached);
        }
        let target_name = tcls.borrow().name.clone();
        let ptr_name = format!("LP_{target_name}");
        let mut attrs = IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("ctypes"));
        attrs.insert("_type_".to_string(), Value::py_class(Rc::clone(tcls)));
        let mut cls = PyClass::new(
            &ptr_name,
            &ptr_name,
            Some(Rc::clone(&core.pointer_base)),
            attrs,
        );
        cls.metatype = Some(Rc::clone(&core.pointer_meta));
        let cls_rc = Rc::new(RefCell::new(cls));
        core.pointer_base
            .borrow()
            .subclasses
            .borrow_mut()
            .push(Rc::downgrade(&cls_rc));

        register_ctype(
            &cls_rc,
            CTypeInfo::Pointer {
                target_cls: Some(Rc::clone(tcls)),
            },
        );
        insert_pointer_cache(tcls, Rc::clone(&cls_rc));
        Ok(cls_rc)
    } else {
        // Incomplete pointer `POINTER(None)` / `POINTER("name")`
        let mut attrs = IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("ctypes"));
        let mut cls = PyClass::new(
            "LP_None",
            "LP_None",
            Some(Rc::clone(&core.pointer_base)),
            attrs,
        );
        cls.metatype = Some(Rc::clone(&core.pointer_meta));
        let cls_rc = Rc::new(RefCell::new(cls));
        core.pointer_base
            .borrow()
            .subclasses
            .borrow_mut()
            .push(Rc::downgrade(&cls_rc));
        register_ctype(&cls_rc, CTypeInfo::Pointer { target_cls: None });
        Ok(cls_rc)
    }
}

/// Create a `CFUNCTYPE` / `WINFUNCTYPE` / `PYFUNCTYPE` subclass of `CFuncPtr`.
pub(crate) fn make_functype_class(
    restype: Option<Value>,
    argtypes: Vec<Value>,
    flags: i32,
) -> Result<Rc<RefCell<PyClass>>> {
    let core = core_classes()?;
    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("ctypes"));
    attrs.insert(
        "_restype_".to_string(),
        restype.clone().unwrap_or_else(Value::none),
    );
    attrs.insert("_argtypes_".to_string(), Value::tuple(argtypes.clone()));
    attrs.insert("_flags_".to_string(), Value::int(i64::from(flags)));

    let name = if (flags & FUNCFLAG_CDECL) != 0 {
        "CFunctionType"
    } else {
        "WinFunctionType"
    };
    let mut cls = PyClass::new(name, name, Some(Rc::clone(&core.funcptr_base)), attrs);
    cls.metatype = Some(Rc::clone(&core.funcptr_meta));
    let cls_rc = Rc::new(RefCell::new(cls));
    core.funcptr_base
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&cls_rc));

    register_ctype(
        &cls_rc,
        CTypeInfo::FuncPtr {
            restype,
            argtypes: Some(argtypes),
            flags,
        },
    );
    Ok(cls_rc)
}

// ── Compound Read / Write (Fields & Array Elements) ──────────────────────────

/// Read a value of type `target_cls` at `offset` inside `parent_storage`.
/// Matches CPython semantics:
/// - Simple C types return their unwrapped Python value (`int`, `float`, `bytes`, `str`, `bool`, `None`).
/// - Compound types (`Structure`, `Union`, `Array`, `Pointer`, `FuncPtr`) return a live view instance.
pub(crate) fn read_typed_at(
    interp: &mut Interpreter,
    parent_storage: &CDataStorage,
    owner: Option<Value>,
    offset: usize,
    target_cls: &Rc<RefCell<PyClass>>,
    bitfield: Option<&FieldLayout>,
    owner_swapped: bool,
) -> Result<Value> {
    let info = ensure_ctype_info(interp, target_cls)?;
    match info {
        CTypeInfo::Simple { kind, byte_order } => {
            let eff_bo = if owner_swapped && byte_order == ByteOrder::Native {
                ByteOrder::other_endian()
            } else {
                byte_order
            };
            if let Some(bf) = bitfield
                && bf.bit_width.is_some()
            {
                return read_bitfield(parent_storage, bf, kind, eff_bo);
            }
            read_prim_value(parent_storage, offset, kind, eff_bo)
        }
        CTypeInfo::Array {
            elem_cls,
            length,
            elem_size,
            ..
        } => {
            // `c_char * n` field inside a Structure returns `bytes` (up to NUL) in CPython!
            // `c_wchar * n` field inside a Structure returns `str` (up to NUL) in CPython!
            if let Some(bf) = bitfield
                && bf.bit_width.is_none()
            {
                // This is a struct field access!
                if let Some(CTypeInfo::Simple {
                    kind: PrimKind::Char,
                    ..
                }) = lookup_ctype(&elem_cls)
                {
                    let mut raw = vec![0u8; length];
                    parent_storage.read_bytes(offset, &mut raw)?;
                    let nul = raw.iter().position(|&b| b == 0).unwrap_or(length);
                    return Ok(Value::bytes(raw[..nul].to_vec()));
                }
                if let Some(CTypeInfo::Simple {
                    kind: PrimKind::WChar,
                    ..
                }) = lookup_ctype(&elem_cls)
                {
                    let sub = parent_storage.subview(offset, length * WCHAR_SIZE, owner);
                    let s = read_wchar_array_value(&sub, length, ByteOrder::Native)?;
                    return Ok(Value::string(s));
                }
            }
            let sub = parent_storage.subview(offset, length * elem_size, owner);
            create_cdata_instance(Rc::clone(target_cls), sub)
        }
        CTypeInfo::Struct(ref layout) => {
            let sub = parent_storage.subview(offset, layout.size, owner);
            create_cdata_instance(Rc::clone(target_cls), sub)
        }
        CTypeInfo::Pointer { .. } | CTypeInfo::FuncPtr { .. } => {
            let sub = parent_storage.subview(offset, PTR_SIZE, owner);
            create_cdata_instance(Rc::clone(target_cls), sub)
        }
    }
}

/// Write a Python value into a typed slot at `offset` inside `parent_state`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_typed_at(
    interp: &mut Interpreter,
    parent_state: &mut CDataState,
    offset: usize,
    target_cls: &Rc<RefCell<PyClass>>,
    val: &Value,
    bitfield: Option<&FieldLayout>,
    owner_swapped: bool,
    keep_key: &str,
) -> Result<()> {
    let info = ensure_ctype_info(interp, target_cls)?;
    match info {
        CTypeInfo::Simple { kind, byte_order } => {
            let eff_bo = if owner_swapped && byte_order == ByteOrder::Native {
                ByteOrder::other_endian()
            } else {
                byte_order
            };
            // Unwrap if caller passed an instance of the same Simple C type (e.g. `c_int(5)`)
            let unwrapped = unwrap_simple_if_cdata(val, kind)?;
            let effective_val = unwrapped.as_ref().unwrap_or(val);
            if let Some(bf) = bitfield
                && bf.bit_width.is_some()
            {
                return write_bitfield(
                    interp,
                    &parent_state.storage,
                    bf,
                    kind,
                    eff_bo,
                    effective_val,
                );
            }
            write_prim_value(
                interp,
                parent_state,
                offset,
                kind,
                eff_bo,
                effective_val,
                keep_key,
            )
        }
        CTypeInfo::Array {
            elem_cls,
            length,
            elem_size,
            ..
        } => {
            // Allow assigning `bytes` directly to a `c_char * n` field
            if let ValueKind::Bytes(b) = val.kind()
                && matches!(
                    lookup_ctype(&elem_cls),
                    Some(CTypeInfo::Simple {
                        kind: PrimKind::Char,
                        ..
                    })
                )
            {
                if b.len() > length {
                    return Err(PyError::named(
                        "ValueError",
                        format!("bytes too long ({}, maximum length {})", b.len(), length),
                    ));
                }
                let mut padded = vec![0u8; length];
                padded[..b.len()].copy_from_slice(b);
                return parent_state.storage.write_bytes(offset, &padded);
            }
            // Allow assigning `str` directly to a `c_wchar * n` field
            if let ValueKind::Str(s) = val.kind()
                && matches!(
                    lookup_ctype(&elem_cls),
                    Some(CTypeInfo::Simple {
                        kind: PrimKind::WChar,
                        ..
                    })
                )
            {
                return write_wchar_array_value(
                    &parent_state.storage,
                    offset,
                    length,
                    s,
                    ByteOrder::Native,
                );
            }
            // Allow assigning another matching Array instance directly
            if let ValueKind::PyInstance(inst) = val.kind()
                && class_is_subclass_of(&inst.borrow().class, target_cls)
            {
                let src_bytes =
                    with_native_state::<CDataState, _>(val, "_CData", |st| st.storage.read_all())?;
                return parent_state.storage.write_bytes(offset, &src_bytes);
            }
            // Allow assigning a tuple/list of element initializers
            let items = interp.collect_iterable(val)?;
            if items.len() > length {
                return Err(PyError::named(
                    "IndexError",
                    format!("too many initializers ({} > {})", items.len(), length),
                ));
            }
            for (i, item) in items.iter().enumerate() {
                let elem_key = format!("{keep_key}[{i}]");
                write_typed_at(
                    interp,
                    parent_state,
                    offset + i * elem_size,
                    &elem_cls,
                    item,
                    None,
                    owner_swapped,
                    &elem_key,
                )?;
            }
            Ok(())
        }
        CTypeInfo::Struct(ref layout) => {
            if let ValueKind::PyInstance(inst) = val.kind()
                && class_is_subclass_of(&inst.borrow().class, target_cls)
            {
                let src_bytes =
                    with_native_state::<CDataState, _>(val, "_CData", |st| st.storage.read_all())?;
                parent_state
                    .keep_alive_vals
                    .insert(keep_key.to_string(), val.clone());
                return parent_state.storage.write_bytes(offset, &src_bytes);
            }
            // Tuple initializer for nested struct (e.g. `Outer((1, 2))`)
            if matches!(val.kind(), ValueKind::Tuple(_)) {
                let items = interp.collect_iterable(val)?;
                if items.len() > layout.fields.len() {
                    return Err(PyError::named("TypeError", "too many initializers"));
                }
                for (i, item) in items.iter().enumerate() {
                    let f = &layout.fields[i];
                    let fkey = format!("{keep_key}.{}", f.name);
                    write_typed_at(
                        interp,
                        parent_state,
                        offset + f.offset,
                        &f.field_cls,
                        item,
                        Some(f),
                        layout.swapped_bytes,
                        &fkey,
                    )?;
                }
                return Ok(());
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "expected {} instance instead of {}",
                    target_cls.borrow().name,
                    value_type_name_str(val)
                ),
            ))
        }
        CTypeInfo::Pointer { target_cls: ref pt } => {
            if val.is_none() {
                parent_state.keep_alive_vals.shift_remove(keep_key);
                return write_ptr_usize(&parent_state.storage, offset, 0, ByteOrder::Native);
            }
            // Exact matching pointer type or compatible array / pointer
            if let ValueKind::PyInstance(inst) = val.kind() {
                let vcls = Rc::clone(&inst.borrow().class);
                if class_is_subclass_of(&vcls, target_cls) {
                    let addr = with_native_state::<CDataState, _>(val, "_CData", |st| {
                        read_ptr_usize(&st.storage, 0, ByteOrder::Native)
                    })?;
                    parent_state
                        .keep_alive_vals
                        .insert(keep_key.to_string(), val.clone());
                    return write_ptr_usize(&parent_state.storage, offset, addr, ByteOrder::Native);
                }
                // Array of T decays to POINTER(T)
                if let (Some(expected_t), Some(CTypeInfo::Array { elem_cls, .. })) =
                    (pt, lookup_ctype(&vcls))
                    && Rc::ptr_eq(expected_t, &elem_cls)
                {
                    let addr = with_native_state::<CDataState, _>(val, "_CData", |st| {
                        Ok(st.storage.address())
                    })?;
                    parent_state
                        .keep_alive_vals
                        .insert(keep_key.to_string(), val.clone());
                    return write_ptr_usize(&parent_state.storage, offset, addr, ByteOrder::Native);
                }
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "expected {} instance instead of {}",
                    target_cls.borrow().name,
                    value_type_name_str(val)
                ),
            ))
        }
        CTypeInfo::FuncPtr { .. } => {
            if val.is_none() {
                parent_state.keep_alive_vals.shift_remove(keep_key);
                return write_ptr_usize(&parent_state.storage, offset, 0, ByteOrder::Native);
            }
            if let Ok(addr) = extract_pointer_or_cdata_addr(val) {
                parent_state
                    .keep_alive_vals
                    .insert(keep_key.to_string(), val.clone());
                return write_ptr_usize(&parent_state.storage, offset, addr, ByteOrder::Native);
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "expected {} instance instead of {}",
                    target_cls.borrow().name,
                    value_type_name_str(val)
                ),
            ))
        }
    }
}

fn unwrap_simple_if_cdata(val: &Value, expected_kind: PrimKind) -> Result<Option<Value>> {
    let ValueKind::PyInstance(inst) = val.kind() else {
        return Ok(None);
    };
    let cls = Rc::clone(&inst.borrow().class);
    let Some(CTypeInfo::Simple { kind, byte_order }) = lookup_ctype(&cls) else {
        return Ok(None);
    };
    if kind == expected_kind
        || (kind.is_signed_int() || kind.is_unsigned_int())
            && (expected_kind.is_signed_int() || expected_kind.is_unsigned_int())
    {
        let v = with_native_state::<CDataState, _>(val, "_CData", |st| {
            read_prim_value(&st.storage, 0, kind, byte_order)
        })?;
        return Ok(Some(v));
    }
    Ok(None)
}

pub(crate) fn read_wchar_array_value(
    storage: &CDataStorage,
    length: usize,
    bo: ByteOrder,
) -> Result<String> {
    let mut raw = vec![0u8; length * WCHAR_SIZE];
    storage.read_bytes(0, &mut raw)?;
    if WCHAR_SIZE == 2 {
        let mut units = Vec::with_capacity(length);
        for i in 0..length {
            let u = read_u16(&raw[i * 2..i * 2 + 2], bo);
            if u == 0 {
                break;
            }
            units.push(u);
        }
        Ok(String::from_utf16_lossy(&units))
    } else {
        let mut out = String::new();
        for i in 0..length {
            let cp = read_u32(&raw[i * 4..i * 4 + 4], bo);
            if cp == 0 {
                break;
            }
            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
        }
        Ok(out)
    }
}

pub(crate) fn write_wchar_array_value(
    storage: &CDataStorage,
    offset: usize,
    length: usize,
    s: &str,
    bo: ByteOrder,
) -> Result<()> {
    if WCHAR_SIZE == 2 {
        let units: Vec<u16> = s.encode_utf16().collect();
        if units.len() > length {
            return Err(PyError::named(
                "ValueError",
                format!(
                    "string too long ({}, maximum length {})",
                    units.len(),
                    length
                ),
            ));
        }
        let mut raw = vec![0u8; length * 2];
        for (i, &u) in units.iter().enumerate() {
            raw[i * 2..i * 2 + 2].copy_from_slice(&write_u16(u, bo));
        }
        storage.write_bytes(offset, &raw)
    } else {
        let chars: Vec<u32> = s.chars().map(|c| c as u32).collect();
        if chars.len() > length {
            return Err(PyError::named(
                "ValueError",
                format!(
                    "string too long ({}, maximum length {})",
                    chars.len(),
                    length
                ),
            ));
        }
        let mut raw = vec![0u8; length * 4];
        for (i, &cp) in chars.iter().enumerate() {
            raw[i * 4..i * 4 + 4].copy_from_slice(&write_u32(cp, bo));
        }
        storage.write_bytes(offset, &raw)
    }
}

// ── FFI Argument & Return Value Marshaling ───────────────────────────────────

/// Convert a Python argument into an [`AbiVal`] according to an optional declared
/// C `argtype`, keeping any temporary allocations alive in `temps`.
pub(crate) fn marshal_ffi_arg(
    interp: &mut Interpreter,
    val: &Value,
    argtype: Option<&Value>,
    temps: &mut Vec<Box<[u8]>>,
) -> Result<AbiVal> {
    if let Some(at) = argtype {
        // Support `argtype.from_param(val)` if custom type defines it? First handle ctypes classes.
        if let ValueKind::PyClass(at_cls) = at.kind() {
            let info = ensure_ctype_info(interp, at_cls)?;
            return marshal_arg_with_type(interp, val, at_cls, &info, temps);
        }
    }
    marshal_arg_untyped(interp, val, temps)
}

fn marshal_arg_with_type(
    interp: &mut Interpreter,
    val: &Value,
    at_cls: &Rc<RefCell<PyClass>>,
    info: &CTypeInfo,
    temps: &mut Vec<Box<[u8]>>,
) -> Result<AbiVal> {
    match info {
        CTypeInfo::Simple { kind, .. } => {
            // If `val` is already an instance of a simple C type, read its value first
            let unwrapped = unwrap_simple_if_cdata(val, *kind)?;
            let v = unwrapped.as_ref().unwrap_or(val);
            match kind {
                PrimKind::Bool => Ok(AbiVal::Int(u64::from(interp.truthy_value(v)?))),
                PrimKind::VariantBool => {
                    let b = interp.truthy_value(v)?;
                    Ok(AbiVal::Int(if b { (-1i16) as i64 as u64 } else { 0 }))
                }
                PrimKind::Char => match v.kind() {
                    ValueKind::Bytes(b) if b.len() == 1 => Ok(AbiVal::Int(u64::from(b[0]))),
                    ValueKind::Int(n) => Ok(AbiVal::Int((n as u8) as u64)),
                    _ => Err(PyError::named(
                        "TypeError",
                        "one character bytes, bytearray or integer expected",
                    )),
                },
                PrimKind::WChar => {
                    if let ValueKind::Str(s) = v.kind()
                        && let Some(ch) = s.chars().next()
                        && s.chars().count() == 1
                    {
                        return Ok(AbiVal::Int(u64::from(ch as u32)));
                    }
                    Err(PyError::named(
                        "TypeError",
                        "one character unicode string expected",
                    ))
                }
                PrimKind::Byte => Ok(AbiVal::Int(
                    i64::from(value_to_u64_wrapping(interp, v)? as i8) as u64,
                )),
                PrimKind::UByte => Ok(AbiVal::Int(u64::from(
                    value_to_u64_wrapping(interp, v)? as u8
                ))),
                PrimKind::Short => Ok(AbiVal::Int(
                    i64::from(value_to_u64_wrapping(interp, v)? as i16) as u64,
                )),
                PrimKind::UShort => Ok(AbiVal::Int(u64::from(
                    value_to_u64_wrapping(interp, v)? as u16
                ))),
                PrimKind::Int => Ok(AbiVal::Int(
                    i64::from(value_to_u64_wrapping(interp, v)? as i32) as u64,
                )),
                PrimKind::UInt => Ok(AbiVal::Int(u64::from(
                    value_to_u64_wrapping(interp, v)? as u32
                ))),
                PrimKind::Long => {
                    let raw = value_to_u64_wrapping(interp, v)?;
                    if kind.size() == 4 {
                        Ok(AbiVal::Int(i64::from(raw as i32) as u64))
                    } else {
                        Ok(AbiVal::Int(raw))
                    }
                }
                PrimKind::ULong => {
                    let raw = value_to_u64_wrapping(interp, v)?;
                    if kind.size() == 4 {
                        Ok(AbiVal::Int(u64::from(raw as u32)))
                    } else {
                        Ok(AbiVal::Int(raw))
                    }
                }
                PrimKind::LongLong | PrimKind::ULongLong => {
                    Ok(AbiVal::Int(value_to_u64_wrapping(interp, v)?))
                }
                PrimKind::Float => Ok(AbiVal::F32(value_to_f64(interp, v)? as f32)),
                PrimKind::Double | PrimKind::LongDouble => {
                    Ok(AbiVal::F64(value_to_f64(interp, v)?))
                }
                PrimKind::CharP => {
                    if v.is_none() {
                        return Ok(AbiVal::Int(0));
                    }
                    if let ValueKind::Bytes(b) = v.kind() {
                        let mut owned = Vec::with_capacity(b.len() + 1);
                        owned.extend_from_slice(b);
                        owned.push(0);
                        let boxed = owned.into_boxed_slice();
                        let addr = boxed.as_ptr() as usize as u64;
                        temps.push(boxed);
                        return Ok(AbiVal::Int(addr));
                    }
                    if matches!(v.kind(), ValueKind::Int(_) | ValueKind::BigInt(_)) {
                        return Ok(AbiVal::Int(value_to_u64_wrapping(interp, v)?));
                    }
                    if let Ok((addr, _)) = extract_char_p_compatible(v) {
                        return Ok(AbiVal::Int(addr as u64));
                    }
                    if let Ok(addr) = extract_pointer_or_cdata_addr(v) {
                        return Ok(AbiVal::Int(addr as u64));
                    }
                    Err(PyError::named(
                        "ArgumentError",
                        format!(
                            "wrong type: expected c_char_p, got {}",
                            value_type_name_str(v)
                        ),
                    ))
                }
                PrimKind::WCharP => {
                    if v.is_none() {
                        return Ok(AbiVal::Int(0));
                    }
                    if let ValueKind::Str(s) = v.kind() {
                        let boxed = encode_wchar_null_terminated(s);
                        let addr = boxed.as_ptr() as usize as u64;
                        temps.push(boxed);
                        return Ok(AbiVal::Int(addr));
                    }
                    if matches!(v.kind(), ValueKind::Int(_) | ValueKind::BigInt(_)) {
                        return Ok(AbiVal::Int(value_to_u64_wrapping(interp, v)?));
                    }
                    if let Ok((addr, _)) = extract_wchar_p_compatible(v) {
                        return Ok(AbiVal::Int(addr as u64));
                    }
                    if let Ok(addr) = extract_pointer_or_cdata_addr(v) {
                        return Ok(AbiVal::Int(addr as u64));
                    }
                    Err(PyError::named(
                        "ArgumentError",
                        format!(
                            "wrong type: expected c_wchar_p, got {}",
                            value_type_name_str(v)
                        ),
                    ))
                }
                PrimKind::VoidP => {
                    if v.is_none() {
                        return Ok(AbiVal::Int(0));
                    }
                    if matches!(
                        v.kind(),
                        ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
                    ) {
                        return Ok(AbiVal::Int(value_to_u64_wrapping(interp, v)?));
                    }
                    if let ValueKind::Bytes(b) = v.kind() {
                        let mut owned = Vec::with_capacity(b.len() + 1);
                        owned.extend_from_slice(b);
                        owned.push(0);
                        let boxed = owned.into_boxed_slice();
                        let addr = boxed.as_ptr() as usize as u64;
                        temps.push(boxed);
                        return Ok(AbiVal::Int(addr));
                    }
                    if let ValueKind::Str(s) = v.kind() {
                        let boxed = encode_wchar_null_terminated(s);
                        let addr = boxed.as_ptr() as usize as u64;
                        temps.push(boxed);
                        return Ok(AbiVal::Int(addr));
                    }
                    if let Ok(addr) = extract_pointer_or_cdata_addr(v) {
                        return Ok(AbiVal::Int(addr as u64));
                    }
                    Err(PyError::named(
                        "ArgumentError",
                        format!("cannot convert {} to c_void_p", value_type_name_str(v)),
                    ))
                }
                PrimKind::PyObject => Ok(AbiVal::Int(alloc_pyobject_handle(v.clone()) as u64)),
            }
        }
        CTypeInfo::Pointer { .. } | CTypeInfo::FuncPtr { .. } => {
            if val.is_none() {
                return Ok(AbiVal::Int(0));
            }
            if let Ok(addr) = extract_pointer_or_cdata_addr(val) {
                return Ok(AbiVal::Int(addr as u64));
            }
            Err(PyError::named(
                "ArgumentError",
                format!(
                    "expected {} instance instead of {}",
                    at_cls.borrow().name,
                    value_type_name_str(val)
                ),
            ))
        }
        CTypeInfo::Array { .. } => {
            if let Ok(addr) = extract_pointer_or_cdata_addr(val) {
                return Ok(AbiVal::Int(addr as u64));
            }
            Err(PyError::named(
                "ArgumentError",
                format!(
                    "expected {} instance instead of {}",
                    at_cls.borrow().name,
                    value_type_name_str(val)
                ),
            ))
        }
        CTypeInfo::Struct(layout) => {
            if let ValueKind::PyInstance(inst) = val.kind()
                && class_is_subclass_of(&inst.borrow().class, at_cls)
            {
                let bytes =
                    with_native_state::<CDataState, _>(val, "_CData", |st| st.storage.read_all())?;
                if layout.size <= 8 {
                    let mut raw = [0u8; 8];
                    raw[..bytes.len()].copy_from_slice(&bytes);
                    return Ok(AbiVal::Int(u64::from_ne_bytes(raw)));
                }
                // On Win64, structs > 8 bytes are passed by pointer to a copy.
                let boxed = bytes.into_boxed_slice();
                let addr = boxed.as_ptr() as usize as u64;
                temps.push(boxed);
                return Ok(AbiVal::Int(addr));
            }
            Err(PyError::named(
                "ArgumentError",
                format!(
                    "expected {} instance instead of {}",
                    at_cls.borrow().name,
                    value_type_name_str(val)
                ),
            ))
        }
    }
}

fn marshal_arg_untyped(
    interp: &mut Interpreter,
    val: &Value,
    temps: &mut Vec<Box<[u8]>>,
) -> Result<AbiVal> {
    if val.is_none() {
        return Ok(AbiVal::Int(0));
    }
    match val.kind() {
        ValueKind::Int(n) => Ok(AbiVal::Int(n as u64)),
        ValueKind::Bool(b) => Ok(AbiVal::Int(u64::from(b))),
        ValueKind::BigInt(_) => Ok(AbiVal::Int(value_to_u64_wrapping(interp, val)?)),
        ValueKind::Bytes(b) => {
            let mut owned = Vec::with_capacity(b.len() + 1);
            owned.extend_from_slice(b);
            owned.push(0);
            let boxed = owned.into_boxed_slice();
            let addr = boxed.as_ptr() as usize as u64;
            temps.push(boxed);
            Ok(AbiVal::Int(addr))
        }
        ValueKind::Str(s) => {
            let boxed = encode_wchar_null_terminated(s);
            let addr = boxed.as_ptr() as usize as u64;
            temps.push(boxed);
            Ok(AbiVal::Int(addr))
        }
        ValueKind::PyInstance(inst) => {
            if let Ok(addr) =
                with_native_state::<CArgObjectState, _>(val, "CArgObject", |st| Ok(st.addr))
            {
                return Ok(AbiVal::Int(addr as u64));
            }
            let cls = Rc::clone(&inst.borrow().class);
            if let Some(info) = lookup_ctype(&cls) {
                return match info {
                    CTypeInfo::Simple { kind, byte_order } => {
                        with_native_state::<CDataState, _>(val, "_CData", |st| {
                            let mut buf = [0u8; 16];
                            let sz = kind.size();
                            st.storage.read_bytes(0, &mut buf[..sz])?;
                            match kind {
                                PrimKind::Float => {
                                    Ok(AbiVal::F32(f32::from_bits(read_u32(&buf[..4], byte_order))))
                                }
                                PrimKind::Double | PrimKind::LongDouble => {
                                    Ok(AbiVal::F64(f64::from_bits(read_u64(&buf[..8], byte_order))))
                                }
                                PrimKind::Bool | PrimKind::Char | PrimKind::UByte => {
                                    Ok(AbiVal::Int(u64::from(buf[0])))
                                }
                                PrimKind::Byte => Ok(AbiVal::Int(i64::from(buf[0] as i8) as u64)),
                                PrimKind::Short | PrimKind::VariantBool => Ok(AbiVal::Int(
                                    i64::from(read_u16(&buf[..2], byte_order) as i16) as u64,
                                )),
                                PrimKind::UShort => {
                                    Ok(AbiVal::Int(u64::from(read_u16(&buf[..2], byte_order))))
                                }
                                PrimKind::WChar => {
                                    if WCHAR_SIZE == 2 {
                                        Ok(AbiVal::Int(u64::from(read_u16(&buf[..2], byte_order))))
                                    } else {
                                        Ok(AbiVal::Int(u64::from(read_u32(&buf[..4], byte_order))))
                                    }
                                }
                                PrimKind::Int => {
                                    Ok(AbiVal::Int(
                                        i64::from(read_u32(&buf[..4], byte_order) as i32) as u64,
                                    ))
                                }
                                PrimKind::UInt => {
                                    Ok(AbiVal::Int(u64::from(read_u32(&buf[..4], byte_order))))
                                }
                                PrimKind::Long => {
                                    if sz == 4 {
                                        Ok(AbiVal::Int(i64::from(
                                            read_u32(&buf[..4], byte_order) as i32
                                        )
                                            as u64))
                                    } else {
                                        Ok(AbiVal::Int(read_u64(&buf[..8], byte_order)))
                                    }
                                }
                                PrimKind::ULong => {
                                    if sz == 4 {
                                        Ok(AbiVal::Int(u64::from(read_u32(&buf[..4], byte_order))))
                                    } else {
                                        Ok(AbiVal::Int(read_u64(&buf[..8], byte_order)))
                                    }
                                }
                                PrimKind::LongLong | PrimKind::ULongLong => {
                                    Ok(AbiVal::Int(read_u64(&buf[..8], byte_order)))
                                }
                                PrimKind::CharP
                                | PrimKind::WCharP
                                | PrimKind::VoidP
                                | PrimKind::PyObject => {
                                    Ok(AbiVal::Int(
                                        read_ptr_usize(&st.storage, 0, byte_order)? as u64
                                    ))
                                }
                            }
                        })
                    }
                    CTypeInfo::Pointer { .. } | CTypeInfo::FuncPtr { .. } => {
                        let addr = with_native_state::<CDataState, _>(val, "_CData", |st| {
                            read_ptr_usize(&st.storage, 0, ByteOrder::Native)
                        })?;
                        Ok(AbiVal::Int(addr as u64))
                    }
                    CTypeInfo::Array { .. } => {
                        let addr = with_native_state::<CDataState, _>(val, "_CData", |st| {
                            Ok(st.storage.address())
                        })?;
                        Ok(AbiVal::Int(addr as u64))
                    }
                    CTypeInfo::Struct(ref layout) => {
                        let bytes = with_native_state::<CDataState, _>(val, "_CData", |st| {
                            st.storage.read_all()
                        })?;
                        if layout.size <= 8 {
                            let mut raw = [0u8; 8];
                            raw[..bytes.len()].copy_from_slice(&bytes);
                            Ok(AbiVal::Int(u64::from_ne_bytes(raw)))
                        } else {
                            let boxed = bytes.into_boxed_slice();
                            let addr = boxed.as_ptr() as usize as u64;
                            temps.push(boxed);
                            Ok(AbiVal::Int(addr))
                        }
                    }
                };
            }
            Err(PyError::named(
                "ArgumentError",
                format!(
                    "Don't know how to convert parameter of type {}",
                    value_type_name_str(val)
                ),
            ))
        }
        _ => Err(PyError::named(
            "ArgumentError",
            format!(
                "Don't know how to convert parameter of type {}",
                value_type_name_str(val)
            ),
        )),
    }
}

pub(crate) fn classify_restype(
    interp: &mut Interpreter,
    restype: Option<&Value>,
) -> Result<AbiRetKind> {
    let Some(rt) = restype else {
        return Ok(AbiRetKind::Void);
    };
    if rt.is_none() {
        return Ok(AbiRetKind::Void);
    }
    let ValueKind::PyClass(cls) = rt.kind() else {
        // Callable restype receives raw c_int return value in CPython
        return Ok(AbiRetKind::Int);
    };
    let info = ensure_ctype_info(interp, cls)?;
    match info {
        CTypeInfo::Simple { kind, .. } => match kind {
            PrimKind::Float => Ok(AbiRetKind::F32),
            PrimKind::Double | PrimKind::LongDouble => Ok(AbiRetKind::F64),
            _ => Ok(AbiRetKind::Int),
        },
        CTypeInfo::Pointer { .. } | CTypeInfo::FuncPtr { .. } => Ok(AbiRetKind::Int),
        CTypeInfo::Struct(layout) => Ok(AbiRetKind::StructBytes(layout.size)),
        CTypeInfo::Array { .. } => Ok(AbiRetKind::Int),
    }
}

pub(crate) fn unmarshal_ffi_return(
    interp: &mut Interpreter,
    restype: Option<&Value>,
    ret: AbiRet,
) -> Result<Value> {
    let Some(rt) = restype else {
        return Ok(Value::none());
    };
    if rt.is_none() {
        return Ok(Value::none());
    }
    let ValueKind::PyClass(cls) = rt.kind() else {
        // Non-ctype callable restype: call `restype(int_result)`
        let int_val = match ret {
            AbiRet::Int(v) => Value::int(i64::from(v as i32)),
            AbiRet::F32(f) => Value::float(f64::from(f)),
            AbiRet::F64(f) => Value::float(f),
            _ => Value::int(0),
        };
        return interp.call_function_expanded(
            rt.clone(),
            &[ExpandedCallArg {
                name: None,
                value: int_val,
            }],
        );
    };
    let info = ensure_ctype_info(interp, cls)?;
    match info {
        CTypeInfo::Simple { kind, .. } => match ret {
            AbiRet::F32(f) => Ok(Value::float(f64::from(f))),
            AbiRet::F64(f) => Ok(Value::float(f)),
            AbiRet::Int(raw) => match kind {
                PrimKind::Bool => Ok(Value::bool_((raw as u8) != 0)),
                PrimKind::VariantBool => Ok(Value::bool_((raw as i16) != 0)),
                PrimKind::Char => Ok(Value::bytes(vec![raw as u8])),
                PrimKind::WChar => {
                    let cp = if WCHAR_SIZE == 2 {
                        u32::from(raw as u16)
                    } else {
                        raw as u32
                    };
                    Ok(Value::string(
                        char::from_u32(cp).unwrap_or('\u{FFFD}').to_string(),
                    ))
                }
                PrimKind::Byte => Ok(Value::int(i64::from(raw as i8))),
                PrimKind::UByte => Ok(Value::int(i64::from(raw as u8))),
                PrimKind::Short => Ok(Value::int(i64::from(raw as i16))),
                PrimKind::UShort => Ok(Value::int(i64::from(raw as u16))),
                PrimKind::Int => Ok(Value::int(i64::from(raw as i32))),
                PrimKind::UInt => Ok(Value::int(i64::from(raw as u32))),
                PrimKind::Long => {
                    if kind.size() == 4 {
                        Ok(Value::int(i64::from(raw as i32)))
                    } else {
                        Ok(Value::int(raw as i64))
                    }
                }
                PrimKind::ULong => {
                    if kind.size() == 4 {
                        Ok(Value::int(i64::from(raw as u32)))
                    } else {
                        Ok(u64_to_py_value(raw))
                    }
                }
                PrimKind::LongLong => Ok(Value::int(raw as i64)),
                PrimKind::ULongLong => Ok(u64_to_py_value(raw)),
                PrimKind::CharP => {
                    let addr = raw as usize;
                    if addr == 0 {
                        Ok(Value::none())
                    } else {
                        Ok(Value::bytes(read_c_string(addr, None)?))
                    }
                }
                PrimKind::WCharP => {
                    let addr = raw as usize;
                    if addr == 0 {
                        Ok(Value::none())
                    } else {
                        Ok(Value::string(read_c_wstring(addr, None)?))
                    }
                }
                PrimKind::VoidP => {
                    let addr = raw as usize;
                    if addr == 0 {
                        Ok(Value::none())
                    } else {
                        Ok(u64_to_py_value(addr as u64))
                    }
                }
                PrimKind::PyObject => lookup_pyobject_handle(raw as usize)
                    .ok_or_else(|| PyError::named("ValueError", "PyObject is NULL")),
                PrimKind::Float | PrimKind::Double | PrimKind::LongDouble => {
                    Ok(Value::float(f64::from_bits(raw)))
                }
            },
            _ => Ok(Value::none()),
        },
        CTypeInfo::Pointer { .. } | CTypeInfo::FuncPtr { .. } => {
            let addr = match ret {
                AbiRet::Int(v) => v as usize,
                _ => 0,
            };
            let storage = CDataStorage::allocate_zeroed(PTR_SIZE);
            write_ptr_usize(&storage, 0, addr, ByteOrder::Native)?;
            create_cdata_instance(Rc::clone(cls), storage)
        }
        CTypeInfo::Struct(layout) => {
            let storage = CDataStorage::allocate_zeroed(layout.size);
            if let AbiRet::StructBytes(buf, sz) = ret {
                storage.write_bytes(0, &buf[..sz.min(layout.size)])?;
            }
            create_cdata_instance(Rc::clone(cls), storage)
        }
        CTypeInfo::Array { .. } => Ok(Value::none()),
    }
}

/// Convert a raw C callback argument into a Python `Value` per `arg_type`.
pub(crate) fn abi_val_to_python(
    interp: &mut Interpreter,
    arg_type: &Value,
    raw: AbiVal,
) -> Result<Value> {
    let ret = match raw {
        AbiVal::Int(v) => AbiRet::Int(v),
        AbiVal::F32(v) => AbiRet::F32(v),
        AbiVal::F64(v) => AbiRet::F64(v),
    };
    unmarshal_ffi_return(interp, Some(arg_type), ret)
}

/// Convert a Python callback return value into a raw [`AbiVal`] per `restype`.
pub(crate) fn python_to_abi_return(
    interp: &mut Interpreter,
    restype: &Value,
    py_val: &Value,
) -> Result<AbiVal> {
    let mut temps = Vec::new();
    marshal_ffi_arg(interp, py_val, Some(restype), &mut temps)
}
