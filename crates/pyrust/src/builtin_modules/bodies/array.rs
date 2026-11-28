// `array` module — 100% native Rust port of CPython 3.12's
// `Modules/arraymodule.c` (issues #2809, #3389).
//
// Everything lives here: the `array` type (`ArrayType` alias), the hidden
// `arrayiterator` type, `typecodes`, and `_array_reconstructor`.  There is no
// Python-source injection.
//
// Layout and conversions follow the C module exactly:
// - items are stored in native byte order with platform C sizes (`c_long` is
//   4 bytes on Windows / LLP64 and 8 bytes on LP64; `wchar_t` — the `'u'`
//   item — is 2 bytes on Windows and 4 bytes elsewhere);
// - item conversion reproduces the staged `PyArg_Parse` / `PyLong_As*`
//   range checks and their exact error messages;
// - every size change goes through `ArrayInner::begin_resize`, which applies
//   CPython's `ob_exports` guard ("cannot resize an array that is exporting
//   buffers") and mirrors `array_resize`'s over-allocation so `__sizeof__`
//   reports CPython's numbers;
// - argument errors use the wording of the C calling conventions each method
//   has in CPython (`METH_O`, `METH_NOARGS`, Argument Clinic, slot wrappers).
//
// Per-interpreter state (the hidden `arrayiterator` class) lives in a
// `gil_local!` registry.
//
// Reference: <https://docs.python.org/3/library/array.html>

use std::cell::{Cell, RefCell};
use std::ffi::{c_int, c_long, c_longlong, c_short, c_uint, c_ulong, c_ulonglong};
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, c3_linearize_classes, extract_buffer_bytes,
    invoke_class_method, is_stop_iteration_error, lookup_class_attr_owner,
    lookup_value_special_method, make_iterator, normalize_float_slot_result,
    object_class_singleton, release_memoryview_with_interp, render_value_repr, value_to_float,
};
use crate::value::{
    InstanceAttrs, PyBigInt, PyBigIntSign, PyClass, PyInstance, PyModule, PyToPrimitive, Value,
    ValueKind,
};

const MODULE_DOC: &str = "This module defines an object type which can efficiently represent
an array of basic values: characters, integers, floating point
numbers.  Arrays are sequence types and behave very much like lists,
except that the type of objects stored in them is constrained.
";

const ARRAY_DOC: &str = "array(typecode [, initializer]) -> array

Return a new array whose items are restricted by typecode, and
initialized from the optional initializer value, which must be a list,
string or iterable over elements of the appropriate type.

Arrays represent basic values and behave very much like lists, except
the type of objects stored in them is constrained. The type is specified
at object creation time by using a type code, which is a single character.
The following type codes are defined:

    Type code   C Type             Minimum size in bytes
    'b'         signed integer     1
    'B'         unsigned integer   1
    'u'         Unicode character  2 (see note)
    'h'         signed integer     2
    'H'         unsigned integer   2
    'i'         signed integer     2
    'I'         unsigned integer   2
    'l'         signed integer     4
    'L'         unsigned integer   4
    'q'         signed integer     8 (see note)
    'Q'         unsigned integer   8 (see note)
    'f'         floating point     4
    'd'         floating point     8

NOTE: The 'u' typecode corresponds to Python's unicode character. On
narrow builds this is 2-bytes on wide builds this is 4-bytes.

NOTE: The 'q' and 'Q' type codes are only available if the platform
C compiler used to build Python supports 'long long', or, on Windows,
'__int64'.

Methods:

append() -- append a new item to the end of the array
buffer_info() -- return information giving the current memory info
byteswap() -- byteswap all the items of the array
count() -- return number of occurrences of an object
extend() -- extend array by appending multiple elements from an iterable
fromfile() -- read items from a file object
fromlist() -- append items from the list
frombytes() -- append items from the string
index() -- return index of first occurrence of an object
insert() -- insert a new item into the array at a provided position
pop() -- remove and return item (default last)
remove() -- remove first occurrence of an object
reverse() -- reverse the order of the items in the array
tofile() -- write all items to a file object
tolist() -- return the array converted to an ordinary list
tobytes() -- return the array converted to a string

Attributes:

typecode -- the typecode character used to create the array
itemsize -- the length in bytes of one array item
";

/// `array.typecodes`, in CPython's `descriptors[]` order.
const TYPECODES: &str = "bBuhHiIlLqQfd";

/// CPython's `tp_name` spellings, used by diagnostics and identity checks.
const ARRAY_TP_NAME: &str = "array.array";
const ITER_TP_NAME: &str = "array.arrayiterator";

/// `sizeof(wchar_t)`: the storage of one `'u'` item.
const WCHAR_SIZE: usize = if cfg!(windows) { 2 } else { 4 };

/// `PY_SSIZE_T_MAX`.
const SSIZE_MAX: i64 = isize::MAX as i64;

/// `array.tofile()` writes in 64 KiB blocks.
const TOFILE_BLOCKSIZE: usize = 64 * 1024;

// ── Errors ───────────────────────────────────────────────────────────────────

/// `Py_TYPE(value)->tp_name`, as CPython's diagnostics spell it.
fn tp_name(value: &Value) -> String {
    pyrust_core::error_type_name(value).into_owned()
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

fn overflow_error(msg: impl Into<String>) -> PyError {
    PyError::named("OverflowError", msg.into())
}

fn index_error(msg: impl Into<String>) -> PyError {
    PyError::named("IndexError", msg.into())
}

fn memory_error() -> PyError {
    PyError::named("MemoryError", String::new())
}

fn resize_while_exported() -> PyError {
    PyError::named(
        "BufferError",
        "cannot resize an array that is exporting buffers".to_string(),
    )
}

/// `PyErr_BadArgument()`.
fn bad_argument() -> PyError {
    type_error("bad argument type for built-in operation")
}

// ── Typecodes ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TypeCode {
    SignedChar,
    UnsignedChar,
    Unicode,
    SignedShort,
    UnsignedShort,
    SignedInt,
    UnsignedInt,
    SignedLong,
    UnsignedLong,
    SignedLongLong,
    UnsignedLongLong,
    Float,
    Double,
}

impl TypeCode {
    /// CPython's `descriptors[]` table, in order.
    const ALL: [TypeCode; 13] = [
        Self::SignedChar,
        Self::UnsignedChar,
        Self::Unicode,
        Self::SignedShort,
        Self::UnsignedShort,
        Self::SignedInt,
        Self::UnsignedInt,
        Self::SignedLong,
        Self::UnsignedLong,
        Self::SignedLongLong,
        Self::UnsignedLongLong,
        Self::Float,
        Self::Double,
    ];

    fn from_char(c: char) -> Option<Self> {
        Self::ALL.into_iter().find(|tc| tc.as_char() == c)
    }

    fn as_char(self) -> char {
        match self {
            Self::SignedChar => 'b',
            Self::UnsignedChar => 'B',
            Self::Unicode => 'u',
            Self::SignedShort => 'h',
            Self::UnsignedShort => 'H',
            Self::SignedInt => 'i',
            Self::UnsignedInt => 'I',
            Self::SignedLong => 'l',
            Self::UnsignedLong => 'L',
            Self::SignedLongLong => 'q',
            Self::UnsignedLongLong => 'Q',
            Self::Float => 'f',
            Self::Double => 'd',
        }
    }

    fn itemsize(self) -> usize {
        match self {
            Self::SignedChar | Self::UnsignedChar => 1,
            Self::Unicode => WCHAR_SIZE,
            Self::SignedShort | Self::UnsignedShort => size_of::<c_short>(),
            Self::SignedInt | Self::UnsignedInt => size_of::<c_int>(),
            Self::SignedLong | Self::UnsignedLong => size_of::<c_long>(),
            Self::SignedLongLong | Self::UnsignedLongLong => size_of::<c_longlong>(),
            Self::Float => size_of::<f32>(),
            Self::Double => size_of::<f64>(),
        }
    }

    /// `Some(is_signed)` for the integer typecodes (`is_integer_type`).
    fn integer_signedness(self) -> Option<bool> {
        match self {
            Self::SignedChar
            | Self::SignedShort
            | Self::SignedInt
            | Self::SignedLong
            | Self::SignedLongLong => Some(true),
            Self::UnsignedChar
            | Self::UnsignedShort
            | Self::UnsignedInt
            | Self::UnsignedLong
            | Self::UnsignedLongLong => Some(false),
            Self::Unicode | Self::Float | Self::Double => None,
        }
    }

    /// The PEP 3118 format the buffer export reports (`'w'` on wide builds).
    fn buffer_format(self) -> &'static str {
        match self {
            Self::SignedChar => "b",
            Self::UnsignedChar => "B",
            Self::Unicode => {
                if WCHAR_SIZE == 4 {
                    "w"
                } else {
                    "u"
                }
            }
            Self::SignedShort => "h",
            Self::UnsignedShort => "H",
            Self::SignedInt => "i",
            Self::UnsignedInt => "I",
            Self::SignedLong => "l",
            Self::UnsignedLong => "L",
            Self::SignedLongLong => "q",
            Self::UnsignedLongLong => "Q",
            Self::Float => "f",
            Self::Double => "d",
        }
    }

    /// `typecode_to_mformat_code()`.
    fn mformat_code(self) -> i64 {
        let big_endian = i64::from(cfg!(target_endian = "big"));
        match self {
            Self::SignedChar => SIGNED_INT8,
            Self::UnsignedChar => UNSIGNED_INT8,
            Self::Unicode => {
                if WCHAR_SIZE == 2 {
                    UTF16_LE + big_endian
                } else {
                    UTF32_LE + big_endian
                }
            }
            Self::Float => IEEE_754_FLOAT_LE + big_endian,
            Self::Double => IEEE_754_DOUBLE_LE + big_endian,
            _ => {
                let signed = i64::from(self.integer_signedness() == Some(true));
                let base = match self.itemsize() {
                    2 => UNSIGNED_INT16_LE,
                    4 => UNSIGNED_INT32_LE,
                    8 => UNSIGNED_INT64_LE,
                    _ => return UNKNOWN_FORMAT,
                };
                base + big_endian + 2 * signed
            }
        }
    }
}

// ── Machine format codes (pickling) ─────────────────────────────────────────

const UNKNOWN_FORMAT: i64 = -1;
const UNSIGNED_INT8: i64 = 0;
const SIGNED_INT8: i64 = 1;
const UNSIGNED_INT16_LE: i64 = 2;
const UNSIGNED_INT32_LE: i64 = 6;
const UNSIGNED_INT64_LE: i64 = 10;
const SIGNED_INT64_BE: i64 = 13;
const IEEE_754_FLOAT_LE: i64 = 14;
const IEEE_754_FLOAT_BE: i64 = 15;
const IEEE_754_DOUBLE_LE: i64 = 16;
const IEEE_754_DOUBLE_BE: i64 = 17;
const UTF16_LE: i64 = 18;
const UTF16_BE: i64 = 19;
const UTF32_LE: i64 = 20;
const UTF32_BE: i64 = 21;
const MACHINE_FORMAT_CODE_MAX: i64 = 21;

/// `mformat_descriptors[]`: `(size, is_signed, is_big_endian)`.  The UTF-16
/// and UTF-32 sizes (4 and 8) are CPython's, quirks included: they drive the
/// "string length not a multiple of item size" check.
const MFORMAT_DESCRIPTORS: [(usize, bool, bool); 22] = [
    (1, false, false),
    (1, true, false),
    (2, false, false),
    (2, false, true),
    (2, true, false),
    (2, true, true),
    (4, false, false),
    (4, false, true),
    (4, true, false),
    (4, true, true),
    (8, false, false),
    (8, false, true),
    (8, true, false),
    (8, true, true),
    (4, false, false),
    (4, false, true),
    (8, false, false),
    (8, false, true),
    (4, false, false),
    (4, false, true),
    (8, false, false),
    (8, false, true),
];

// ── Array storage ────────────────────────────────────────────────────────────

/// The C `arrayobject` payload: typecode, native-order item bytes, the
/// buffer export count, and the `allocated` item capacity CPython would
/// have (reported by `__sizeof__`).
struct ArrayInner {
    tc: TypeCode,
    data: Rc<RefCell<Vec<u8>>>,
    exports: Rc<Cell<usize>>,
    allocated: Cell<usize>,
}

