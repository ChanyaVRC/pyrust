//! C type metadata, platform ABI sizes/alignments, structure/union/bitfield
//! layout computation, and thread-local type/handle registries.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyClass, Value, ValueKind};

// ── Byte Order ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ByteOrder {
    Native,
    Little,
    Big,
}

impl ByteOrder {
    #[inline]
    pub(crate) fn is_swapped(self) -> bool {
        match self {
            ByteOrder::Native => false,
            ByteOrder::Little => cfg!(target_endian = "big"),
            ByteOrder::Big => cfg!(target_endian = "little"),
        }
    }

    #[inline]
    pub(crate) fn other_endian() -> Self {
        if cfg!(target_endian = "little") {
            ByteOrder::Big
        } else {
            ByteOrder::Little
        }
    }

    #[inline]
    pub(crate) fn native_endian() -> Self {
        if cfg!(target_endian = "little") {
            ByteOrder::Little
        } else {
            ByteOrder::Big
        }
    }
}

// ── Platform C Scalar Sizes & Alignments ─────────────────────────────────────

pub(crate) const PTR_SIZE: usize = std::mem::size_of::<usize>();
pub(crate) const PTR_ALIGN: usize = std::mem::align_of::<usize>();

#[cfg(windows)]
pub(crate) const C_LONG_SIZE: usize = 4;
#[cfg(not(windows))]
pub(crate) const C_LONG_SIZE: usize = std::mem::size_of::<libc::c_long>();

#[cfg(windows)]
pub(crate) const C_LONG_ALIGN: usize = 4;
#[cfg(not(windows))]
pub(crate) const C_LONG_ALIGN: usize = std::mem::align_of::<libc::c_long>();

#[cfg(windows)]
pub(crate) const WCHAR_SIZE: usize = 2;
#[cfg(not(windows))]
pub(crate) const WCHAR_SIZE: usize = 4;

#[cfg(windows)]
pub(crate) const WCHAR_ALIGN: usize = 2;
#[cfg(not(windows))]
pub(crate) const WCHAR_ALIGN: usize = 4;

#[cfg(any(windows, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) const LONGDOUBLE_SIZE: usize = 8;
#[cfg(not(any(windows, all(target_os = "macos", target_arch = "aarch64"))))]
pub(crate) const LONGDOUBLE_SIZE: usize = 16;

#[cfg(any(windows, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) const LONGDOUBLE_ALIGN: usize = 8;
#[cfg(not(any(windows, all(target_os = "macos", target_arch = "aarch64"))))]
pub(crate) const LONGDOUBLE_ALIGN: usize = 16;

// ── Primitive C Types ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PrimKind {
    Bool,        // '?'
    Char,        // 'c'
    WChar,       // 'u'
    Byte,        // 'b'
    UByte,       // 'B'
    Short,       // 'h'
    UShort,      // 'H'
    Int,         // 'i'
    UInt,        // 'I'
    Long,        // 'l'
    ULong,       // 'L'
    LongLong,    // 'q'
    ULongLong,   // 'Q'
    Float,       // 'f'
    Double,      // 'd'
    LongDouble,  // 'g'
    CharP,       // 'z'
    WCharP,      // 'Z'
    VoidP,       // 'P'
    PyObject,    // 'O'
    VariantBool, // 'v'
}

