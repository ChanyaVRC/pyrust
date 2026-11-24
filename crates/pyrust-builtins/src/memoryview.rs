//! `memoryview` built-in type (PEP 3118 / PEP 688).
//!
//! Provides zero-copy live views over `bytes`, `bytearray`, `array.array`,
//! `io.BytesIO`, another `memoryview`, and custom PEP 688 `__buffer__` /
//! `__release_buffer__` objects.

mod format;
mod indexing;
mod methods;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{
    BuiltinState, BuiltinTypeOps, PyError, Result, Value, ValueKind, builtin_ops_is,
};

use format::{NativeFormat, unpack_element};
use methods::compute_c_strides;

pub const TYPE_NAME: &str = "memoryview";
pub const MEMORYVIEW_OPS: &MemoryViewOps = &MemoryViewOps;

pub const METHODS: &[&str] = &[
    "tobytes",
    "tolist",
    "hex",
    "toreadonly",
    "cast",
    "release",
    "__enter__",
    "__exit__",
    "__buffer__",
    "__release_buffer__",
];

pub fn has_method(name: &str) -> bool {
    METHODS.contains(&name)
}

/// Shared underlying byte buffer owned by one or more `memoryview` instances.
#[derive(Clone)]
pub enum BufferBacking {
    Bytes(Rc<Vec<u8>>),
    Mutable {
        data: Rc<RefCell<Vec<u8>>>,
        exports: Option<Rc<Cell<usize>>>,
    },
}

/// CPython `_PyManagedBufferObject` analogue: shared across a root `memoryview`
/// and all its derived subviews / casts / readonly copies.
pub struct ManagedBuffer {
    pub backing: BufferBacking,
    pub obj: Value,
    pub active_views: usize,
    /// Optional PEP 688 `(owner, inner_memoryview)` callback fired once when
    /// `active_views` drops to 0.
    pub release_callback: Option<(Value, Value)>,
}

/// Per-`memoryview` state.
pub struct MemoryViewState {
    pub managed: Rc<RefCell<ManagedBuffer>>,
    pub released: bool,
    pub readonly: bool,
    pub format: String,
    pub(crate) parsed_format: NativeFormat,
    pub itemsize: usize,
    pub offset: isize,
    pub shape: Vec<usize>,
    pub strides: Vec<isize>,
}

impl Drop for MemoryViewState {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.release_internal();
        }
    }
}

pub fn released_error() -> PyError {
    PyError::named(
        "ValueError",
        "operation forbidden on released memoryview object".to_string(),
    )
}

pub(crate) struct ViewLayout {
    pub readonly: bool,
    pub format: String,
    pub parsed_format: NativeFormat,
    pub itemsize: usize,
    pub offset: isize,
    pub shape: Vec<usize>,
    pub strides: Vec<isize>,
}

/// Contiguous writable backing slice `(data_rc, byte_offset, nbytes)`.
pub type WritableContiguousRange = (Rc<RefCell<Vec<u8>>>, usize, usize);

impl MemoryViewState {
    pub(crate) fn derive_view(&self, layout: ViewLayout) -> Self {
        self.managed.borrow_mut().active_views += 1;
        Self {
            managed: Rc::clone(&self.managed),
            released: false,
            readonly: layout.readonly,
            format: layout.format,
            parsed_format: layout.parsed_format,
            itemsize: layout.itemsize,
            offset: layout.offset,
            shape: layout.shape,
            strides: layout.strides,
        }
    }

    pub(crate) fn clone_view(&self, readonly: bool) -> Self {
        self.derive_view(ViewLayout {
            readonly,
            format: self.format.clone(),
            parsed_format: self.parsed_format,
            itemsize: self.itemsize,
            offset: self.offset,
            shape: self.shape.clone(),
            strides: self.strides.clone(),
        })
    }