/// Native-state payload attached to every `array` instance.
struct ArrayState(Rc<ArrayInner>);

impl ArrayInner {
    /// `newarrayobject(type, len, descr)` filled with `bytes`.
    fn new(tc: TypeCode, bytes: Vec<u8>) -> Rc<Self> {
        let allocated = bytes.len() / tc.itemsize();
        Rc::new(Self {
            tc,
            data: Rc::new(RefCell::new(bytes)),
            exports: Rc::new(Cell::new(0)),
            allocated: Cell::new(allocated),
        })
    }

    fn itemsize(&self) -> usize {
        self.tc.itemsize()
    }

    fn len(&self) -> usize {
        self.data.borrow().len() / self.itemsize()
    }

    fn snapshot(&self) -> Vec<u8> {
        self.data.borrow().clone()
    }

    /// `getarrayitem(self, i)` — `i` must be in range.
    fn item(&self, i: usize) -> Result<Value> {
        let size = self.itemsize();
        let data = self.data.borrow();
        decode_item(self.tc, &data[i * size..(i + 1) * size])
    }

    /// The `array_resize(self, new_len)` bookkeeping: refuse to resize while
    /// buffers are exported, then mirror CPython's over-allocation policy.
    /// Callers apply the actual byte change afterwards.
    fn begin_resize(&self, new_len: usize) -> Result<()> {
        let old_len = self.len();
        if self.exports.get() > 0 && new_len != old_len {
            return Err(resize_while_exported());
        }
        self.track_resize(old_len, new_len);
        Ok(())
    }

    /// Allocation accounting of `array_resize` (no export check).
    fn track_resize(&self, old_len: usize, new_len: usize) {
        let allocated = self.allocated.get();
        if allocated >= new_len && old_len < new_len + 16 && allocated > 0 {
            return;
        }
        if new_len == 0 {
            self.allocated.set(0);
            return;
        }
        let extra = if old_len < 8 { 3 } else { 7 };
        self.allocated.set((new_len >> 4) + extra + new_len);
    }

    /// `array_del_slice(self, ilow, ihigh)`.
    fn del_slice(&self, ilow: usize, ihigh: usize) -> Result<()> {
        let len = self.len();
        let ilow = ilow.min(len);
        let ihigh = ihigh.clamp(ilow, len);
        let removed = ihigh - ilow;
        if removed != 0 && self.exports.get() > 0 {
            return Err(resize_while_exported());
        }
        if removed > 0 {
            self.begin_resize(len - removed)?;
            let size = self.itemsize();
            self.data.borrow_mut().drain(ilow * size..ihigh * size);
        }
        Ok(())
    }

    /// Append raw item bytes (`array_do_extend` with another array).
    fn extend_bytes(&self, bytes: &[u8]) -> Result<()> {
        let size = self.itemsize();
        self.begin_resize(self.len() + bytes.len() / size)?;
        self.data.borrow_mut().extend_from_slice(bytes);
        Ok(())
    }
}

/// The array payload of `value`, if it is an `array` instance.
fn as_array(value: &Value) -> Option<Rc<ArrayInner>> {
    if !matches!(value.kind(), ValueKind::PyInstance(_)) {
        return None;
    }
    with_native_state::<ArrayState, _>(value, ARRAY_TP_NAME, |state| Ok(Rc::clone(&state.0))).ok()
}

/// The raw item bytes of an `array` instance (subclasses included): the
/// contiguous buffer native consumers (`hashlib`, `bz2`, `codecs`) read
/// without going through `memoryview`.
pub(crate) fn buffer_snapshot(value: &Value) -> Option<Vec<u8>> {
    as_array(value).map(|inner| inner.snapshot())
}

/// The live item storage of an `array` instance, for native in-place writers
/// such as `socket.recv_into` (the length never changes through it).
pub(crate) fn writable_buffer(value: &Value) -> Option<Rc<RefCell<Vec<u8>>>> {
    as_array(value).map(|inner| Rc::clone(&inner.data))
}

/// Whether `class` is a generation of the `array.array` type itself.
fn is_array_base_class(class: &Rc<RefCell<PyClass>>) -> bool {
    class.borrow().error_name == Some(ARRAY_TP_NAME)
}

/// `PyType_IsSubtype(class, ArrayType)`.
fn class_is_array_subtype(class: &Rc<RefCell<PyClass>>) -> bool {
    is_array_base_class(class) || c3_linearize_classes(class).iter().any(is_array_base_class)
}

fn instance_class(value: &Value) -> Option<Rc<RefCell<PyClass>>> {
    match value.kind() {
        ValueKind::PyInstance(inst) => Some(Rc::clone(&inst.borrow().class)),
        _ => None,
    }
}

/// `state->ArrayType` for the module that defined `value`'s type: slicing,
/// concatenation, repetition and copies always produce a plain `array`.
fn base_array_class(value: &Value) -> Result<Rc<RefCell<PyClass>>> {
    let class = instance_class(value)
        .ok_or_else(|| PyError::runtime("array: receiver is not an instance".into()))?;
    if is_array_base_class(&class) {
        return Ok(class);
    }
    c3_linearize_classes(&class)
        .into_iter()
        .find(is_array_base_class)
        .ok_or_else(|| PyError::runtime("array: receiver has no array base".into()))
}

/// A fresh instance of `class` holding `inner`.
fn new_instance(class: &Rc<RefCell<PyClass>>, inner: Rc<ArrayInner>) -> Result<Value> {
    let instance = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(class),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(&instance, ArrayState(inner))?;
    Ok(instance)
}

/// A plain `array` (the receiver's base type) holding `bytes`.
fn new_base_array(receiver: &Value, tc: TypeCode, bytes: Vec<u8>) -> Result<Value> {
    new_instance(&base_array_class(receiver)?, ArrayInner::new(tc, bytes))
}

/// `Vec<u8>` of `len` bytes, raising `MemoryError` instead of aborting when
/// the allocation is impossible.
fn try_alloc(len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| memory_error())?;
    Ok(out)
}

// ── Item encoding ────────────────────────────────────────────────────────────

/// One encoded item (at most 8 bytes).
struct Encoded {
    buf: [u8; 8],
    len: usize,
}

impl Encoded {
    fn from_slice(bytes: &[u8]) -> Self {
        let mut buf = [0u8; 8];
        buf[..bytes.len()].copy_from_slice(bytes);
        Self {
            buf,
            len: bytes.len(),
        }
    }

    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// An `__index__` result widened to `i128`; magnitudes beyond that only keep
/// their sign (every C range check rejects them anyway).
enum WideInt {
    Fits(i128),
    Huge { negative: bool },
}

impl WideInt {
    fn from_index_value(value: &Value) -> Option<Self> {
        match value.kind() {
            ValueKind::Int(n) => Some(Self::Fits(i128::from(n))),
            ValueKind::Bool(b) => Some(Self::Fits(i128::from(b))),
            ValueKind::BigInt(bi) => Some(match bi.to_i128() {
                Some(n) => Self::Fits(n),
                None => Self::Huge {
                    negative: bi.sign() == PyBigIntSign::Minus,
                },
            }),
            _ => None,
        }
    }

    fn is_negative(&self) -> bool {
        match *self {
            Self::Fits(n) => n < 0,
            Self::Huge { negative } => negative,
        }
    }

    fn in_range(&self, min: i128, max: i128) -> Option<i128> {
        match *self {
            Self::Fits(n) if (min..=max).contains(&n) => Some(n),
            _ => None,
        }
    }
}

/// `PyNumber_Index(value)` widened for the C range checks.
fn index_wide(interp: &mut Interpreter, value: &Value) -> Result<WideInt> {
    if let Some(wide) = WideInt::from_index_value(value) {
        return Ok(wide);
    }
    let Some(index) = interp.try_value_to_index(value)? else {
        return Err(type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            tp_name(value)
        )));
    };
    WideInt::from_index_value(&index).ok_or_else(|| {
        type_error(format!(
            "__index__ returned non-int (type {})",
            tp_name(&index)
        ))
    })
}

/// `PyLong_AsLong`.
fn as_c_long(wide: &WideInt) -> Result<i128> {
    wide.in_range(i128::from(c_long::MIN), i128::from(c_long::MAX))
        .ok_or_else(|| overflow_error("Python int too large to convert to C long"))
}

/// `PyLong_AsUnsignedLong`.
fn as_c_ulong(wide: &WideInt) -> Result<i128> {
    if wide.is_negative() {
        return Err(overflow_error(
            "can't convert negative value to unsigned int",
        ));
    }
    wide.in_range(0, i128::from(c_ulong::MAX))
        .ok_or_else(|| overflow_error("Python int too large to convert to C unsigned long"))
}

/// `PyLong_AsLongLong`.
fn as_c_longlong(wide: &WideInt) -> Result<i128> {
    wide.in_range(i128::from(c_longlong::MIN), i128::from(c_longlong::MAX))
        .ok_or_else(|| overflow_error("int too big to convert"))
}

/// `PyLong_AsUnsignedLongLong`.
fn as_c_ulonglong(wide: &WideInt) -> Result<i128> {
    if wide.is_negative() {
        return Err(overflow_error("can't convert negative int to unsigned"));
    }
    wide.in_range(0, i128::from(c_ulonglong::MAX))
        .ok_or_else(|| overflow_error("int too big to convert"))
}

/// Shared `"<what> is less than minimum"` / `"... greater than maximum"`
/// range check of the getargs converters and the array setters.
fn check_range(value: i128, min: i128, max: i128, what: &str) -> Result<i128> {
    if value < min {
        return Err(overflow_error(format!("{what} is less than minimum")));
    }
    if value > max {
        return Err(overflow_error(format!("{what} is greater than maximum")));
    }
    Ok(value)
}

/// getargs `'h'`: `PyLong_AsLong` then the `short` range.
fn getargs_short(wide: &WideInt) -> Result<i128> {
    check_range(
        as_c_long(wide)?,
        i128::from(c_short::MIN),
        i128::from(c_short::MAX),
        "signed short integer",
    )
}

/// getargs `'i'`: `PyLong_AsLong` then the `int` range.
fn getargs_int(wide: &WideInt) -> Result<i128> {
    check_range(
        as_c_long(wide)?,
        i128::from(c_int::MIN),
        i128::from(c_int::MAX),
        "signed integer",
    )
}

/// `PyFloat_AsDouble(value)`.
fn as_double(interp: &mut Interpreter, value: &Value) -> Result<f64> {
    match value.kind() {
        ValueKind::Float(f) => return Ok(f),
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
            return value_to_float(value, "array");
        }
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {}
        _ => {
            return Err(type_error(format!(
                "must be real number, not {}",
                tp_name(value)
            )));
        }
    }
    let backing = builtin_data_backing(value);
    if let Some(ref b) = backing
        && let ValueKind::Float(f) = b.kind()
    {
        return Ok(f);
    }
    if let Some(method) = lookup_value_special_method(value, "__float__").transpose()? {
        let result = invoke_class_method(interp, method, value.clone(), &[])?;
        let Some(normalized) = normalize_float_slot_result(&result) else {
            return Err(type_error(format!(
                "{}.__float__ returned non-float (type {})",
                tp_name(value),
                tp_name(&result)
            )));
        };
        let ValueKind::Float(f) = normalized.kind() else {
            unreachable!("normalize_float_slot_result returns a float");
        };
        return Ok(f);
    }
    if let Some(ref b) = backing
        && matches!(
            b.kind(),
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
        )
    {
        return value_to_float(b, "array");
    }
    if let Some(index) = interp.try_value_to_index(value)? {
        return value_to_float(&index, "array");
    }
    Err(type_error(format!(
        "must be real number, not {}",
        tp_name(value)
    )))
}

/// The text of a `str` (or `str` subclass) value.
fn str_text(value: &Value) -> Option<String> {
    if let ValueKind::Str(s) = value.kind() {
        return Some(s.to_string());
    }
    if matches!(value.kind(), ValueKind::PyInstance(_))
        && let Some(backing) = builtin_data_backing(value)
        && let ValueKind::Str(s) = backing.kind()
    {
        return Some(s.to_string());
    }
    None
}