impl PrimKind {
    pub(crate) fn from_code(code: &str) -> Option<Self> {
        match code {
            "?" => Some(Self::Bool),
            "c" => Some(Self::Char),
            "u" => Some(Self::WChar),
            "b" => Some(Self::Byte),
            "B" => Some(Self::UByte),
            "h" => Some(Self::Short),
            "H" => Some(Self::UShort),
            "i" => Some(Self::Int),
            "I" => Some(Self::UInt),
            "l" => Some(Self::Long),
            "L" => Some(Self::ULong),
            "q" => Some(Self::LongLong),
            "Q" => Some(Self::ULongLong),
            "f" => Some(Self::Float),
            "d" => Some(Self::Double),
            "g" => Some(Self::LongDouble),
            "z" => Some(Self::CharP),
            "Z" => Some(Self::WCharP),
            "P" => Some(Self::VoidP),
            "O" => Some(Self::PyObject),
            "v" => Some(Self::VariantBool),
            _ => None,
        }
    }

    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Bool => "?",
            Self::Char => "c",
            Self::WChar => "u",
            Self::Byte => "b",
            Self::UByte => "B",
            Self::Short => "h",
            Self::UShort => "H",
            Self::Int => "i",
            Self::UInt => "I",
            Self::Long => "l",
            Self::ULong => "L",
            Self::LongLong => "q",
            Self::ULongLong => "Q",
            Self::Float => "f",
            Self::Double => "d",
            Self::LongDouble => "g",
            Self::CharP => "z",
            Self::WCharP => "Z",
            Self::VoidP => "P",
            Self::PyObject => "O",
            Self::VariantBool => "v",
        }
    }

    pub(crate) const fn size(self) -> usize {
        match self {
            Self::Bool | Self::Char | Self::Byte | Self::UByte => 1,
            Self::WChar => WCHAR_SIZE,
            Self::Short | Self::UShort | Self::VariantBool => 2,
            Self::Int | Self::UInt | Self::Float => 4,
            Self::Long | Self::ULong => C_LONG_SIZE,
            Self::LongLong | Self::ULongLong | Self::Double => 8,
            Self::LongDouble => LONGDOUBLE_SIZE,
            Self::CharP | Self::WCharP | Self::VoidP | Self::PyObject => PTR_SIZE,
        }
    }

    pub(crate) const fn align(self) -> usize {
        match self {
            Self::Bool | Self::Char | Self::Byte | Self::UByte => 1,
            Self::WChar => WCHAR_ALIGN,
            Self::Short | Self::UShort | Self::VariantBool => 2,
            Self::Int | Self::UInt | Self::Float => 4,
            Self::Long | Self::ULong => C_LONG_ALIGN,
            Self::LongLong | Self::ULongLong | Self::Double => 8,
            Self::LongDouble => LONGDOUBLE_ALIGN,
            Self::CharP | Self::WCharP | Self::VoidP | Self::PyObject => PTR_ALIGN,
        }
    }

    pub(crate) const fn is_signed_int(self) -> bool {
        matches!(
            self,
            Self::Byte | Self::Short | Self::Int | Self::Long | Self::LongLong
        )
    }

    pub(crate) const fn is_unsigned_int(self) -> bool {
        matches!(
            self,
            Self::UByte | Self::UShort | Self::UInt | Self::ULong | Self::ULongLong
        )
    }

    pub(crate) const fn is_integer_for_bitfield(self) -> bool {
        matches!(
            self,
            Self::Bool
                | Self::Byte
                | Self::UByte
                | Self::Short
                | Self::UShort
                | Self::Int
                | Self::UInt
                | Self::Long
                | Self::ULong
                | Self::LongLong
                | Self::ULongLong
        )
    }

    pub(crate) const fn supports_byte_swap(self) -> bool {
        matches!(
            self,
            Self::Bool
                | Self::Char
                | Self::WChar
                | Self::Byte
                | Self::UByte
                | Self::Short
                | Self::UShort
                | Self::Int
                | Self::UInt
                | Self::Long
                | Self::ULong
                | Self::LongLong
                | Self::ULongLong
                | Self::Float
                | Self::Double
                | Self::LongDouble
                | Self::VoidP
        )
    }
}

// ── Struct / Union Layout Metadata ───────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct FieldLayout {
    pub(crate) name: String,
    pub(crate) field_cls: Rc<RefCell<PyClass>>,
    pub(crate) offset: usize,
    pub(crate) size: usize,
    pub(crate) bit_offset: Option<u32>,
    pub(crate) bit_width: Option<u32>,
}

impl FieldLayout {
    /// Value returned by `CField.size` in CPython:
    /// - Regular field: byte size.
    /// - Bitfield: `(bit_width << 16) | bit_offset`.
    pub(crate) fn py_size_attr(&self) -> i64 {
        if let (Some(bo), Some(bw)) = (self.bit_offset, self.bit_width) {
            (((bw as u64) << 16) | (bo as u64)) as i64
        } else {
            self.size as i64
        }
    }
}

#[derive(Clone)]
pub(crate) struct StructLayout {
    pub(crate) _is_union: bool,
    pub(crate) swapped_bytes: bool,
    pub(crate) size: usize,
    pub(crate) align: usize,
    pub(crate) fields: Vec<FieldLayout>,
    pub(crate) _anonymous: Vec<String>,
    pub(crate) finalized: bool,
    pub(crate) _pack: Option<usize>,
}