    /// Release this view. Returns `Some((owner, inner_mv))` if this was the
    /// last active view and a PEP 688 `__release_buffer__` callback must fire.
    pub fn release_internal(&mut self) -> Option<(Value, Value)> {
        if self.released {
            return None;
        }
        self.released = true;
        let mut managed = self.managed.borrow_mut();
        managed.active_views = managed.active_views.saturating_sub(1);
        if managed.active_views == 0 {
            if let BufferBacking::Mutable {
                exports: Some(exp), ..
            } = &managed.backing
            {
                exp.set(exp.get().saturating_sub(1));
            }
            return managed.release_callback.take();
        }
        None
    }

    pub fn nbytes(&self) -> usize {
        let count: usize = self.shape.iter().product();
        count * self.itemsize
    }

    pub fn is_c_contiguous(&self) -> bool {
        if self.nbytes() == 0 || self.shape.is_empty() {
            return true;
        }
        let mut expected = self.itemsize as isize;
        for (&dim, &stride) in self.shape.iter().zip(self.strides.iter()).rev() {
            if dim > 1 {
                if stride != expected {
                    return false;
                }
                expected *= dim as isize;
            }
        }
        true
    }

    pub fn is_f_contiguous(&self) -> bool {
        if self.nbytes() == 0 || self.shape.is_empty() {
            return true;
        }
        let mut expected = self.itemsize as isize;
        for (&dim, &stride) in self.shape.iter().zip(self.strides.iter()) {
            if dim > 1 {
                if stride != expected {
                    return false;
                }
                expected *= dim as isize;
            }
        }
        true
    }

    pub fn is_contiguous(&self) -> bool {
        self.is_c_contiguous() || self.is_f_contiguous()
    }