/// `PyUnicode_AsWideChar`: the `wchar_t` units of `s` (UTF-16 code units on
/// Windows, code points elsewhere).
fn wide_units(s: &str) -> Vec<u32> {
    let mut out = Vec::with_capacity(s.len());
    let mut pos = 0usize;
    while let Some((cp, next)) = pyrust_core::cesu8_next_codepoint(s, pos) {
        pos = next;
        if WCHAR_SIZE == 2 && cp > 0xFFFF {
            let adjusted = cp - 0x10000;
            out.push(0xD800 + (adjusted >> 10));
            out.push(0xDC00 + (adjusted & 0x3FF));
        } else {
            out.push(cp);
        }
    }
    out
}

fn push_wide_unit(out: &mut Vec<u8>, unit: u32) {
    if WCHAR_SIZE == 2 {
        out.extend_from_slice(&(unit as u16).to_ne_bytes());
    } else {
        out.extend_from_slice(&unit.to_ne_bytes());
    }
}

/// The native encoding of `s` as `'u'` items.
fn encode_wide_str(s: &str) -> Vec<u8> {
    let units = wide_units(s);
    let mut out = Vec::with_capacity(units.len() * WCHAR_SIZE);
    for unit in units {
        push_wide_unit(&mut out, unit);
    }
    out
}

/// The `<type>_setitem` converter for one item of typecode `tc`.
fn encode_item(interp: &mut Interpreter, tc: TypeCode, value: &Value) -> Result<Encoded> {
    let bytes: Encoded = match tc {
        TypeCode::SignedChar => {
            let x = getargs_short(&index_wide(interp, value)?)?;
            let x = check_range(x, -128, 127, "signed char")?;
            Encoded::from_slice(&(x as i8).to_ne_bytes())
        }
        TypeCode::UnsignedChar => {
            let x = check_range(
                as_c_long(&index_wide(interp, value)?)?,
                0,
                i128::from(u8::MAX),
                "unsigned byte integer",
            )?;
            Encoded::from_slice(&(x as u8).to_ne_bytes())
        }
        TypeCode::SignedShort => {
            let x = getargs_short(&index_wide(interp, value)?)?;
            Encoded::from_slice(&(x as c_short).to_ne_bytes())
        }
        TypeCode::UnsignedShort => {
            let x = getargs_int(&index_wide(interp, value)?)?;
            let x = check_range(x, 0, i128::from(u16::MAX), "unsigned short")?;
            Encoded::from_slice(&(x as u16).to_ne_bytes())
        }
        TypeCode::SignedInt => {
            let x = getargs_int(&index_wide(interp, value)?)?;
            Encoded::from_slice(&(x as c_int).to_ne_bytes())
        }
        TypeCode::UnsignedInt => {
            let x = as_c_ulong(&index_wide(interp, value)?)?;
            if x > i128::from(c_uint::MAX) {
                return Err(overflow_error("unsigned int is greater than maximum"));
            }
            Encoded::from_slice(&(x as c_uint).to_ne_bytes())
        }
        TypeCode::SignedLong => {
            let x = as_c_long(&index_wide(interp, value)?)?;
            Encoded::from_slice(&(x as c_long).to_ne_bytes())
        }
        TypeCode::UnsignedLong => {
            let x = as_c_ulong(&index_wide(interp, value)?)?;
            Encoded::from_slice(&(x as c_ulong).to_ne_bytes())
        }
        TypeCode::SignedLongLong => {
            let x = as_c_longlong(&index_wide(interp, value)?)?;
            Encoded::from_slice(&(x as c_longlong).to_ne_bytes())
        }
        TypeCode::UnsignedLongLong => {
            let x = as_c_ulonglong(&index_wide(interp, value)?)?;
            Encoded::from_slice(&(x as c_ulonglong).to_ne_bytes())
        }
        TypeCode::Float => {
            let x = as_double(interp, value)?;
            Encoded::from_slice(&(x as f32).to_ne_bytes())
        }
        TypeCode::Double => {
            let x = as_double(interp, value)?;
            Encoded::from_slice(&x.to_ne_bytes())
        }
        TypeCode::Unicode => {
            let not_char = || type_error("array item must be unicode character");
            let text = str_text(value).ok_or_else(not_char)?;
            let units = wide_units(&text);
            if units.len() != 1 {
                return Err(not_char());
            }
            let mut out = Vec::with_capacity(WCHAR_SIZE);
            push_wide_unit(&mut out, units[0]);
            Encoded::from_slice(&out)
        }
    };
    Ok(bytes)
}

/// Whether converting `value` can run Python code (so CPython's double
/// `setitem` call in `ins1` is observable).
fn conversion_runs_python(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::PyInstance(_) | ValueKind::PyClass(_)
    )
}

// ── Item decoding ────────────────────────────────────────────────────────────

fn value_from_i128(n: i128) -> Value {
    match i64::try_from(n) {
        Ok(small) => Value::int(small),
        Err(_) => Value::bigint(PyBigInt::from(n)),
    }
}

/// `PyUnicode_FromOrdinal`.
fn char_from_ordinal(ordinal: i64) -> Result<Value> {
    if !(0..=0x10FFFF).contains(&ordinal) {
        return Err(value_error("chr() arg not in range(0x110000)"));
    }
    Ok(Value::string(pyrust_core::cesu8_encode_codepoint(
        ordinal as u32,
    )))
}

/// `'u'` item ordinal: `wchar_t` is unsigned 16-bit on Windows and signed
/// 32-bit elsewhere.
fn wide_ordinal(slot: &[u8]) -> i64 {
    if WCHAR_SIZE == 2 {
        i64::from(u16::from_ne_bytes([slot[0], slot[1]]))
    } else {
        i64::from(i32::from_ne_bytes([slot[0], slot[1], slot[2], slot[3]]))
    }
}