impl StructLayout {
    pub(crate) fn empty(is_union: bool, swapped_bytes: bool) -> Self {
        Self {
            _is_union: is_union,
            swapped_bytes,
            size: 0,
            align: 1,
            fields: Vec::new(),
            _anonymous: Vec::new(),
            finalized: false,
            _pack: None,
        }
    }
}

// ── Full CType Descriptor ────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) enum CTypeInfo {
    Simple {
        kind: PrimKind,
        byte_order: ByteOrder,
    },
    Array {
        elem_cls: Rc<RefCell<PyClass>>,
        length: usize,
        elem_size: usize,
        elem_align: usize,
    },
    Pointer {
        target_cls: Option<Rc<RefCell<PyClass>>>,
    },
    Struct(StructLayout),
    FuncPtr {
        restype: Option<Value>,
        argtypes: Option<Vec<Value>>,
        flags: i32,
    },
}

impl CTypeInfo {
    pub(crate) fn size(&self) -> usize {
        match self {
            Self::Simple { kind, .. } => kind.size(),
            Self::Array {
                length, elem_size, ..
            } => length.saturating_mul(*elem_size),
            Self::Pointer { .. } | Self::FuncPtr { .. } => PTR_SIZE,
            Self::Struct(layout) => layout.size,
        }
    }

    pub(crate) fn align(&self) -> usize {
        match self {
            Self::Simple { kind, .. } => kind.align(),
            Self::Array { elem_align, .. } => (*elem_align).max(1),
            Self::Pointer { .. } | Self::FuncPtr { .. } => PTR_ALIGN,
            Self::Struct(layout) => layout.align.max(1),
        }
    }
}

// ── Thread-Local Registries ──────────────────────────────────────────────────

// Thread-local justification:
// (a) `PyClass` is `!Send` (`Rc<RefCell<PyClass>>`), so per-class C type metadata,
//     pointer/array caches, and `py_object` value handles must live on the
//     owning interpreter thread.
// (b) Weak `PyClass` keys are pruned on insertion so abandoned user classes do
//     not leak across interpreter runs.
type TypeEntry = (Weak<RefCell<PyClass>>, CTypeInfo);
type PtrCacheEntry = (Weak<RefCell<PyClass>>, Rc<RefCell<PyClass>>);
type ArrCacheEntry = ((Weak<RefCell<PyClass>>, usize), Rc<RefCell<PyClass>>);
type SwapCacheEntry = (Weak<RefCell<PyClass>>, Rc<RefCell<PyClass>>);