    pub(crate) fn with_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        let managed = self.managed.borrow();
        match &managed.backing {
            BufferBacking::Bytes(rc) => f(rc.as_slice()),
            BufferBacking::Mutable { data, .. } => {
                let borrow = data.borrow();
                f(borrow.as_slice())
            }
        }
    }

    pub(crate) fn read_element_at(&self, byte_off: isize) -> Result<Value> {
        let pos = byte_off as usize;
        Ok(self
            .with_bytes(|buf| unpack_element(self.parsed_format, &buf[pos..pos + self.itemsize])))
    }

    pub(crate) fn write_bytes_at(&self, byte_off: isize, bytes: &[u8]) -> Result<()> {
        let managed = self.managed.borrow();
        match &managed.backing {
            BufferBacking::Bytes(_) => Err(PyError::named(
                "TypeError",
                "cannot modify read-only memory".to_string(),
            )),
            BufferBacking::Mutable { data, .. } => {
                let pos = byte_off as usize;
                let mut dst = data.borrow_mut();
                dst[pos..pos + bytes.len()].copy_from_slice(bytes);
                Ok(())
            }
        }
    }

    pub(crate) fn write_1d_strided(
        &self,
        first_offset: isize,
        elem_stride: isize,
        count: usize,
        src_bytes: &[u8],
    ) -> Result<()> {
        let managed = self.managed.borrow();
        match &managed.backing {
            BufferBacking::Bytes(_) => Err(PyError::named(
                "TypeError",
                "cannot modify read-only memory".to_string(),
            )),
            BufferBacking::Mutable { data, .. } => {
                let mut dst = data.borrow_mut();
                let itemsize = self.itemsize;
                for i in 0..count {
                    let pos = (first_offset + (i as isize) * elem_stride) as usize;
                    dst[pos..pos + itemsize]
                        .copy_from_slice(&src_bytes[i * itemsize..(i + 1) * itemsize]);
                }
                Ok(())
            }
        }
    }

    pub fn tobytes_order(&self, order: char) -> Result<Vec<u8>> {
        if self.released {
            return Err(released_error());
        }
        let nbytes = self.nbytes();
        if nbytes == 0 {
            return Ok(Vec::new());
        }
        let effective_order = match order {
            'C' => 'C',
            'F' => 'F',
            'A' => {
                if self.is_f_contiguous() && !self.is_c_contiguous() {
                    'F'
                } else {
                    'C'
                }
            }
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    "order must be 'C', 'F' or 'A'".to_string(),
                ));
            }
        };
        if self.shape.is_empty() {
            let pos = self.offset as usize;
            return Ok(self.with_bytes(|buf| buf[pos..pos + self.itemsize].to_vec()));
        }
        if effective_order == 'C' && self.is_c_contiguous() {
            let start = self.offset as usize;
            return Ok(self.with_bytes(|buf| buf[start..start + nbytes].to_vec()));
        }
        if effective_order == 'F' && self.is_f_contiguous() {
            let start = self.offset as usize;
            return Ok(self.with_bytes(|buf| buf[start..start + nbytes].to_vec()));
        }
        let itemsize = self.itemsize;
        let ndim = self.shape.len();
        let mut out = Vec::with_capacity(nbytes);
        self.with_bytes(|buf| {
            let mut coords = vec![0usize; ndim];
            let total_elems: usize = self.shape.iter().product();
            for _ in 0..total_elems {
                let mut pos = self.offset;
                for d in 0..ndim {
                    pos += (coords[d] as isize) * self.strides[d];
                }
                let u = pos as usize;
                out.extend_from_slice(&buf[u..u + itemsize]);
                if effective_order == 'C' {
                    for d in (0..ndim).rev() {
                        coords[d] += 1;
                        if coords[d] < self.shape[d] {
                            break;
                        }
                        coords[d] = 0;
                    }
                } else {
                    for d in 0..ndim {
                        coords[d] += 1;
                        if coords[d] < self.shape[d] {
                            break;
                        }
                        coords[d] = 0;
                    }
                }
            }
        });
        Ok(out)
    }

    pub fn getattr_result(&self, name: &str) -> Result<Option<Value>> {
        let is_mv_attr = matches!(
            name,
            "obj"
                | "nbytes"
                | "readonly"
                | "format"
                | "itemsize"
                | "ndim"
                | "shape"
                | "strides"
                | "suboffsets"
                | "c_contiguous"
                | "f_contiguous"
                | "contiguous"
        );
        if !is_mv_attr {
            return Ok(None);
        }
        if self.released {
            return Err(released_error());
        }
        let val = match name {
            "obj" => self.managed.borrow().obj.clone(),
            "nbytes" => Value::int(self.nbytes() as i64),
            "readonly" => Value::bool_(self.readonly),
            "format" => Value::string(self.format.as_str()),
            "itemsize" => Value::int(self.itemsize as i64),
            "ndim" => Value::int(self.shape.len() as i64),
            "shape" => Value::tuple(self.shape.iter().map(|&d| Value::int(d as i64)).collect()),
            "strides" => Value::tuple(self.strides.iter().map(|&s| Value::int(s as i64)).collect()),
            "suboffsets" => Value::tuple(Vec::new()),
            "c_contiguous" => Value::bool_(self.is_c_contiguous()),
            "f_contiguous" => Value::bool_(self.is_f_contiguous()),
            "contiguous" => Value::bool_(self.is_contiguous()),
            _ => unreachable!(),
        };
        Ok(Some(val))
    }
}

pub struct MemoryViewOps;