/// Integer value of a non-float, non-unicode item.
fn integer_item(tc: TypeCode, slot: &[u8]) -> i128 {
    match tc {
        TypeCode::SignedChar => i128::from(slot[0] as i8),
        TypeCode::UnsignedChar => i128::from(slot[0]),
        TypeCode::SignedShort => i128::from(c_short::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::UnsignedShort => i128::from(u16::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::SignedInt => i128::from(c_int::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::UnsignedInt => i128::from(c_uint::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::SignedLong => i128::from(c_long::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::UnsignedLong => i128::from(c_ulong::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::SignedLongLong => i128::from(c_longlong::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::UnsignedLongLong => {
            i128::from(c_ulonglong::from_ne_bytes(slot.try_into().unwrap()))
        }
        TypeCode::Unicode => i128::from(wide_ordinal(slot)),
        TypeCode::Float | TypeCode::Double => 0,
    }
}

/// The `<type>_getitem` converter for one item slot.
fn decode_item(tc: TypeCode, slot: &[u8]) -> Result<Value> {
    Ok(match tc {
        TypeCode::Float => Value::float(f64::from(f32::from_ne_bytes(slot.try_into().unwrap()))),
        TypeCode::Double => Value::float(f64::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::Unicode => return char_from_ordinal(wide_ordinal(slot)),
        _ => value_from_i128(integer_item(tc, slot)),
    })
}

/// `PyUnicode_FromWideChar`: Windows joins surrogate pairs; wide builds
/// reject units beyond U+10FFFF.
fn decode_wide_str(bytes: &[u8]) -> Result<String> {
    let mut out = String::with_capacity(bytes.len() / WCHAR_SIZE);
    if WCHAR_SIZE == 2 {
        let units: Vec<u32> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u32::from(u16::from_ne_bytes(*c)))
            .collect();
        let mut i = 0usize;
        while i < units.len() {
            let unit = units[i];
            i += 1;
            if (0xD800..=0xDBFF).contains(&unit)
                && let Some(&low) = units.get(i)
                && (0xDC00..=0xDFFF).contains(&low)
            {
                i += 1;
                let cp = 0x10000 + (((unit - 0xD800) << 10) | (low - 0xDC00));
                out.push_str(&pyrust_core::cesu8_encode_codepoint(cp));
                continue;
            }
            out.push_str(&pyrust_core::cesu8_encode_codepoint(unit));
        }
    } else {
        for chunk in bytes.as_chunks::<4>().0 {
            let cp = u32::from_ne_bytes(*chunk);
            if cp > 0x10FFFF {
                return Err(value_error(format!(
                    "character U+{cp:x} is not in range [U+0000; U+10ffff]"
                )));
            }
            out.push_str(&pyrust_core::cesu8_encode_codepoint(cp));
        }
    }
    Ok(out)
}

fn tolist(inner: &ArrayInner) -> Result<Value> {
    let size = inner.itemsize();
    let data = inner.data.borrow();
    let mut items = Vec::with_capacity(data.len() / size);
    for slot in data.chunks_exact(size) {
        items.push(decode_item(inner.tc, slot)?);
    }
    Ok(Value::list(items))
}

// ── Mutation primitives ──────────────────────────────────────────────────────

/// `ins1(self, where, v)`: validate, grow, then store (the converter runs
/// twice, as in CPython).
fn ins1(interp: &mut Interpreter, inner: &ArrayInner, position: i64, value: &Value) -> Result<()> {
    let mut encoded = encode_item(interp, inner.tc, value)?;
    let len = inner.len();
    inner.begin_resize(len + 1)?;
    let mut position = position;
    if position < 0 {
        position = (position + len as i64).max(0);
    }
    let position = (position as usize).min(len);
    if conversion_runs_python(value) {
        encoded = encode_item(interp, inner.tc, value)?;
    }
    let size = inner.itemsize();
    let mut data = inner.data.borrow_mut();
    let at = position * size;
    data.splice(at..at, encoded.as_slice().iter().copied());
    Ok(())
}

/// `array_iter_extend(self, it)`.
fn iter_extend(interp: &mut Interpreter, inner: &ArrayInner, iterator: &Value) -> Result<()> {
    loop {
        let item = match interp.call_next(iterator, None) {
            Ok(item) => item,
            Err(err) if is_stop_iteration_error(&err) => return Ok(()),
            Err(err) => return Err(err),
        };
        let end = inner.len() as i64;
        ins1(interp, inner, end, &item)?;
    }
}

/// `array_do_extend(state, self, bb)`.
fn do_extend(interp: &mut Interpreter, inner: &ArrayInner, other: &Value) -> Result<()> {
    let Some(other_inner) = as_array(other) else {
        let iterator = make_iterator(interp, other)?;
        return iter_extend(interp, inner, &iterator);
    };
    if other_inner.tc != inner.tc {
        return Err(type_error("can only extend with array of same kind"));
    }
    // Snapshot first: `other` may be `self`.
    let bytes = other_inner.snapshot();
    inner.extend_bytes(&bytes)
}

/// `frombytes(self, buffer)`; `is_self` marks a buffer exported by `self`.
fn frombytes_raw(inner: &ArrayInner, bytes: &[u8], is_self: bool) -> Result<()> {
    let size = inner.itemsize();
    if !bytes.len().is_multiple_of(size) {
        return Err(value_error("bytes length not a multiple of item size"));
    }
    if bytes.is_empty() {
        return Ok(());
    }
    if is_self {
        return Err(resize_while_exported());
    }
    inner.extend_bytes(bytes)
}

/// The `Py_buffer` argument of `frombytes`: any buffer exporter whose item
/// size is 1.
fn frombytes_source(
    interp: &mut Interpreter,
    value: &Value,
    inner: &Rc<ArrayInner>,
) -> Result<(Vec<u8>, bool)> {
    if let Some(other) = as_array(value) {
        if other.itemsize() != 1 {
            return Err(type_error("a bytes-like object is required"));
        }
        return Ok((other.snapshot(), Rc::ptr_eq(&other, inner)));
    }
    if pyrust_builtins::memoryview::is_memoryview(value)
        && !pyrust_builtins::memoryview::is_released(value)
        && let Some(itemsize) = pyrust_builtins::memoryview::getattr_checked(value, "itemsize")?
        && itemsize.as_int() != Some(1)
    {
        return Err(type_error("a bytes-like object is required"));
    }
    Ok((extract_buffer_bytes(interp, value)?, false))
}

/// `fromlist` into `inner`: grow first, convert in place, roll back on error.
fn fromlist(interp: &mut Interpreter, inner: &ArrayInner, list: &Value) -> Result<()> {
    let source = list_source(list).ok_or_else(|| type_error("arg must be list"))?;
    let count = list_len(&source);
    if count == 0 {
        return Ok(());
    }
    let size = inner.itemsize();
    let old_len = inner.len();
    inner.begin_resize(old_len + count)?;
    inner.data.borrow_mut().resize((old_len + count) * size, 0);
    let rollback = |inner: &ArrayInner| {
        inner.track_resize(old_len + count, old_len);
        inner.data.borrow_mut().truncate(old_len * size);
    };
    for i in 0..count {
        let Some(item) = list_item(&source, i) else {
            rollback(inner);
            return Err(PyError::named(
                "RuntimeError",
                "list changed size during iteration".to_string(),
            ));
        };
        match encode_item(interp, inner.tc, &item) {
            Ok(encoded) => {
                let at = (old_len + i) * size;
                inner.data.borrow_mut()[at..at + size].copy_from_slice(encoded.as_slice());
            }
            Err(err) => {
                rollback(inner);
                return Err(err);
            }
        }
        if list_len(&source) != count {
            rollback(inner);
            return Err(PyError::named(
                "RuntimeError",
                "list changed size during iteration".to_string(),
            ));
        }
    }
    Ok(())
}

// ── Builtin-sequence probes (`PyList_Check` & co. accept subclasses) ────────

/// The builtin payload of `value`: itself, or a subclass instance's backing.
fn builtin_payload(value: &Value) -> Value {
    if matches!(value.kind(), ValueKind::PyInstance(_))
        && let Some(backing) = builtin_data_backing(value)
    {
        return backing;
    }
    value.clone()
}

/// The `list` behind `value` (`PyList_Check`).
fn list_source(value: &Value) -> Option<Value> {
    let payload = builtin_payload(value);
    let is_list = matches!(payload.kind(), ValueKind::List(_));
    is_list.then_some(payload)
}

fn list_len(list: &Value) -> usize {
    match list.kind() {
        ValueKind::List(items) => items.len(),
        _ => 0,
    }
}

fn list_item(list: &Value, i: usize) -> Option<Value> {
    match list.kind() {
        ValueKind::List(items) => items.get(i).cloned(),
        _ => None,
    }
}

fn is_tuple(value: &Value) -> bool {
    matches!(builtin_payload(value).kind(), ValueKind::Tuple(_))
}

fn is_bytes(value: &Value) -> bool {
    matches!(builtin_payload(value).kind(), ValueKind::Bytes(_))
}

fn is_bytearray(value: &Value) -> bool {
    pyrust_builtins::bytearray::as_bytearray_rc(&builtin_payload(value)).is_some()
}

fn bytes_payload(value: &Value) -> Option<Vec<u8>> {
    let payload = builtin_payload(value);
    match payload.kind() {
        ValueKind::Bytes(bytes) => Some(bytes.to_vec()),
        _ => pyrust_builtins::bytearray::as_bytearray_snapshot(&payload),
    }
}

// ── Construction (`array_new`) ──────────────────────────────────────────────

/// The getargs `'C'` converter: a `str` of exactly one character.
fn parse_typecode_char(value: &Value, context: &str) -> Result<char> {
    if let Some(text) = str_text(value) {
        let mut chars = text.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Ok(c);
        }
        return Err(type_error(format!(
            "{context} must be a unicode character, not str"
        )));
    }
    let type_name = if value.is_none() {
        "None".to_string()
    } else {
        tp_name(value)
    };
    Err(type_error(format!(
        "{context} must be a unicode character, not {type_name}"
    )))
}

/// The body of `array_new` after argument parsing; also `make_array`.
fn construct(
    interp: &mut Interpreter,
    class: &Rc<RefCell<PyClass>>,
    c: char,
    initial: Option<&Value>,
) -> Result<Value> {
    interp.emit_audit_event(
        "array.__new__",
        &[
            Value::string(c.to_string()),
            initial.cloned().unwrap_or_else(Value::none),
        ],
    )?;
    if let Some(init) = initial
        && c != 'u'
    {
        if str_text(init).is_some() {
            return Err(type_error(format!(
                "cannot use a str to initialize an array with typecode '{c}'"
            )));
        }
        if as_array(init).is_some_and(|other| other.tc == TypeCode::Unicode) {
            return Err(type_error(format!(
                "cannot use a unicode array to initialize an array with typecode '{c}'"
            )));
        }
    }
    let mut initial = initial;
    let mut iterator = None;
    if let Some(init) = initial {
        let direct = list_source(init).is_some()
            || is_bytearray(init)
            || is_bytes(init)
            || is_tuple(init)
            || (c == 'u' && str_text(init).is_some())
            || as_array(init).is_some_and(|other| other.tc.as_char() == c);
        if !direct {
            iterator = Some(make_iterator(interp, init)?);
            initial = None;
        }
    }
    let Some(tc) = TypeCode::from_char(c) else {
        return Err(value_error(
            "bad typecode (must be b, B, u, h, H, i, I, l, L, q, Q, f or d)",
        ));
    };
    let inner = match initial {
        None => ArrayInner::new(tc, Vec::new()),
        Some(init) => construct_from_initial(interp, tc, init)?,
    };
    if let Some(iterator) = iterator {
        iter_extend(interp, &inner, &iterator)?;
    }
    new_instance(class, inner)
}

fn construct_from_initial(
    interp: &mut Interpreter,
    tc: TypeCode,
    init: &Value,
) -> Result<Rc<ArrayInner>> {
    if let Some(other) = as_array(init) {
        return Ok(ArrayInner::new(tc, other.snapshot()));
    }
    let sequence = list_source(init).or_else(|| {
        let payload = builtin_payload(init);
        let is_tuple = matches!(payload.kind(), ValueKind::Tuple(_));
        is_tuple.then_some(payload)
    });
    if let Some(sequence) = sequence {
        let items: Vec<Value> = match sequence.kind() {
            ValueKind::List(items) => items.to_vec(),
            ValueKind::Tuple(items) => items.to_vec(),
            _ => Vec::new(),
        };
        let size = tc.itemsize();
        let mut bytes = try_alloc(items.len() * size)?;
        for item in &items {
            bytes.extend_from_slice(encode_item(interp, tc, item)?.as_slice());
        }
        return Ok(ArrayInner::new(tc, bytes));
    }
    if let Some(bytes) = bytes_payload(init) {
        let inner = ArrayInner::new(tc, Vec::new());
        frombytes_raw(&inner, &bytes, false)?;
        return Ok(inner);
    }
    if let Some(text) = str_text(init) {
        return Ok(ArrayInner::new(tc, encode_wide_str(&text)));
    }
    Ok(ArrayInner::new(tc, Vec::new()))
}

/// `type == ArrayType || type->tp_init == ArrayType->tp_init`: keyword
/// arguments are only tolerated when a subclass defines its own `__init__`.
fn init_is_inherited(class: &Rc<RefCell<PyClass>>) -> bool {
    match lookup_class_attr_owner(class, "__init__") {
        None => true,
        Some(owner) => Rc::ptr_eq(&owner, &object_class_singleton()) || is_array_base_class(&owner),
    }
}

/// `array.__new__(cls, typecode[, initializer])`.
fn array_new(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let Some((cls_arg, rest)) = args.split_first() else {
        return Err(type_error("array.array.__new__(): not enough arguments"));
    };
    let ValueKind::PyClass(class) = cls_arg.value.kind() else {
        return Err(type_error(format!(
            "array.array.__new__(X): X is not a type object ({})",
            tp_name(&cls_arg.value)
        )));
    };
    let class = Rc::clone(class);
    if !class_is_array_subtype(&class) {
        let name = class.borrow().name.clone();
        return Err(type_error(format!(
            "array.array.__new__({name}): {name} is not a subtype of array.array"
        )));
    }
    if rest.iter().any(|arg| arg.name.is_some()) && init_is_inherited(&class) {
        return Err(type_error("array.array() takes no keyword arguments"));
    }
    let positional: Vec<&Value> = rest
        .iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
        .collect();
    match positional.len() {
        0 => {
            return Err(type_error("array() takes at least 1 argument (0 given)"));
        }
        1 | 2 => {}
        n => {
            return Err(type_error(format!(
                "array() takes at most 2 arguments ({n} given)"
            )));
        }
    }
    let c = parse_typecode_char(positional[0], "array() argument 1")?;
    construct(interp, &class, c, positional.get(1).copied())
}

// ── Argument parsing (CPython calling-convention wording) ───────────────────

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn has_keywords(args: &[ExpandedCallArg]) -> bool {
    args.iter().any(|arg| arg.name.is_some())
}

/// `self` of an `array` method descriptor call.
fn method_receiver<'a>(
    method: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, Rc<ArrayInner>, &'a [ExpandedCallArg])> {
    let Some((first, rest)) = args.split_first() else {
        return Err(type_error(format!(
            "unbound method array.{method}() needs an argument"
        )));
    };
    match as_array(&first.value) {
        Some(inner) => Ok((&first.value, inner, rest)),
        None => Err(type_error(format!(
            "descriptor '{method}' for '{ARRAY_TP_NAME}' objects doesn't apply to a '{}' object",
            tp_name(&first.value)
        ))),
    }
}

/// `self` of an `array` slot-wrapper call (`__len__`, `__getitem__`, ...).
fn slot_receiver<'a>(
    method: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, Rc<ArrayInner>, &'a [ExpandedCallArg])> {
    let Some((first, rest)) = args.split_first() else {
        return Err(type_error(format!(
            "descriptor '{method}' of '{ARRAY_TP_NAME}' object needs an argument"
        )));
    };
    match as_array(&first.value) {
        Some(inner) => Ok((&first.value, inner, rest)),
        None => Err(type_error(format!(
            "descriptor '{method}' requires a '{ARRAY_TP_NAME}' object but received a '{}'",
            tp_name(&first.value)
        ))),
    }
}

/// `METH_NOARGS`.
fn noargs(method: &str, rest: &[ExpandedCallArg]) -> Result<()> {
    if has_keywords(rest) {
        return Err(type_error(format!(
            "array.{method}() takes no keyword arguments"
        )));
    }
    if !rest.is_empty() {
        return Err(type_error(format!(
            "array.{method}() takes no arguments ({} given)",
            rest.len()
        )));
    }
    Ok(())
}

/// `METH_O`.
fn one_arg<'a>(owner: &str, method: &str, rest: &'a [ExpandedCallArg]) -> Result<&'a Value> {
    if has_keywords(rest) {
        return Err(type_error(format!(
            "{owner}.{method}() takes no keyword arguments"
        )));
    }
    if rest.len() != 1 {
        return Err(type_error(format!(
            "{owner}.{method}() takes exactly one argument ({} given)",
            rest.len()
        )));
    }
    Ok(&rest[0].value)
}

/// Argument Clinic positional-only `METH_FASTCALL` (`_PyArg_CheckPositional`).
fn fastcall<'a>(
    method: &str,
    rest: &'a [ExpandedCallArg],
    min: usize,
    max: usize,
) -> Result<&'a [ExpandedCallArg]> {
    if has_keywords(rest) {
        return Err(type_error(format!(
            "array.{method}() takes no keyword arguments"
        )));
    }
    let n = rest.len();
    if n < min {
        let qualifier = if min == max { "" } else { "at least " };
        return Err(type_error(format!(
            "{method} expected {qualifier}{min} argument{}, got {n}",
            plural(min)
        )));
    }
    if n > max {
        let qualifier = if min == max { "" } else { "at most " };
        return Err(type_error(format!(
            "{method} expected {qualifier}{max} argument{}, got {n}",
            plural(max)
        )));
    }
    Ok(rest)
}