thread_local! {
    static CTYPE_REGISTRY: RefCell<Vec<TypeEntry>> = const { RefCell::new(Vec::new()) };
    static POINTER_CACHE: RefCell<Vec<PtrCacheEntry>> = const { RefCell::new(Vec::new()) };
    static ARRAY_CACHE: RefCell<Vec<ArrCacheEntry>> = const { RefCell::new(Vec::new()) };
    static SWAPPED_CACHE: RefCell<Vec<SwapCacheEntry>> = const { RefCell::new(Vec::new()) };
    static PYOBJECT_HANDLES: RefCell<(usize, IndexMap<usize, Value>)> =
        RefCell::new((0x1000, IndexMap::new()));
    /// Core `_ctypes` base classes cached for fast subtype checks and dynamic type creation.
    static CORE_CLASSES: RefCell<Option<CoreCtypesClasses>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub(crate) struct CoreCtypesClasses {
    pub(crate) _cdata_meta: Rc<RefCell<PyClass>>,
    pub(crate) simple_meta: Rc<RefCell<PyClass>>,
    pub(crate) array_meta: Rc<RefCell<PyClass>>,
    pub(crate) pointer_meta: Rc<RefCell<PyClass>>,
    pub(crate) struct_meta: Rc<RefCell<PyClass>>,
    pub(crate) union_meta: Rc<RefCell<PyClass>>,
    pub(crate) funcptr_meta: Rc<RefCell<PyClass>>,
    pub(crate) cdata_base: Rc<RefCell<PyClass>>,
    pub(crate) simple_base: Rc<RefCell<PyClass>>,
    pub(crate) array_base: Rc<RefCell<PyClass>>,
    pub(crate) pointer_base: Rc<RefCell<PyClass>>,
    pub(crate) struct_base: Rc<RefCell<PyClass>>,
    pub(crate) union_base: Rc<RefCell<PyClass>>,
    pub(crate) funcptr_base: Rc<RefCell<PyClass>>,
    pub(crate) carg_cls: Rc<RefCell<PyClass>>,
    pub(crate) cfield_cls: Rc<RefCell<PyClass>>,
    pub(crate) _arg_error_cls: Rc<RefCell<PyClass>>,
}

pub(crate) fn set_core_classes(classes: CoreCtypesClasses) {
    CORE_CLASSES.with(|c| {
        *c.borrow_mut() = Some(classes);
    });
}

pub(crate) fn core_classes() -> Result<CoreCtypesClasses> {
    CORE_CLASSES.with(|c| {
        c.borrow().clone().ok_or_else(|| {
            PyError::runtime("internal: _ctypes core classes not initialized".to_string())
        })
    })
}

pub(crate) fn register_ctype(cls: &Rc<RefCell<PyClass>>, info: CTypeInfo) {
    let target = Rc::as_ptr(cls);
    CTYPE_REGISTRY.with(|cell| {
        let mut reg = cell.borrow_mut();
        reg.retain(|(w, _)| w.strong_count() > 0);
        if let Some((_, existing)) = reg.iter_mut().find(|(w, _)| {
            w.upgrade()
                .is_some_and(|rc| std::ptr::eq(Rc::as_ptr(&rc), target))
        }) {
            *existing = info;
        } else {
            reg.push((Rc::downgrade(cls), info));
        }
    });
}

pub(crate) fn lookup_ctype_exact(cls: &Rc<RefCell<PyClass>>) -> Option<CTypeInfo> {
    let target = Rc::as_ptr(cls);
    CTYPE_REGISTRY.with(|cell| {
        cell.borrow().iter().find_map(|(w, info)| {
            w.upgrade()
                .is_some_and(|rc| std::ptr::eq(Rc::as_ptr(&rc), target))
                .then(|| info.clone())
        })
    })
}

/// Look up CTypeInfo for `cls` or its nearest registered ancestor in the base chain.
pub(crate) fn lookup_ctype(cls: &Rc<RefCell<PyClass>>) -> Option<CTypeInfo> {
    if let Some(info) = lookup_ctype_exact(cls) {
        return Some(info);
    }
    // Check `_type_` on custom `_SimpleCData` subclasses (e.g., `VARIANT_BOOL` or user subclasses)
    if let Some(code_val) = cls.borrow().attrs.get("_type_").cloned()
        && let ValueKind::Str(code) = code_val.kind()
        && let Some(kind) = PrimKind::from_code(code)
    {
        return Some(CTypeInfo::Simple {
            kind,
            byte_order: ByteOrder::Native,
        });
    }
    let mut cur = cls.borrow().base.clone();
    while let Some(base) = cur {
        if let Some(info) = lookup_ctype_exact(&base) {
            return Some(info);
        }
        cur = base.borrow().base.clone();
    }
    None
}

pub(crate) fn lookup_pointer_cache(target: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
    let ptr = Rc::as_ptr(target);
    POINTER_CACHE.with(|cell| {
        cell.borrow().iter().find_map(|(w, pcls)| {
            w.upgrade()
                .is_some_and(|rc| std::ptr::eq(Rc::as_ptr(&rc), ptr))
                .then(|| Rc::clone(pcls))
        })
    })
}

pub(crate) fn insert_pointer_cache(target: &Rc<RefCell<PyClass>>, ptr_cls: Rc<RefCell<PyClass>>) {
    POINTER_CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        cache.retain(|(w, _)| w.strong_count() > 0);
        cache.push((Rc::downgrade(target), ptr_cls));
    });
}

pub(crate) fn lookup_array_cache(
    elem: &Rc<RefCell<PyClass>>,
    length: usize,
) -> Option<Rc<RefCell<PyClass>>> {
    let ptr = Rc::as_ptr(elem);
    ARRAY_CACHE.with(|cell| {
        cell.borrow().iter().find_map(|((w, len), acls)| {
            (*len == length
                && w.upgrade()
                    .is_some_and(|rc| std::ptr::eq(Rc::as_ptr(&rc), ptr)))
            .then(|| Rc::clone(acls))
        })
    })
}

pub(crate) fn insert_array_cache(
    elem: &Rc<RefCell<PyClass>>,
    length: usize,
    arr_cls: Rc<RefCell<PyClass>>,
) {
    ARRAY_CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        cache.retain(|((w, _), _)| w.strong_count() > 0);
        cache.push(((Rc::downgrade(elem), length), arr_cls));
    });
}