impl BuiltinTypeOps for MemoryViewOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let ptr = Rc::as_ptr(state) as usize;
        let borrow = state.borrow();
        let mv = borrow
            .downcast_ref::<MemoryViewState>()
            .expect("memoryview state");
        if mv.released {
            format!("<released memory at {ptr:#x}>")
        } else {
            format!("<memory at {ptr:#x}>")
        }
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        let borrow = state.borrow();
        let mv = borrow
            .downcast_ref::<MemoryViewState>()
            .expect("memoryview state");
        !mv.released && !mv.shape.is_empty() && mv.shape[0] != 0
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        if let ValueKind::BuiltinObject {
            ops: other_ops,
            state: other_state,
        } = other.kind()
            && is_memoryview_ops(other_ops)
        {
            if Rc::ptr_eq(state, other_state) {
                return true;
            }
            let a = state.borrow();
            let a_mv = a
                .downcast_ref::<MemoryViewState>()
                .expect("memoryview state");
            let b = other_state.borrow();
            let b_mv = b
                .downcast_ref::<MemoryViewState>()
                .expect("memoryview state");
            return indexing::eq_memoryviews(a_mv, b_mv);
        }
        let borrow = state.borrow();
        let mv = borrow
            .downcast_ref::<MemoryViewState>()
            .expect("memoryview state");
        if mv.released {
            return false;
        }
        match other.kind() {
            ValueKind::Bytes(rc) => indexing::eq_with_raw_bytes(mv, rc.as_slice()),
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
            {
                if let Some(rc) = crate::bytearray::as_bytearray_rc(other) {
                    indexing::eq_with_bytearray(mv, &rc)
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        let borrow = state.borrow();
        let mv = borrow
            .downcast_ref::<MemoryViewState>()
            .expect("memoryview state");
        if mv.released || mv.shape.is_empty() {
            return None;
        }
        Some(mv.shape[0])
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        let borrow = state.borrow();
        let mv = borrow
            .downcast_ref::<MemoryViewState>()
            .expect("memoryview state");
        indexing::get_item(mv, key)
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let borrow = state.borrow();
        let mv = borrow
            .downcast_ref::<MemoryViewState>()
            .expect("memoryview state");
        indexing::set_item(mv, key, value)
    }

    fn delete_item(&self, state: &BuiltinState, _key: &Value) -> Result<()> {
        let borrow = state.borrow();
        let mv = borrow
            .downcast_ref::<MemoryViewState>()
            .expect("memoryview state");
        if mv.released {
            return Err(released_error());
        }
        Err(PyError::named(
            "TypeError",
            "cannot delete memory".to_string(),
        ))
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let borrow = state.borrow();
        let mv = borrow
            .downcast_ref::<MemoryViewState>()
            .expect("memoryview state");
        if mv.released {
            return Err(released_error());
        }
        let ndim = mv.shape.len();
        if ndim == 0 {
            return Err(PyError::named(
                "TypeError",
                "argument of type 'memoryview' is not iterable".to_string(),
            ));
        }
        if ndim > 1 {
            return Err(PyError::named(
                "NotImplementedError",
                "multi-dimensional sub-views are not implemented".to_string(),
            ));
        }
        let len = mv.shape[0];
        for i in 0..len {
            let byte_off = mv.offset + (i as isize) * mv.strides[0];
            let elem = mv.read_element_at(byte_off)?;
            if elem == *item {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn has_method(&self, name: &str) -> bool {
        has_method(name)
    }

    fn setattr(&self, _state: &BuiltinState, name: &str, _value: Value) -> Result<()> {
        let is_ro_attr = matches!(
            name,
            "obj"
                | "nbytes"
                | "readonly"
                | "format"
                | "itemsize"
                | "ndim"
                | "shape"
                | "strides"
                | "suboffsets"
                | "c_contiguous"
                | "f_contiguous"
                | "contiguous"
        );
        if is_ro_attr {
            Err(PyError::named(
                "AttributeError",
                format!("attribute '{name}' of 'memoryview' objects is not writable"),
            ))
        } else {
            Err(PyError::named(
                "AttributeError",
                format!("'memoryview' object has no attribute '{name}'"),
            ))
        }
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        match name {
            "tobytes" => {
                let borrow = state.borrow();
                let mv = borrow
                    .downcast_ref::<MemoryViewState>()
                    .expect("memoryview state");
                methods::call_tobytes(mv, &args, kwargs)
            }
            "tolist" => {
                let borrow = state.borrow();
                let mv = borrow
                    .downcast_ref::<MemoryViewState>()
                    .expect("memoryview state");
                methods::call_tolist(mv, &args, kwargs)
            }
            "hex" => {
                let borrow = state.borrow();
                let mv = borrow
                    .downcast_ref::<MemoryViewState>()
                    .expect("memoryview state");
                methods::call_hex(mv, &args, kwargs)
            }
            "toreadonly" => {
                let borrow = state.borrow();
                let mv = borrow
                    .downcast_ref::<MemoryViewState>()
                    .expect("memoryview state");
                methods::call_toreadonly(mv, &args, kwargs)
            }
            "cast" => {
                let borrow = state.borrow();
                let mv = borrow
                    .downcast_ref::<MemoryViewState>()
                    .expect("memoryview state");
                methods::call_cast(mv, &args, kwargs)
            }
            "release" => {
                if !kwargs.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "memoryview.release() takes no keyword arguments".to_string(),
                    ));
                }
                if !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "memoryview.release() takes no arguments ({} given)",
                            args.len()
                        ),
                    ));
                }
                let mut borrow = state.borrow_mut();
                let mv = borrow
                    .downcast_mut::<MemoryViewState>()
                    .expect("memoryview state");
                let _ = mv.release_internal();
                Ok(Value::none())
            }
            "__enter__" => {
                if !kwargs.is_empty() || !args.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "memoryview.__enter__() takes no arguments".to_string(),
                    ));
                }
                let borrow = state.borrow();
                let mv = borrow
                    .downcast_ref::<MemoryViewState>()
                    .expect("memoryview state");
                if mv.released {
                    return Err(released_error());
                }
                // Note: interpreter's bound_objects intercepts __enter__ to return
                // the original receiver Value preserving object identity.
                Ok(Value::none())
            }
            "__exit__" => {
                let mut borrow = state.borrow_mut();
                let mv = borrow
                    .downcast_mut::<MemoryViewState>()
                    .expect("memoryview state");
                let _ = mv.release_internal();
                Ok(Value::none())
            }
            "__buffer__" => {
                if !kwargs.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "memoryview.__buffer__() takes no keyword arguments".to_string(),
                    ));
                }
                if args.len() != 1 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "memoryview.__buffer__() takes exactly one argument ({} given)",
                            args.len()
                        ),
                    ));
                }
                let flags = match args[0].kind() {
                    ValueKind::Int(n) => n,
                    ValueKind::Bool(b) => i64::from(b),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "'{}' object cannot be interpreted as an integer",
                                pyrust_core::builtin_type_name(&args[0])
                            ),
                        ));
                    }
                };
                let borrow = state.borrow();
                let mv = borrow
                    .downcast_ref::<MemoryViewState>()
                    .expect("memoryview state");
                if mv.released {
                    return Err(released_error());
                }
                // PyBUF_WRITABLE = 0x1
                if (flags & 0x1) != 0 && mv.readonly {
                    return Err(PyError::named(
                        "BufferError",
                        "Object is not writable.".to_string(),
                    ));
                }
                let derived = mv.clone_view(mv.readonly);
                Ok(Value::builtin_object(MEMORYVIEW_OPS, Box::new(derived)))
            }
            "__release_buffer__" => {
                if !kwargs.is_empty() || args.len() != 1 {
                    return Err(PyError::named(
                        "TypeError",
                        "memoryview.__release_buffer__() takes exactly one argument".to_string(),
                    ));
                }
                Ok(Value::none())
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'memoryview' object has no attribute '{name}'"),
            )),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public constructors & inspection helpers