/// Argument Clinic `defining_class` methods (`_PyArg_UnpackKeywords` with
/// `count` positional-only parameters).
fn clinic_positional<'a>(
    method: &str,
    rest: &'a [ExpandedCallArg],
    count: usize,
) -> Result<Vec<&'a Value>> {
    let nargs = rest.iter().filter(|arg| arg.name.is_none()).count();
    let nkwargs = rest.len() - nargs;
    if rest.len() > count {
        let keyword = if nargs == 0 { "keyword " } else { "" };
        return Err(type_error(format!(
            "{method}() takes at most {count} {keyword}argument{} ({} given)",
            plural(count),
            rest.len()
        )));
    }
    if nargs < count {
        return Err(type_error(format!(
            "{method}() takes exactly {count} positional argument{} ({nargs} given)",
            plural(count)
        )));
    }
    debug_assert_eq!(nkwargs, 0);
    Ok(rest.iter().map(|arg| &arg.value).collect())
}

/// Slot-wrapper argument count (`check_num_args`, or `PyArg_UnpackTuple`
/// with an empty name when `unpack` is set).
fn slot_args<'a>(
    method: &str,
    rest: &'a [ExpandedCallArg],
    count: usize,
    unpack: bool,
) -> Result<&'a [ExpandedCallArg]> {
    if has_keywords(rest) {
        return Err(type_error(format!(
            "wrapper {method}() takes no keyword arguments"
        )));
    }
    if rest.len() != count {
        let lead = if unpack { " " } else { "" };
        return Err(type_error(format!(
            "{lead}expected {count} argument{}, got {}",
            plural(count),
            rest.len()
        )));
    }
    Ok(rest)
}

/// The Clinic `Py_ssize_t` converter (`insert`, `pop`, `fromfile`).
fn ssize_arg(interp: &mut Interpreter, value: &Value) -> Result<i64> {
    match index_wide(interp, value)? {
        WideInt::Fits(n) if (i128::from(-SSIZE_MAX - 1)..=i128::from(SSIZE_MAX)).contains(&n) => {
            Ok(n as i64)
        }
        _ => Err(overflow_error(
            "Python int too large to convert to C ssize_t",
        )),
    }
}

/// `PyNumber_AsSsize_t(value, NULL)`: clamp on overflow.
fn ssize_clamped(wide: &WideInt) -> i64 {
    match *wide {
        WideInt::Fits(n) => n.clamp(i128::from(-SSIZE_MAX - 1), i128::from(SSIZE_MAX)) as i64,
        WideInt::Huge { negative: true } => -SSIZE_MAX - 1,
        WideInt::Huge { negative: false } => SSIZE_MAX,
    }
}

/// `_PyEval_SliceIndexNotNone` (`index()` bounds).
fn slice_index_arg(interp: &mut Interpreter, value: &Value) -> Result<i64> {
    let wide = match WideInt::from_index_value(value) {
        Some(wide) => wide,
        None => match interp.try_value_to_index(value)? {
            Some(index) => WideInt::from_index_value(&index).ok_or_else(|| {
                type_error(format!(
                    "__index__ returned non-int (type {})",
                    tp_name(&index)
                ))
            })?,
            None => {
                return Err(type_error(
                    "slice indices must be integers or have an __index__ method",
                ));
            }
        },
    };
    Ok(ssize_clamped(&wide))
}

/// `PyIndex_Check(key)` + `PyNumber_AsSsize_t(key, exc)`; `None` for
/// non-index keys.
fn subscript_index(
    interp: &mut Interpreter,
    key: &Value,
    overflow_kind: &'static str,
) -> Result<Option<i64>> {
    let wide = match WideInt::from_index_value(key) {
        Some(wide) => wide,
        None => match interp.try_value_to_index(key)? {
            Some(index) => WideInt::from_index_value(&index).ok_or_else(|| {
                type_error(format!(
                    "__index__ returned non-int (type {})",
                    tp_name(&index)
                ))
            })?,
            None => return Ok(None),
        },
    };
    match wide {
        WideInt::Fits(n) if (i128::from(-SSIZE_MAX - 1)..=i128::from(SSIZE_MAX)).contains(&n) => {
            Ok(Some(n as i64))
        }
        _ => Err(PyError::named(
            overflow_kind,
            format!("cannot fit '{}' into an index-sized integer", tp_name(key)),
        )),
    }
}

/// `PySlice_Unpack` + `PySlice_AdjustIndices`:
/// `(start, stop, step, slicelength)`.
fn slice_indices(
    interp: &mut Interpreter,
    key: &Value,
    len: usize,
) -> Result<Option<(i64, i64, i64, usize)>> {
    let Some((lo, hi, st)) = pyrust_builtins::slice::slice_fields(key) else {
        return Ok(None);
    };
    let mut resolve = |bound: Value| -> Result<Value> {
        if bound.is_none() || WideInt::from_index_value(&bound).is_some() {
            return Ok(bound);
        }
        interp.try_value_to_index(&bound)?.ok_or_else(|| {
            type_error("slice indices must be integers or None or have an __index__ method")
        })
    };
    let (lo, hi, st) = (resolve(lo)?, resolve(hi)?, resolve(st)?);
    let (start, stop, step) = pyrust_builtins::slice::compute_indices(len as i64, &lo, &hi, &st)?;
    let length = if step < 0 {
        if stop < start {
            ((start - stop - 1) / -step + 1) as usize
        } else {
            0
        }
    } else if start < stop {
        ((stop - start - 1) / step + 1) as usize
    } else {
        0
    };
    Ok(Some((start, stop, step, length)))
}

// ── Sequence protocol ───────────────────────────────────────────────────────

/// `PyObject_RichCompareBool(a, b, Py_EQ)` for decoded items.
fn items_equal(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    match (a.kind(), b.kind()) {
        (ValueKind::Int(x), ValueKind::Int(y)) => Ok(x == y),
        (ValueKind::Float(x), ValueKind::Float(y)) => Ok(x == y),
        (ValueKind::Str(x), ValueKind::Str(y)) => Ok(x == y),
        _ => {
            let result = interp.eval_binary(a.clone(), BinaryOp::Eq, b.clone())?;
            interp.truthy_value(&result)
        }
    }
}

/// Index of the first item equal to `value` in `[start, stop)`, re-reading
/// the length each step (the array may change during comparisons).
fn find_item(
    interp: &mut Interpreter,
    inner: &ArrayInner,
    value: &Value,
    start: usize,
    stop: usize,
) -> Result<Option<usize>> {
    let mut i = start;
    while i < stop && i < inner.len() {
        let item = inner.item(i)?;
        if items_equal(interp, &item, value)? {
            return Ok(Some(i));
        }
        i += 1;
    }
    Ok(None)
}

fn subscript(
    interp: &mut Interpreter,
    this: &Value,
    inner: &ArrayInner,
    key: &Value,
) -> Result<Value> {
    if let Some(i) = subscript_index(interp, key, "IndexError")? {
        let len = inner.len() as i64;
        let i = if i < 0 { i + len } else { i };
        if i < 0 || i >= len {
            return Err(index_error("array index out of range"));
        }
        return inner.item(i as usize);
    }
    if let Some((start, _stop, step, length)) = slice_indices(interp, key, inner.len())? {
        let size = inner.itemsize();
        let data = inner.data.borrow();
        let bytes = if length == 0 {
            Vec::new()
        } else if step == 1 {
            let at = start as usize * size;
            data[at..at + length * size].to_vec()
        } else {
            let mut out = Vec::with_capacity(length * size);
            let mut cur = start;
            for _ in 0..length {
                let at = cur as usize * size;
                out.extend_from_slice(&data[at..at + size]);
                // A huge step (clamped BigInt) selects one item; the final
                // advance must not overflow.
                cur = cur.wrapping_add(step);
            }
            out
        };
        drop(data);
        return new_base_array(this, inner.tc, bytes);
    }
    Err(type_error("array indices must be integers"))
}

/// `array_ass_subscr(self, key, value)`; `value == None` deletes.
fn ass_subscript(
    interp: &mut Interpreter,
    inner: &Rc<ArrayInner>,
    key: &Value,
    value: Option<&Value>,
) -> Result<()> {
    let len = inner.len();
    let (mut start, mut stop, mut step, length);
    if let Some(i) = subscript_index(interp, key, "IndexError")? {
        let i = if i < 0 { i + len as i64 } else { i };
        if i < 0 || i >= len as i64 {
            return Err(index_error("array assignment index out of range"));
        }
        match value {
            Some(value) => {
                let encoded = encode_item(interp, inner.tc, value)?;
                let size = inner.itemsize();
                let at = i as usize * size;
                let mut data = inner.data.borrow_mut();
                if at + size <= data.len() {
                    data[at..at + size].copy_from_slice(encoded.as_slice());
                }
                return Ok(());
            }
            None => {
                (start, stop, step, length) = (i, i + 1, 1, 1);
            }
        }
    } else if let Some(indices) = slice_indices(interp, key, len)? {
        (start, stop, step, length) = indices;
    } else {
        return Err(type_error("array indices must be integers"));
    }
    let source: Vec<u8> = match value {
        None => Vec::new(),
        Some(value) => match as_array(value) {
            Some(other) => {
                if !Rc::ptr_eq(&other, inner) && other.tc != inner.tc {
                    return Err(bad_argument());
                }
                other.snapshot()
            }
            None => {
                return Err(type_error(format!(
                    "can only assign array (not \"{}\") to array slice",
                    tp_name(value)
                )));
            }
        },
    };
    let size = inner.itemsize();
    let needed = source.len() / size;
    if (step > 0 && stop < start) || (step < 0 && stop > start) {
        stop = start;
    }
    if (needed == 0 || length != needed) && inner.exports.get() > 0 {
        return Err(resize_while_exported());
    }
    if step == 1 {
        let (lo, hi) = (start as usize, stop as usize);
        if length != needed {
            inner.begin_resize(len + needed - length)?;
        }
        inner
            .data
            .borrow_mut()
            .splice(lo * size..hi * size, source.iter().copied());
        return Ok(());
    }
    if needed == 0 {
        if step < 0 {
            // CPython evaluates this in C `Py_ssize_t` (wrapping in practice);
            // for an empty selection with a huge step the result is never read,
            // so wrap instead of overflowing.
            stop = start + 1;
            start = stop
                .wrapping_add(step.wrapping_mul(length as i64 - 1))
                .wrapping_sub(1);
            step = -step;
        }
        let _ = stop;
        let doomed: Vec<usize> = (0..length)
            .map(|k| (start + step * k as i64) as usize)
            .collect();
        inner.begin_resize(len - length)?;
        let mut data = inner.data.borrow_mut();
        let mut kept = Vec::with_capacity((len - length) * size);
        let mut next = doomed.iter().peekable();
        for (index, slot) in data.chunks_exact(size).enumerate() {
            if next.peek() == Some(&&index) {
                next.next();
                continue;
            }
            kept.extend_from_slice(slot);
        }
        *data = kept;
        return Ok(());
    }
    if needed != length {
        return Err(value_error(format!(
            "attempt to assign array of size {needed} to extended slice of size {length}"
        )));
    }
    let mut data = inner.data.borrow_mut();
    let mut cur = start;
    for k in 0..length {
        let at = cur as usize * size;
        data[at..at + size].copy_from_slice(&source[k * size..(k + 1) * size]);
        cur = cur.wrapping_add(step);
    }
    Ok(())
}

/// Whether `other`'s type defines the reflected `name` slot, so the binary
/// operator must defer to it (CPython tries `nb_*` before `sq_concat` /
/// `sq_repeat`).
fn defers_to_reflected(other: &Value, name: &str) -> Result<bool> {
    if !matches!(other.kind(), ValueKind::PyInstance(_)) {
        return Ok(false);
    }
    Ok(lookup_value_special_method(other, name)
        .transpose()?
        .is_some())
}

/// `sequence_repeat`'s count: `None` when `n` is not an integer at all.
fn repeat_count(interp: &mut Interpreter, n: &Value) -> Result<Option<i64>> {
    match subscript_index(interp, n, "OverflowError")? {
        Some(count) => Ok(Some(count)),
        None => Ok(None),
    }
}

fn non_int_multiplier(n: &Value) -> PyError {
    type_error(format!(
        "can't multiply sequence by non-int of type '{}'",
        tp_name(n)
    ))
}