pub(crate) fn lookup_swapped_cache(orig: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
    let ptr = Rc::as_ptr(orig);
    SWAPPED_CACHE.with(|cell| {
        cell.borrow().iter().find_map(|(w, scls)| {
            w.upgrade()
                .is_some_and(|rc| std::ptr::eq(Rc::as_ptr(&rc), ptr))
                .then(|| Rc::clone(scls))
        })
    })
}

pub(crate) fn insert_swapped_cache(orig: &Rc<RefCell<PyClass>>, swapped: Rc<RefCell<PyClass>>) {
    SWAPPED_CACHE.with(|cell| {
        let mut cache = cell.borrow_mut();
        cache.retain(|(w, _)| w.strong_count() > 0);
        cache.push((Rc::downgrade(orig), swapped));
    });
}

pub(crate) fn clear_caches() {
    POINTER_CACHE.with(|c| c.borrow_mut().clear());
    ARRAY_CACHE.with(|c| c.borrow_mut().clear());
}

// ── `py_object` Handle Table ─────────────────────────────────────────────────

pub(crate) fn alloc_pyobject_handle(val: Value) -> usize {
    PYOBJECT_HANDLES.with(|cell| {
        let mut state = cell.borrow_mut();
        let id = state.0;
        state.0 = id.wrapping_add(8).max(0x1000);
        state.1.insert(id, val);
        id
    })
}

pub(crate) fn lookup_pyobject_handle(id: usize) -> Option<Value> {
    if id == 0 {
        return None;
    }
    PYOBJECT_HANDLES.with(|cell| cell.borrow().1.get(&id).cloned())
}

// ── Struct / Union Layout Engine ─────────────────────────────────────────────

#[inline]
pub(crate) fn align_up(offset: usize, align: usize) -> usize {
    if align <= 1 {
        return offset;
    }
    let rem = offset % align;
    if rem == 0 {
        offset
    } else {
        offset + (align - rem)
    }
}

/// Parse `_pack_` attribute on a Structure/Union class.
pub(crate) fn parse_pack_attr(cls: &Rc<RefCell<PyClass>>) -> Result<Option<usize>> {
    let pack_val = cls.borrow().attrs.get("_pack_").cloned();
    let Some(v) = pack_val else {
        return Ok(None);
    };
    let n = match v.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "The _pack_ attribute must be a non-negative integer",
            ));
        }
    };
    if n < 0 {
        return Err(PyError::named(
            "ValueError",
            "The _pack_ attribute must be a non-negative integer",
        ));
    }
    if n == 0 {
        Ok(None)
    } else {
        Ok(Some(n as usize))
    }
}