// ─────────────────────────────────────────────────────────────────────────────

#[inline]
pub fn is_memoryview_ops(ops: &dyn BuiltinTypeOps) -> bool {
    builtin_ops_is::<MemoryViewOps>(ops)
}

#[inline]
pub fn is_memoryview(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::BuiltinObject { ops, .. } if is_memoryview_ops(ops)
    )
}

/// Construct a readonly `memoryview` over `bytes`.
pub fn from_bytes(bytes_rc: Rc<Vec<u8>>, obj: Value) -> Value {
    let len = bytes_rc.len();
    let managed = Rc::new(RefCell::new(ManagedBuffer {
        backing: BufferBacking::Bytes(bytes_rc),
        obj,
        active_views: 1,
        release_callback: None,
    }));
    let state: Box<dyn Any> = Box::new(MemoryViewState {
        managed,
        released: false,
        readonly: true,
        format: "B".to_string(),
        parsed_format: NativeFormat::UnsignedChar,
        itemsize: 1,
        offset: 0,
        shape: vec![len],
        strides: vec![1],
    });
    Value::builtin_object(MEMORYVIEW_OPS, state)
}

/// Construct a writable `memoryview` over a shared mutable byte buffer (`bytearray`,
/// `array.array._buf`, `io.BytesIO._buf`).
pub fn from_mutable_buffer(
    data: Rc<RefCell<Vec<u8>>>,
    exports: Option<Rc<Cell<usize>>>,
    obj: Value,
    format_str: &str,
    readonly: bool,
) -> Result<Value> {
    let parsed_format = NativeFormat::parse(format_str)?;
    let itemsize = parsed_format.itemsize();
    let byte_len = data.borrow().len();
    let elem_count = byte_len / itemsize;
    if let Some(ref exp) = exports {
        exp.set(exp.get() + 1);
    }
    let managed = Rc::new(RefCell::new(ManagedBuffer {
        backing: BufferBacking::Mutable { data, exports },
        obj,
        active_views: 1,
        release_callback: None,
    }));
    let shape = vec![elem_count];
    let strides = compute_c_strides(&shape, itemsize);
    let state: Box<dyn Any> = Box::new(MemoryViewState {
        managed,
        released: false,
        readonly,
        format: format_str.to_string(),
        parsed_format,
        itemsize,
        offset: 0,
        shape,
        strides,
    });
    Ok(Value::builtin_object(MEMORYVIEW_OPS, state))
}