fn repeat_bytes(bytes: &[u8], count: i64) -> Result<Vec<u8>> {
    let count = count.max(0) as usize;
    let total = bytes.len().checked_mul(count).ok_or_else(memory_error)?;
    if total > SSIZE_MAX as usize {
        return Err(memory_error());
    }
    let mut out = try_alloc(total)?;
    for _ in 0..count {
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum CompareOp {
    Lt,
    Le,
    Eq,
    Ne,
    Gt,
    Ge,
}

impl CompareOp {
    fn binary_op(self) -> BinaryOp {
        match self {
            Self::Lt => BinaryOp::Lt,
            Self::Le => BinaryOp::Le,
            Self::Eq => BinaryOp::Eq,
            Self::Ne => BinaryOp::Ne,
            Self::Gt => BinaryOp::Gt,
            Self::Ge => BinaryOp::Ge,
        }
    }

    fn holds_for(self, ordering: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self {
            Self::Lt => ordering == Less,
            Self::Le => ordering != Greater,
            Self::Eq => ordering == Equal,
            Self::Ne => ordering != Equal,
            Self::Gt => ordering == Greater,
            Self::Ge => ordering != Less,
        }
    }
}

/// `array_richcompare(v, w, op)`.
fn richcompare(interp: &mut Interpreter, v: &Value, w: &Value, op: CompareOp) -> Result<Value> {
    let (Some(va), Some(wa)) = (as_array(v), as_array(w)) else {
        return Ok(Value::not_implemented());
    };
    if va.len() != wa.len() && matches!(op, CompareOp::Eq | CompareOp::Ne) {
        return Ok(Value::bool_(matches!(op, CompareOp::Ne)));
    }
    if va.tc == wa.tc && !matches!(va.tc, TypeCode::Float | TypeCode::Double) {
        // `compareitems` fast path: raw native values.
        let size = va.itemsize();
        let ordering = {
            let (a, b) = (va.data.borrow(), wa.data.borrow());
            a.chunks_exact(size)
                .zip(b.chunks_exact(size))
                .map(|(x, y)| integer_item(va.tc, x).cmp(&integer_item(va.tc, y)))
                .find(|ordering| ordering.is_ne())
        };
        let ordering = ordering.unwrap_or_else(|| va.len().cmp(&wa.len()));
        return Ok(Value::bool_(op.holds_for(ordering)));
    }
    let mut i = 0usize;
    while i < va.len() && i < wa.len() {
        let vi = va.item(i)?;
        let wi = wa.item(i)?;
        if !items_equal(interp, &vi, &wi)? {
            return match op {
                CompareOp::Eq => Ok(Value::bool_(false)),
                CompareOp::Ne => Ok(Value::bool_(true)),
                _ => interp.eval_binary(vi, op.binary_op(), wi),
            };
        }
        i += 1;
    }
    let result = match op {
        CompareOp::Eq => true,
        CompareOp::Ne => false,
        _ => op.holds_for(va.len().cmp(&wa.len())),
    };
    Ok(Value::bool_(result))
}

fn compare_method(
    interp: &mut Interpreter,
    method: &str,
    args: &[ExpandedCallArg],
    op: CompareOp,
) -> Result<Value> {
    let (this, _inner, rest) = slot_receiver(method, args)?;
    let rest = slot_args(method, rest, 1, false)?;
    richcompare(interp, this, &rest[0].value, op)
}

fn repr(interp: &mut Interpreter, this: &Value, inner: &ArrayInner) -> Result<Value> {
    let name = instance_class(this)
        .map(|class| class.borrow().name.clone())
        .unwrap_or_else(|| "array".to_string());
    let c = inner.tc.as_char();
    if inner.len() == 0 {
        return Ok(Value::string(format!("{name}('{c}')")));
    }
    let contents = if inner.tc == TypeCode::Unicode {
        Value::string(decode_wide_str(&inner.data.borrow())?)
    } else {
        tolist(inner)?
    };
    let rendered = render_value_repr(interp, &contents)?;
    Ok(Value::string(format!("{name}('{c}', {rendered})")))
}

/// `__sizeof__`: `tp_basicsize` (the 8-word `arrayobject`, plus 8 bytes per
/// `__slots__` entry a subclass adds) plus the allocated item storage.
fn sizeof(this: &Value, inner: &ArrayInner) -> usize {
    let word = size_of::<usize>();
    let mut basicsize = 8 * word;
    if let Some(class) = instance_class(this) {
        // The MRO starts with the class itself.
        for cls in c3_linearize_classes(&class) {
            if is_array_base_class(&cls) {
                break;
            }
            if let Some(slots) = &cls.borrow().slots {
                basicsize += word
                    * slots
                        .iter()
                        .filter(|slot| !matches!(slot.as_str(), "__dict__" | "__weakref__"))
                        .count();
            }
        }
    }
    basicsize + inner.allocated.get() * inner.itemsize()
}

fn reduce_ex(
    interp: &mut Interpreter,
    this: &Value,
    inner: &ArrayInner,
    value: &Value,
) -> Result<Value> {
    let protocol_value = builtin_payload(value);
    let protocol = match protocol_value.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        ValueKind::BigInt(_) => {
            return Err(overflow_error("Python int too large to convert to C long"));
        }
        _ => {
            return Err(type_error("__reduce_ex__ argument should be an integer"));
        }
    };
    if i128::from(protocol) > i128::from(c_long::MAX)
        || i128::from(protocol) < i128::from(c_long::MIN)
    {
        return Err(overflow_error("Python int too large to convert to C long"));
    }
    let dict = match interp.get_attr(this, "__dict__") {
        Ok(dict) => dict,
        Err(err) if err.class_name_is("AttributeError") => Value::none(),
        Err(err) => return Err(err),
    };
    let class = instance_class(this)
        .map(Value::py_class)
        .unwrap_or_else(Value::none);
    let typecode = Value::string(inner.tc.as_char().to_string());
    let mformat = inner.tc.mformat_code();
    if mformat == UNKNOWN_FORMAT || protocol < 3 {
        let items = tolist(inner)?;
        return Ok(Value::tuple(vec![
            class,
            Value::tuple(vec![typecode, items]),
            dict,
        ]));
    }
    let module = interp.load_module("array")?;
    let reconstructor = interp.get_attr(&module, "_array_reconstructor")?;
    Ok(Value::tuple(vec![
        reconstructor,
        Value::tuple(vec![
            class,
            typecode,
            Value::int(mformat),
            Value::bytes(inner.snapshot()),
        ]),
        dict,
    ]))
}

// ── `_array_reconstructor` ──────────────────────────────────────────────────

/// `_PyLong_AsInt` (Clinic `int` converter).
fn c_int_arg(interp: &mut Interpreter, value: &Value) -> Result<i64> {
    match index_wide(interp, value)? {
        WideInt::Fits(n) if (i128::from(c_int::MIN)..=i128::from(c_int::MAX)).contains(&n) => {
            Ok(n as i64)
        }
        _ => Err(overflow_error("Python int too large to convert to C int")),
    }
}

/// `tp_name` of a class object.
fn class_tp_name(class: &Rc<RefCell<PyClass>>) -> String {
    let class = class.borrow();
    class
        .error_name
        .map(str::to_string)
        .unwrap_or_else(|| class.name.clone())
}

fn decode_str(interp: &mut Interpreter, items: &Value, encoding: &str) -> Result<Value> {
    let decode = interp.get_attr(items, "decode")?;
    interp.call_function_expanded(
        decode,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(encoding),
        }],
    )
}

fn array_reconstructor(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if has_keywords(args) {
        return Err(type_error(
            "array._array_reconstructor() takes no keyword arguments",
        ));
    }
    if args.len() != 4 {
        return Err(type_error(format!(
            "_array_reconstructor expected 4 arguments, got {}",
            args.len()
        )));
    }
    let arraytype = &args[0].value;
    let c = parse_typecode_char(&args[1].value, "_array_reconstructor() argument 2")?;
    let mformat_code = c_int_arg(interp, &args[2].value)?;
    let items = &args[3].value;

    let ValueKind::PyClass(class) = arraytype.kind() else {
        return Err(type_error(format!(
            "first argument must be a type object, not {}",
            tp_name(arraytype)
        )));
    };
    let class = Rc::clone(class);
    if !class_is_array_subtype(&class) {
        return Err(type_error(format!(
            "{} is not a subtype of {ARRAY_TP_NAME}",
            class_tp_name(&class)
        )));
    }
    let Some(tc) = TypeCode::from_char(c) else {
        return Err(value_error("second argument must be a valid type code"));
    };
    if !(0..=MACHINE_FORMAT_CODE_MAX).contains(&mformat_code) {
        return Err(value_error(
            "third argument must be a valid machine format code.",
        ));
    }
    if !is_bytes(items) {
        return Err(type_error(format!(
            "fourth argument should be bytes, not {}",
            tp_name(items)
        )));
    }
    if mformat_code == tc.mformat_code() {
        return construct(interp, &class, c, Some(items));
    }
    let raw = bytes_payload(items).unwrap_or_default();
    let (size, is_signed, is_big_endian) = MFORMAT_DESCRIPTORS[mformat_code as usize];
    if !raw.len().is_multiple_of(size) {
        return Err(value_error("string length not a multiple of item size"));
    }
    let mut typecode = c;
    let converted = match mformat_code {
        IEEE_754_FLOAT_LE | IEEE_754_FLOAT_BE => Value::list(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| {
                    let bytes = *chunk;
                    let x = if mformat_code == IEEE_754_FLOAT_LE {
                        f32::from_le_bytes(bytes)
                    } else {
                        f32::from_be_bytes(bytes)
                    };
                    Value::float(f64::from(x))
                })
                .collect(),
        ),
        IEEE_754_DOUBLE_LE | IEEE_754_DOUBLE_BE => Value::list(
            raw.as_chunks::<8>()
                .0
                .iter()
                .map(|chunk| {
                    let bytes = *chunk;
                    Value::float(if mformat_code == IEEE_754_DOUBLE_LE {
                        f64::from_le_bytes(bytes)
                    } else {
                        f64::from_be_bytes(bytes)
                    })
                })
                .collect(),
        ),
        UTF16_LE | UTF16_BE => {
            let encoding = if mformat_code == UTF16_LE {
                "utf-16-le"
            } else {
                "utf-16-be"
            };
            decode_str(interp, items, encoding)?
        }
        UTF32_LE | UTF32_BE => {
            let encoding = if mformat_code == UTF32_LE {
                "utf-32-le"
            } else {
                "utf-32-be"
            };
            decode_str(interp, items, encoding)?
        }
        UNSIGNED_INT8..=SIGNED_INT64_BE => {
            // Re-pack with the integer typecode that fits the pickled width
            // (the last matching descriptor wins, as in CPython).
            for candidate in TypeCode::ALL {
                if candidate.integer_signedness() == Some(is_signed) && candidate.itemsize() == size
                {
                    typecode = candidate.as_char();
                }
            }
            Value::list(
                raw.chunks_exact(size)
                    .map(|chunk| {
                        let mut wide = [0u8; 16];
                        let negative = is_signed
                            && (if is_big_endian {
                                chunk[0]
                            } else {
                                chunk[size - 1]
                            }) & 0x80
                                != 0;
                        if negative {
                            wide = [0xFF; 16];
                        }
                        for (k, &byte) in chunk.iter().enumerate() {
                            let position = if is_big_endian { size - 1 - k } else { k };
                            wide[position] = byte;
                        }
                        value_from_i128(i128::from_le_bytes(wide))
                    })
                    .collect(),
            )
        }
        _ => return Err(bad_argument()),
    };
    construct(interp, &class, typecode, Some(&converted))
}

// ── Iterator ─────────────────────────────────────────────────────────────────

/// `arrayiterobject`: `None` once exhausted (CPython drops `ao`).
struct ArrayIterState {
    source: Option<(Value, Rc<ArrayInner>)>,
    index: usize,
}

#[derive(Default)]
struct ArrayRegistry {
    /// `array.arrayiterator`: the type of `iter(array)`, not exported by the
    /// module.
    iterator: Option<Rc<RefCell<PyClass>>>,
}

// `gil_local!` justification:
// (a) Why: the hidden `arrayiterator` class is a `!Send` `Rc` handle shared by
//     every Python thread of one interpreter under the GIL.
// (b) Isolation: independent interpreters on separate OS threads each get
//     their own registry, so classes never leak between them.
pyrust_core::gil_local! {
    static REGISTRY: RefCell<ArrayRegistry> = RefCell::new(ArrayRegistry::default());
}