/// Compute the C ABI layout of a Structure or Union from its base layout and `_fields_` list.
pub(crate) fn compute_struct_layout(
    interp: &mut Interpreter,
    is_union: bool,
    swapped_bytes: bool,
    pack: Option<usize>,
    base_layout: Option<&StructLayout>,
    fields_val: &Value,
    anonymous: Vec<String>,
) -> Result<StructLayout> {
    let field_items = interp.collect_iterable(fields_val)?;

    let mut fields: Vec<FieldLayout> = Vec::new();
    let mut max_align: usize = 1;
    let mut cur_offset: usize = 0;
    let mut union_size: usize = 0;

    // Inherit base fields for Structure subclasses (CPython concatenates base fields first).
    if let Some(base) = base_layout
        && !base.fields.is_empty()
    {
        fields.extend(base.fields.iter().cloned());
        max_align = base.align.max(1);
        if is_union {
            union_size = base.size;
        } else {
            cur_offset = base.size;
        }
    }

    // Active bitfield storage-unit state (for Structure):
    // (unit_byte_offset, unit_byte_size, bits_used_in_unit)
    let mut bf_state: Option<(usize, usize, u32)> = None;

    for item in field_items {
        let elems = interp.collect_iterable(&item)?;
        if elems.len() != 2 && elems.len() != 3 {
            return Err(PyError::named(
                "ValueError",
                "_fields_ entries must be 2-tuples or 3-tuples",
            ));
        }
        let field_name = match elems[0].kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named("TypeError", "field name must be a string"));
            }
        };
        let ValueKind::PyClass(raw_field_cls) = elems[1].kind() else {
            return Err(PyError::named(
                "TypeError",
                format!("second item in _fields_ tuple (index '{field_name}') must be a C type"),
            ));
        };
        let mut field_cls = Rc::clone(raw_field_cls);

        // Ensure nested structures have their layouts finalized first.
        let mut field_info = super::ensure_ctype_info(interp, &field_cls)?;

        // For BigEndianStructure / LittleEndianStructure with `_swappedbytes_`, swap field types.
        if swapped_bytes {
            field_cls = super::other_endian_type(interp, &field_cls)?;
            field_info = super::ensure_ctype_info(interp, &field_cls)?;
        }

        let bit_width: Option<u32> = if elems.len() == 3 {
            let bw_i64 = match elems[2].kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => {
                    return Err(PyError::named("TypeError", "bit width must be an integer"));
                }
            };
            let CTypeInfo::Simple { kind, .. } = field_info else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "bit fields not allowed for type {}",
                        field_cls.borrow().name
                    ),
                ));
            };
            if !kind.is_integer_for_bitfield() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "bit fields not allowed for type {}",
                        field_cls.borrow().name
                    ),
                ));
            }
            let max_bits = (kind.size() * 8) as i64;
            if bw_i64 <= 0 {
                return Err(PyError::named(
                    "ValueError",
                    "number of bits invalid for bit field",
                ));
            }
            if bw_i64 > max_bits {
                return Err(PyError::named(
                    "ValueError",
                    "number of bits invalid for bit field",
                ));
            }
            Some(bw_i64 as u32)
        } else {
            None
        };

        let fsize = field_info.size();
        let raw_align = field_info.align().max(1);
        let falign = match pack {
            Some(p) => raw_align.min(p).max(1),
            None => raw_align,
        };
        max_align = max_align.max(falign);

        if is_union {
            let (bo, bw) = match bit_width {
                Some(bw) => {
                    let unit_bits = (fsize * 8) as u32;
                    let bit_off = if cfg!(target_endian = "little") ^ swapped_bytes {
                        0
                    } else {
                        unit_bits - bw
                    };
                    (Some(bit_off), Some(bw))
                }
                None => (None, None),
            };
            union_size = union_size.max(fsize);
            fields.push(FieldLayout {
                name: field_name,
                field_cls,
                offset: 0,
                size: fsize,
                bit_offset: bo,
                bit_width: bw,
            });
        } else if let Some(bw) = bit_width {
            let unit_size = fsize;
            let unit_bits = (unit_size * 8) as u32;
            let can_pack = if let Some((unit_off, prev_unit_size, used_bits)) = bf_state {
                // On MSVC (Windows), bitfields only pack into the same storage unit if
                // their underlying storage unit size matches AND remaining bits suffice.
                // On GCC/Clang (Unix), bitfields also pack when storage unit matches and fits.
                let same_unit = prev_unit_size == unit_size;
                if same_unit && used_bits + bw <= unit_bits {
                    Some((unit_off, used_bits))
                } else {
                    None
                }
            } else {
                None
            };

            let (unit_off, start_bit) = if let Some((uoff, used)) = can_pack {
                bf_state = Some((uoff, unit_size, used + bw));
                (uoff, used)
            } else {
                let uoff = align_up(cur_offset, falign);
                cur_offset = uoff + unit_size;
                bf_state = Some((uoff, unit_size, bw));
                (uoff, 0)
            };

            // Bit numbering: LSB-first on native little-endian (unless swapped),
            // MSB-first on big-endian (or BigEndianStructure on little-endian).
            let bit_off = if cfg!(target_endian = "little") ^ swapped_bytes {
                start_bit
            } else {
                unit_bits - start_bit - bw
            };

            fields.push(FieldLayout {
                name: field_name,
                field_cls,
                offset: unit_off,
                size: unit_size,
                bit_offset: Some(bit_off),
                bit_width: Some(bw),
            });
        } else {
            bf_state = None;
            let foff = align_up(cur_offset, falign);
            cur_offset = foff + fsize;
            fields.push(FieldLayout {
                name: field_name,
                field_cls,
                offset: foff,
                size: fsize,
                bit_offset: None,
                bit_width: None,
            });
        }
    }

    let raw_total = if is_union { union_size } else { cur_offset };
    let total_size = align_up(raw_total, max_align);

    Ok(StructLayout {
        _is_union: is_union,
        swapped_bytes,
        size: total_size,
        align: max_align,
        fields,
        _anonymous: anonymous,
        finalized: true,
        _pack: pack,
    })
}