/// Construct a new `memoryview` sharing the buffer of an existing `memoryview`.
pub fn from_memoryview(src: &Value) -> Result<Value> {
    let ValueKind::BuiltinObject { ops, state } = src.kind() else {
        return Err(PyError::runtime("expected memoryview".to_string()));
    };
    if !is_memoryview_ops(ops) {
        return Err(PyError::runtime("expected memoryview".to_string()));
    }
    let borrow = state.borrow();
    let mv = borrow
        .downcast_ref::<MemoryViewState>()
        .expect("memoryview state");
    if mv.released {
        return Err(released_error());
    }
    let derived = mv.clone_view(mv.readonly);
    Ok(Value::builtin_object(MEMORYVIEW_OPS, Box::new(derived)))
}

/// Wrap a `memoryview` returned by a PEP 688 `__buffer__` method so releasing
/// the wrapper invokes `owner.__release_buffer__(inner_mv)`.
pub fn wrap_pep688_view(owner: Value, inner_mv: Value, has_release_hook: bool) -> Result<Value> {
    let ValueKind::BuiltinObject { ops, state } = inner_mv.kind() else {
        return Err(PyError::named(
            "TypeError",
            "__buffer__ returned non-memoryview object".to_string(),
        ));
    };
    if !is_memoryview_ops(ops) {
        return Err(PyError::named(
            "TypeError",
            "__buffer__ returned non-memoryview object".to_string(),
        ));
    }
    let borrow = state.borrow();
    let src = borrow
        .downcast_ref::<MemoryViewState>()
        .expect("memoryview state");
    if src.released {
        return Err(released_error());
    }
    if !has_release_hook {
        // Still create an independent wrapper view whose release is decoupled
        // from the inner view (or return a derived view sharing the buffer).
        let derived = src.clone_view(src.readonly);
        return Ok(Value::builtin_object(MEMORYVIEW_OPS, Box::new(derived)));
    }
    // Create a ManagedBuffer holding the backing from `src` AND the PEP 688
    // release callback so that releasing this view (and all its slices) fires
    // `owner.__release_buffer__(inner_mv)` once active_views reaches 0.
    let backing = src.managed.borrow().backing.clone();
    if let BufferBacking::Mutable {
        exports: Some(ref exp),
        ..
    } = backing
    {
        exp.set(exp.get() + 1);
    }
    let managed = Rc::new(RefCell::new(ManagedBuffer {
        backing,
        obj: owner.clone(),
        active_views: 1,
        release_callback: Some((owner, inner_mv.clone())),
    }));
    let wrapped = MemoryViewState {
        managed,
        released: false,
        readonly: src.readonly,
        format: src.format.clone(),
        parsed_format: src.parsed_format,
        itemsize: src.itemsize,
        offset: src.offset,
        shape: src.shape.clone(),
        strides: src.strides.clone(),
    };
    Ok(Value::builtin_object(MEMORYVIEW_OPS, Box::new(wrapped)))
}