fn iterator_class(interp: &mut Interpreter) -> Result<Rc<RefCell<PyClass>>> {
    if let Some(class) = REGISTRY.with(|registry| registry.borrow().iterator.clone()) {
        return Ok(class);
    }
    interp.load_module("array")?;
    REGISTRY
        .with(|registry| registry.borrow().iterator.clone())
        .ok_or_else(|| PyError::runtime("array: arrayiterator is not initialised".into()))
}

fn new_iterator(interp: &mut Interpreter, this: &Value, inner: Rc<ArrayInner>) -> Result<Value> {
    let class = iterator_class(interp)?;
    let iterator = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &iterator,
        ArrayIterState {
            source: Some((this.clone(), inner)),
            index: 0,
        },
    )?;
    Ok(iterator)
}

fn iterator_receiver<'a>(
    method: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, &'a [ExpandedCallArg])> {
    let Some((first, rest)) = args.split_first() else {
        return Err(type_error(format!(
            "descriptor '{method}' of '{ITER_TP_NAME}' object needs an argument"
        )));
    };
    let is_iterator = matches!(first.value.kind(), ValueKind::PyInstance(_))
        && with_native_state::<ArrayIterState, _>(&first.value, ITER_TP_NAME, |_| Ok(())).is_ok();
    if !is_iterator {
        return Err(type_error(format!(
            "descriptor '{method}' for '{ITER_TP_NAME}' objects doesn't apply to a '{}' object",
            tp_name(&first.value)
        )));
    }
    Ok((&first.value, rest))
}

// ── Module wiring ───────────────────────────────────────────────────────────

/// Post-import wiring (replaces the former `@inject` hook): give the types
/// their CPython identity, hide `arrayiterator`, add `ArrayType`, put the
/// namespace in CPython's order and register `array` as a
/// `collections.abc.MutableSequence`.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let take = |name: &str| -> Result<Value> {
        module
            .borrow_mut()
            .attrs
            .shift_remove(name)
            .ok_or_else(|| PyError::runtime(format!("array: missing {name}")))
    };
    let reconstructor = take("_array_reconstructor")?;
    let array_cls = take("array")?;
    let typecodes = take("typecodes")?;
    let iterator_cls = take("arrayiterator")?;

    let (ValueKind::PyClass(array_rc), ValueKind::PyClass(iter_rc)) =
        (array_cls.kind(), iterator_cls.kind())
    else {
        return Err(PyError::runtime("array: classes not initialised".into()));
    };
    {
        let mut class = array_rc.borrow_mut();
        class.error_name = Some(ARRAY_TP_NAME);
        // No per-instance `__dict__`, but weak references are supported
        // (CPython's `tp_weaklistoffset`).
        class.slots = Some(std::iter::once("__weakref__".to_string()).collect());
        class
            .attrs
            .insert("__doc__".to_string(), Value::string(ARRAY_DOC));
        for (property, doc) in [
            (
                "typecode",
                "the typecode character used to create the array",
            ),
            ("itemsize", "the size, in bytes, of one array item"),
        ] {
            let Some(fields) = class
                .attrs
                .get(property)
                .and_then(pyrust_builtins::property::as_property)
            else {
                continue;
            };
            // Getset descriptors refuse deletion exactly like assignment.
            let readonly = pyrust_builtins::property::property_with_doc(
                (*fields.0).clone(),
                (*fields.1).clone(),
                (*fields.1).clone(),
                Some(Value::string(doc)),
            );
            class.attrs.insert(property.to_string(), readonly);
        }
    }
    {
        let mut class = iter_rc.borrow_mut();
        class.error_name = Some(ITER_TP_NAME);
        class.non_subclassable_name = Some(ITER_TP_NAME);
        class.slots = Some(indexmap::IndexSet::new());
        class.attrs.shift_remove("__doc__");
    }
    REGISTRY.with(|registry| registry.borrow_mut().iterator = Some(Rc::clone(iter_rc)));

    {
        let mut module = module.borrow_mut();
        module
            .attrs
            .insert("_array_reconstructor".to_string(), reconstructor);
        module
            .attrs
            .insert("ArrayType".to_string(), array_cls.clone());
        module.attrs.insert("array".to_string(), array_cls.clone());
        module.attrs.insert("typecodes".to_string(), typecodes);
    }

    // CPython's `array_modexec` imports `collections.abc` and calls
    // `MutableSequence.register(array)`.
    interp.load_module("collections.abc")?;
    super::collections_abc::register_virtual_mutable_sequence(interp, array_rc)
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "typecodes" => Value::string(TYPECODES),
    }

    /// `_array_reconstructor(arraytype, typecode, mformat_code, items)`.
    fn _array_reconstructor(args) -> Result<Value> {
        array_reconstructor(_interp, args)
    }

    class array {
        constants {
            "__hash__" => Value::none(),
        }

        fn __new__(args) -> Result<Value> {
            array_new(_interp, args)
        }

        // Properties.

        #[getter("typecode")]
        fn get_typecode(args) -> Result<Value> {
            let (_this, inner, _rest) = method_receiver("typecode", args)?;
            Ok(Value::string(inner.tc.as_char().to_string()))
        }

        #[setter("typecode")]
        fn set_typecode(_args) -> Result<Value> {
            Err(PyError::named(
                "AttributeError",
                "attribute 'typecode' of 'array.array' objects is not writable".to_string(),
            ))
        }

        #[getter("itemsize")]
        fn get_itemsize(args) -> Result<Value> {
            let (_this, inner, _rest) = method_receiver("itemsize", args)?;
            Ok(Value::int(inner.itemsize() as i64))
        }

        #[setter("itemsize")]
        fn set_itemsize(_args) -> Result<Value> {
            Err(PyError::named(
                "AttributeError",
                "attribute 'itemsize' of 'array.array' objects is not writable".to_string(),
            ))
        }

        // Methods.

        fn append(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("append", args)?;
            let value = one_arg("array", "append", rest)?;
            let end = inner.len() as i64;
            ins1(_interp, &inner, end, value)?;
            Ok(Value::none())
        }

        fn buffer_info(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("buffer_info", args)?;
            noargs("buffer_info", rest)?;
            let address = if inner.allocated.get() == 0 {
                0
            } else {
                inner.data.borrow().as_ptr() as usize
            };
            let address = match i64::try_from(address) {
                Ok(small) => Value::int(small),
                Err(_) => Value::bigint(PyBigInt::from(address as u64)),
            };
            Ok(Value::tuple(vec![address, Value::int(inner.len() as i64)]))
        }

        fn byteswap(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("byteswap", args)?;
            noargs("byteswap", rest)?;
            let size = inner.itemsize();
            if size > 1 {
                for slot in inner.data.borrow_mut().chunks_exact_mut(size) {
                    slot.reverse();
                }
            }
            Ok(Value::none())
        }

        fn __copy__(args) -> Result<Value> {
            let (this, inner, rest) = method_receiver("__copy__", args)?;
            noargs("__copy__", rest)?;
            new_base_array(this, inner.tc, inner.snapshot())
        }

        fn count(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("count", args)?;
            let value = one_arg("array", "count", rest)?;
            let mut total = 0i64;
            let mut i = 0usize;
            while i < inner.len() {
                let item = inner.item(i)?;
                if items_equal(_interp, &item, value)? {
                    total += 1;
                }
                i += 1;
            }
            Ok(Value::int(total))
        }

        fn __deepcopy__(args) -> Result<Value> {
            let (this, inner, rest) = method_receiver("__deepcopy__", args)?;
            one_arg("array", "__deepcopy__", rest)?;
            new_base_array(this, inner.tc, inner.snapshot())
        }

        fn extend(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("extend", args)?;
            let params = clinic_positional("extend", rest, 1)?;
            do_extend(_interp, &inner, params[0])?;
            Ok(Value::none())
        }

        fn fromfile(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("fromfile", args)?;
            let params = clinic_positional("fromfile", rest, 2)?;
            let n = ssize_arg(_interp, params[1])?;
            if n < 0 {
                return Err(value_error("negative count"));
            }
            let size = inner.itemsize() as i64;
            if n > SSIZE_MAX / size {
                return Err(memory_error());
            }
            let nbytes = n * size;
            let read = _interp.get_attr(params[0], "read")?;
            let data = _interp.call_function_expanded(
                read,
                &[ExpandedCallArg { name: None, value: Value::int(nbytes) }],
            )?;
            if !is_bytes(&data) {
                return Err(type_error("read() didn't return bytes"));
            }
            let bytes = bytes_payload(&data).unwrap_or_default();
            let not_enough = bytes.len() as i64 != nbytes;
            frombytes_raw(&inner, &bytes, false)?;
            if not_enough {
                return Err(PyError::named(
                    "EOFError",
                    "read() didn't return enough bytes".to_string(),
                ));
            }
            Ok(Value::none())
        }

        fn fromlist(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("fromlist", args)?;
            let list = one_arg("array", "fromlist", rest)?;
            fromlist(_interp, &inner, list)?;
            Ok(Value::none())
        }

        fn frombytes(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("frombytes", args)?;
            let buffer = one_arg("array", "frombytes", rest)?;
            let (bytes, is_self) = frombytes_source(_interp, buffer, &inner)?;
            frombytes_raw(&inner, &bytes, is_self)?;
            Ok(Value::none())
        }

        fn fromunicode(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("fromunicode", args)?;
            let ustr = one_arg("array", "fromunicode", rest)?;
            let Some(text) = str_text(ustr) else {
                return Err(type_error(format!(
                    "fromunicode() argument must be str, not {}",
                    tp_name(ustr)
                )));
            };
            if inner.tc != TypeCode::Unicode {
                return Err(value_error(
                    "fromunicode() may only be called on unicode type arrays",
                ));
            }
            let bytes = encode_wide_str(&text);
            if !bytes.is_empty() {
                inner.extend_bytes(&bytes)?;
            }
            Ok(Value::none())
        }

        fn index(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("index", args)?;
            let params = fastcall("index", rest, 1, 3)?;
            let value = &params[0].value;
            let len = inner.len() as i64;
            let mut start = match params.get(1) {
                Some(arg) => slice_index_arg(_interp, &arg.value)?,
                None => 0,
            };
            let mut stop = match params.get(2) {
                Some(arg) => slice_index_arg(_interp, &arg.value)?,
                None => SSIZE_MAX,
            };
            if start < 0 {
                start = (start + len).max(0);
            }
            if stop < 0 {
                stop += len;
            }
            if stop > start
                && let Some(found) =
                    find_item(_interp, &inner, value, start as usize, stop as usize)?
            {
                return Ok(Value::int(found as i64));
            }
            Err(value_error("array.index(x): x not in array"))
        }

        fn insert(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("insert", args)?;
            let params = fastcall("insert", rest, 2, 2)?;
            let position = ssize_arg(_interp, &params[0].value)?;
            ins1(_interp, &inner, position, &params[1].value)?;
            Ok(Value::none())
        }

        fn pop(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("pop", args)?;
            let params = fastcall("pop", rest, 0, 1)?;
            let mut i = match params.first() {
                Some(arg) => ssize_arg(_interp, &arg.value)?,
                None => -1,
            };
            let len = inner.len() as i64;
            if len == 0 {
                return Err(index_error("pop from empty array"));
            }
            if i < 0 {
                i += len;
            }
            if i < 0 || i >= len {
                return Err(index_error("pop index out of range"));
            }
            let item = inner.item(i as usize)?;
            inner.del_slice(i as usize, i as usize + 1)?;
            Ok(item)
        }

        fn __reduce_ex__(args) -> Result<Value> {
            let (this, inner, rest) = method_receiver("__reduce_ex__", args)?;
            let params = clinic_positional("__reduce_ex__", rest, 1)?;
            reduce_ex(_interp, this, &inner, params[0])
        }

        fn remove(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("remove", args)?;
            let value = one_arg("array", "remove", rest)?;
            match find_item(_interp, &inner, value, 0, usize::MAX)? {
                Some(i) => {
                    inner.del_slice(i, i + 1)?;
                    Ok(Value::none())
                }
                None => Err(value_error("array.remove(x): x not in array")),
            }
        }

        fn reverse(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("reverse", args)?;
            noargs("reverse", rest)?;
            let size = inner.itemsize();
            let mut data = inner.data.borrow_mut();
            let len = data.len() / size;
            for k in 0..len / 2 {
                let (front, back) = (k * size, (len - 1 - k) * size);
                for b in 0..size {
                    data.swap(front + b, back + b);
                }
            }
            Ok(Value::none())
        }

        fn tofile(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("tofile", args)?;
            let params = clinic_positional("tofile", rest, 1)?;
            let bytes = inner.snapshot();
            for block in bytes.chunks(TOFILE_BLOCKSIZE) {
                let write = _interp.get_attr(params[0], "write")?;
                _interp.call_function_expanded(
                    write,
                    &[ExpandedCallArg { name: None, value: Value::bytes(block.to_vec()) }],
                )?;
            }
            Ok(Value::none())
        }

        fn tolist(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("tolist", args)?;
            noargs("tolist", rest)?;
            tolist(&inner)
        }

        fn tobytes(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("tobytes", args)?;
            noargs("tobytes", rest)?;
            Ok(Value::bytes(inner.snapshot()))
        }

        fn tounicode(args) -> Result<Value> {
            let (_this, inner, rest) = method_receiver("tounicode", args)?;
            noargs("tounicode", rest)?;
            if inner.tc != TypeCode::Unicode {
                return Err(value_error(
                    "tounicode() may only be called on unicode type arrays",
                ));
            }
            Ok(Value::string(decode_wide_str(&inner.data.borrow())?))
        }

        fn __sizeof__(args) -> Result<Value> {
            let (this, inner, rest) = method_receiver("__sizeof__", args)?;
            noargs("__sizeof__", rest)?;
            Ok(Value::int(sizeof(this, &inner) as i64))
        }

        #[classmethod]
        fn __class_getitem__(args) -> Result<Value> {
            let Some((cls, rest)) = args.split_first() else {
                return Err(type_error(
                    "array.__class_getitem__() takes exactly one argument (0 given)",
                ));
            };
            let item = one_arg("array", "__class_getitem__", rest)?;
            let type_arguments = if matches!(item.kind(), ValueKind::Tuple(_)) {
                item.clone()
            } else {
                Value::tuple(vec![item.clone()])
            };
            Ok(pyrust_builtins::generic_alias::generic_alias(
                cls.value.clone(),
                type_arguments,
            ))
        }

        // Slots.

        fn __repr__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__repr__", args)?;
            slot_args("__repr__", rest, 0, false)?;
            repr(_interp, this, &inner)
        }

        fn __eq__(args) -> Result<Value> {
            compare_method(_interp, "__eq__", args, CompareOp::Eq)
        }

        fn __ne__(args) -> Result<Value> {
            compare_method(_interp, "__ne__", args, CompareOp::Ne)
        }

        fn __lt__(args) -> Result<Value> {
            compare_method(_interp, "__lt__", args, CompareOp::Lt)
        }

        fn __le__(args) -> Result<Value> {
            compare_method(_interp, "__le__", args, CompareOp::Le)
        }

        fn __gt__(args) -> Result<Value> {
            compare_method(_interp, "__gt__", args, CompareOp::Gt)
        }

        fn __ge__(args) -> Result<Value> {
            compare_method(_interp, "__ge__", args, CompareOp::Ge)
        }

        fn __iter__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__iter__", args)?;
            slot_args("__iter__", rest, 0, false)?;
            new_iterator(_interp, this, inner)
        }

        fn __len__(args) -> Result<Value> {
            let (_this, inner, rest) = slot_receiver("__len__", args)?;
            slot_args("__len__", rest, 0, false)?;
            Ok(Value::int(inner.len() as i64))
        }

        fn __add__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__add__", args)?;
            let other = &slot_args("__add__", rest, 1, false)?[0].value;
            let Some(other_inner) = as_array(other) else {
                if defers_to_reflected(other, "__radd__")? {
                    return Ok(Value::not_implemented());
                }
                return Err(type_error(format!(
                    "can only append array (not \"{}\") to array",
                    tp_name(other)
                )));
            };
            if other_inner.tc != inner.tc {
                return Err(bad_argument());
            }
            let mut bytes = inner.snapshot();
            bytes.extend_from_slice(&other_inner.data.borrow());
            new_base_array(this, inner.tc, bytes)
        }

        fn __mul__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__mul__", args)?;
            let n = &slot_args("__mul__", rest, 1, true)?[0].value;
            if defers_to_reflected(n, "__rmul__")? {
                return Ok(Value::not_implemented());
            }
            let Some(count) = repeat_count(_interp, n)? else {
                return Err(non_int_multiplier(n));
            };
            let bytes = repeat_bytes(&inner.data.borrow(), count)?;
            new_base_array(this, inner.tc, bytes)
        }

        fn __rmul__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__rmul__", args)?;
            let n = &slot_args("__rmul__", rest, 1, true)?[0].value;
            let Some(count) = repeat_count(_interp, n)? else {
                return Err(non_int_multiplier(n));
            };
            let bytes = repeat_bytes(&inner.data.borrow(), count)?;
            new_base_array(this, inner.tc, bytes)
        }

        fn __iadd__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__iadd__", args)?;
            let other = &slot_args("__iadd__", rest, 1, false)?[0].value;
            if as_array(other).is_none() {
                if defers_to_reflected(other, "__radd__")? {
                    return Ok(Value::not_implemented());
                }
                return Err(type_error(format!(
                    "can only extend array with array (not \"{}\")",
                    tp_name(other)
                )));
            }
            do_extend(_interp, &inner, other)?;
            Ok(this.clone())
        }

        fn __imul__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__imul__", args)?;
            let n = &slot_args("__imul__", rest, 1, true)?[0].value;
            if defers_to_reflected(n, "__rmul__")? {
                return Ok(Value::not_implemented());
            }
            let Some(count) = repeat_count(_interp, n)? else {
                return Err(non_int_multiplier(n));
            };
            let len = inner.len();
            if len > 0 && count != 1 {
                let count = count.max(0);
                let new_len = len.checked_mul(count as usize).ok_or_else(memory_error)?;
                inner.begin_resize(new_len)?;
                let bytes = repeat_bytes(&inner.data.borrow(), count)?;
                *inner.data.borrow_mut() = bytes;
            }
            Ok(this.clone())
        }

        fn __contains__(args) -> Result<Value> {
            let (_this, inner, rest) = slot_receiver("__contains__", args)?;
            let value = &slot_args("__contains__", rest, 1, false)?[0].value;
            let found = find_item(_interp, &inner, value, 0, usize::MAX)?;
            Ok(Value::bool_(found.is_some()))
        }

        fn __getitem__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__getitem__", args)?;
            let key = &slot_args("__getitem__", rest, 1, false)?[0].value;
            subscript(_interp, this, &inner, key)
        }

        fn __setitem__(args) -> Result<Value> {
            let (_this, inner, rest) = slot_receiver("__setitem__", args)?;
            let params = slot_args("__setitem__", rest, 2, true)?;
            ass_subscript(_interp, &inner, &params[0].value, Some(&params[1].value))?;
            Ok(Value::none())
        }

        fn __delitem__(args) -> Result<Value> {
            let (_this, inner, rest) = slot_receiver("__delitem__", args)?;
            let key = &slot_args("__delitem__", rest, 1, false)?[0].value;
            ass_subscript(_interp, &inner, key, None)?;
            Ok(Value::none())
        }

        /// PEP 688 exporter: a live, writable, zero-copy `memoryview`.
        fn __buffer__(args) -> Result<Value> {
            let (this, inner, rest) = slot_receiver("__buffer__", args)?;
            let flags = &slot_args("__buffer__", rest, 1, true)?[0].value;
            let Some(flags) = subscript_index(_interp, flags, "OverflowError")? else {
                return Err(type_error(format!(
                    "'{}' object cannot be interpreted as an integer",
                    tp_name(flags)
                )));
            };
            if flags > i64::from(c_int::MAX) {
                return Err(overflow_error("buffer flags too large"));
            }
            pyrust_builtins::memoryview::from_mutable_buffer(
                Rc::clone(&inner.data),
                Some(Rc::clone(&inner.exports)),
                this.clone(),
                inner.tc.buffer_format(),
                false,
            )
        }

        fn __release_buffer__(args) -> Result<Value> {
            let (this, _inner, rest) = slot_receiver("__release_buffer__", args)?;
            let view = &slot_args("__release_buffer__", rest, 1, true)?[0].value;
            if !pyrust_builtins::memoryview::is_memoryview(view) {
                return Err(type_error("expected a memoryview object"));
            }
            if pyrust_builtins::memoryview::is_released(view) {
                return Err(value_error("memoryview's buffer has already been released"));
            }
            let owner = pyrust_builtins::memoryview::getattr_checked(view, "obj")?;
            if !owner.is_some_and(|owner| owner.is_identical_to(this)) {
                return Err(value_error("memoryview's buffer is not this object"));
            }
            release_memoryview_with_interp(_interp, view)?;
            Ok(Value::none())
        }
    }

    class arrayiterator {
        fn __new__(_args) -> Result<Value> {
            Err(type_error("cannot create 'array.arrayiterator' instances"))
        }

        fn __iter__(args) -> Result<Value> {
            let (this, rest) = iterator_receiver("__iter__", args)?;
            slot_args("__iter__", rest, 0, false)?;
            Ok(this.clone())
        }

        fn __next__(args) -> Result<Value> {
            let (this, rest) = iterator_receiver("__next__", args)?;
            slot_args("__next__", rest, 0, false)?;
            let step = with_native_state::<ArrayIterState, _>(this, ITER_TP_NAME, |state| {
                let Some((_, inner)) = &state.source else {
                    return Ok(None);
                };
                Ok(Some((Rc::clone(inner), state.index)))
            })?;
            let Some((inner, index)) = step else {
                return Err(PyError::named("StopIteration", String::new()));
            };
            if index < inner.len() {
                let item = inner.item(index)?;
                crate::builtin_modules::macros::native_state::with_native_state_mut::<ArrayIterState, _>(
                    this,
                    ITER_TP_NAME,
                    |state| {
                        state.index = index + 1;
                        Ok(())
                    },
                )?;
                return Ok(item);
            }
            crate::builtin_modules::macros::native_state::with_native_state_mut::<ArrayIterState, _>(
                this,
                ITER_TP_NAME,
                |state| {
                    state.source = None;
                    Ok(())
                },
            )?;
            Err(PyError::named("StopIteration", String::new()))
        }

        fn __reduce__(args) -> Result<Value> {
            let (this, rest) = iterator_receiver("__reduce__", args)?;
            if !rest.is_empty() {
                return Err(type_error("__reduce__() takes no arguments"));
            }
            let builtins = _interp.load_module("builtins")?;
            let iter_fn = _interp.get_attr(&builtins, "iter")?;
            let state = with_native_state::<ArrayIterState, _>(this, ITER_TP_NAME, |state| {
                Ok(state
                    .source
                    .as_ref()
                    .map(|(array, _)| (array.clone(), state.index)))
            })?;
            Ok(match state {
                Some((array, index)) => Value::tuple(vec![
                    iter_fn,
                    Value::tuple(vec![array]),
                    Value::int(index as i64),
                ]),
                None => Value::tuple(vec![iter_fn, Value::tuple(vec![Value::tuple(Vec::new())])]),
            })
        }

        fn __setstate__(args) -> Result<Value> {
            let (this, rest) = iterator_receiver("__setstate__", args)?;
            let state = one_arg("arrayiterator", "__setstate__", rest)?;
            let index = match builtin_payload(state).kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                ValueKind::BigInt(_) => {
                    return Err(overflow_error("Python int too large to convert to C ssize_t"));
                }
                _ => return Err(type_error("an integer is required")),
            };
            crate::builtin_modules::macros::native_state::with_native_state_mut::<ArrayIterState, _>(
                this,
                ITER_TP_NAME,
                |st| {
                    if let Some((_, inner)) = &st.source {
                        st.index = index.clamp(0, inner.len() as i64) as usize;
                    }
                    Ok(())
                },
            )?;
            Ok(Value::none())
        }
    }
}