/// Release a `memoryview` and return any PEP 688 `(owner, inner_mv)` callback
/// that must be invoked by the interpreter.
pub fn release_and_take_callback(value: &Value) -> Option<(Value, Value)> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !is_memoryview_ops(ops) {
        return None;
    }
    let mut borrow = state.borrow_mut();
    let mv = borrow.downcast_mut::<MemoryViewState>()?;
    mv.release_internal()
}

/// Fallible attribute lookup for `memoryview` (surfaces `ValueError` when released).
pub fn getattr_checked(value: &Value, name: &str) -> Result<Option<Value>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return Ok(None);
    };
    if !is_memoryview_ops(ops) {
        return Ok(None);
    }
    let borrow = state.borrow();
    let mv = borrow
        .downcast_ref::<MemoryViewState>()
        .expect("memoryview state");
    mv.getattr_result(name)
}

/// Fallible `len(mv)` (surfaces `ValueError` when released, `TypeError` for 0-D).
pub fn len_checked(value: &Value) -> Result<usize> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return Err(PyError::runtime("expected memoryview".to_string()));
    };
    if !is_memoryview_ops(ops) {
        return Err(PyError::runtime("expected memoryview".to_string()));
    }
    let borrow = state.borrow();
    let mv = borrow
        .downcast_ref::<MemoryViewState>()
        .expect("memoryview state");
    if mv.released {
        return Err(released_error());
    }
    if mv.shape.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "0-dim memory has no length".to_string(),
        ));
    }
    Ok(mv.shape[0])
}

/// Fallible `bool(mv)` (delegates to `len_checked(mv) != 0`, matching CPython 3.12).
pub fn truthy_checked(value: &Value) -> Result<bool> {
    Ok(len_checked(value)? != 0)
}

/// Validate hashability and return `(underlying_obj, c_order_bytes)` for `hash(mv)`.
pub fn hash_prepare(value: &Value) -> Result<(Value, Vec<u8>)> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return Err(PyError::runtime("expected memoryview".to_string()));
    };
    if !is_memoryview_ops(ops) {
        return Err(PyError::runtime("expected memoryview".to_string()));
    }
    let borrow = state.borrow();
    let mv = borrow
        .downcast_ref::<MemoryViewState>()
        .expect("memoryview state");
    if mv.released {
        return Err(released_error());
    }
    if !mv.readonly {
        return Err(PyError::named(
            "ValueError",
            "cannot hash writable memoryview object".to_string(),
        ));
    }
    if !mv.parsed_format.is_byte_format() {
        return Err(PyError::named(
            "ValueError",
            "memoryview: hashing is restricted to formats 'B', 'b' or 'c'".to_string(),
        ));
    }
    let obj = mv.managed.borrow().obj.clone();
    let bytes = mv.tobytes_order('C')?;
    Ok((obj, bytes))
}

/// Extract C-order bytes snapshot from a `memoryview` (raising `ValueError` if released).
pub fn to_bytes_checked(value: &Value) -> Result<Vec<u8>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return Err(PyError::runtime("expected memoryview".to_string()));
    };
    if !is_memoryview_ops(ops) {
        return Err(PyError::runtime("expected memoryview".to_string()));
    }
    let borrow = state.borrow();
    let mv = borrow
        .downcast_ref::<MemoryViewState>()
        .expect("memoryview state");
    mv.tobytes_order('C')
}

/// Try to extract a contiguous writable slice view `(data_rc, offset, nbytes)` from
/// a `memoryview`. Returns `Err` for released, readonly, or non-contiguous views.
pub fn writable_contiguous_range(value: &Value) -> Result<WritableContiguousRange> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return Err(PyError::runtime("expected memoryview".to_string()));
    };
    if !is_memoryview_ops(ops) {
        return Err(PyError::runtime("expected memoryview".to_string()));
    }
    let borrow = state.borrow();
    let mv = borrow
        .downcast_ref::<MemoryViewState>()
        .expect("memoryview state");
    if mv.released {
        return Err(released_error());
    }
    if mv.readonly {
        return Err(PyError::named(
            "TypeError",
            "cannot modify read-only memory".to_string(),
        ));
    }
    if !mv.is_c_contiguous() {
        return Err(PyError::named(
            "BufferError",
            "memoryview: underlying buffer is not C-contiguous".to_string(),
        ));
    }
    let nbytes = mv.nbytes();
    let offset = mv.offset as usize;
    let managed = mv.managed.borrow();
    match &managed.backing {
        BufferBacking::Bytes(_) => Err(PyError::named(
            "TypeError",
            "cannot modify read-only memory".to_string(),
        )),
        BufferBacking::Mutable { data, .. } => Ok((Rc::clone(data), offset, nbytes)),
    }
}

/// Materialise iteration elements of a `memoryview` (raises `ValueError` if released,
/// `TypeError` for 0-D, `NotImplementedError` for ND > 1).
pub fn iter_elements(value: &Value) -> Result<Vec<Value>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return Err(PyError::runtime("expected memoryview".to_string()));
    };
    if !is_memoryview_ops(ops) {
        return Err(PyError::runtime("expected memoryview".to_string()));
    }
    let borrow = state.borrow();
    let mv = borrow
        .downcast_ref::<MemoryViewState>()
        .expect("memoryview state");
    if mv.released {
        return Err(released_error());
    }
    let ndim = mv.shape.len();
    if ndim == 0 {
        return Err(PyError::named(
            "TypeError",
            "invalid indexing of 0-dim memory".to_string(),
        ));
    }
    if ndim > 1 {
        return Err(PyError::named(
            "NotImplementedError",
            "multi-dimensional sub-views are not implemented".to_string(),
        ));
    }
    let len = mv.shape[0];
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let byte_off = mv.offset + (i as isize) * mv.strides[0];
        out.push(mv.read_element_at(byte_off)?);
    }
    Ok(out)
}

/// Check if a `memoryview` is released.
pub fn is_released(value: &Value) -> bool {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return false;
    };
    if !is_memoryview_ops(ops) {
        return false;
    }
    state
        .borrow()
        .downcast_ref::<MemoryViewState>()
        .is_some_and(|mv| mv.released)
}

/// Compare a `memoryview` against a raw byte slice (used by `ByteArrayOps::eq`).
pub(crate) fn eq_with_bytearray_rc(mv_val: &Value, ba_rc: &Rc<RefCell<Vec<u8>>>) -> bool {
    let ValueKind::BuiltinObject { ops, state } = mv_val.kind() else {
        return false;
    };
    if !is_memoryview_ops(ops) {
        return false;
    }
    let borrow = state.borrow();
    let Some(mv) = borrow.downcast_ref::<MemoryViewState>() else {
        return false;
    };
    indexing::eq_with_bytearray(mv, ba_rc)
}
