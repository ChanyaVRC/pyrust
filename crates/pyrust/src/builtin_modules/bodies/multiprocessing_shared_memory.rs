// `multiprocessing.shared_memory` — 100% native-Rust Python 3.12 OS shared
// memory (`SharedMemory`, `ShareableList`) (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::builtin_modules::mp_rt::shm::OsSharedMem;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyIndex, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, NativeIterFrame, extract_buffer_bytes, render_value_repr,
};
use crate::value::{InstanceAttrs, PyInstance, PyModule, Value, ValueKind};

const BUF_WRITABLE: i64 = 0x1;

// ShareableList binary slot tags (stored in OsSharedMem).
const SL_MAGIC: &[u8; 4] = b"PYSL";
const TAG_INT: u8 = 1;
const TAG_FLOAT: u8 = 2;
const TAG_BOOL: u8 = 3;
const TAG_STR: u8 = 4;
const TAG_BYTES: u8 = 5;
const TAG_NONE: u8 = 6;

struct SharedMemoryState {
    shm: Arc<OsSharedMem>,
    buf_obj: Option<Value>,
}

struct SharedMemViewState {
    shm: Arc<OsSharedMem>,
    owner: Value,
    offset: usize,
    len: usize,
    released: bool,
    /// Temporary Vec backing an active PEP 688 `__buffer__` export so
    /// `__release_buffer__` can flush any writes back into live OS shared memory.
    pending_export: Option<Rc<RefCell<Vec<u8>>>>,
}

impl SharedMemViewState {
    fn ensure_alive(&self) -> Result<()> {
        if self.released {
            return Err(PyError::named(
                "ValueError",
                "operation forbidden on released memoryview object",
            ));
        }
        if self.shm.is_closed() {
            return Err(PyError::named(
                "ValueError",
                "operation forbidden on closed SharedMemory",
            ));
        }
        Ok(())
    }
}

struct ShareableListState {
    shm_val: Value,
    shm: Arc<OsSharedMem>,
    count: usize,
    /// Per-slot `(data_offset, capacity_bytes, fmt_tag_byte)`
    slots: Vec<(usize, usize, u8)>,
    format: String,
}

fn make_shm_view(
    interp: &mut Interpreter,
    shm: Arc<OsSharedMem>,
    owner: Value,
    offset: usize,
    len: usize,
) -> Result<Value> {
    let m = interp.load_module("multiprocessing.shared_memory")?;
    let cls = interp.get_attr(&m, "_SharedMemoryView")?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("RuntimeError", "_SharedMemoryView invalid"));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        SharedMemViewState {
            shm,
            owner,
            offset,
            len,
            released: false,
            pending_export: None,
        },
    )?;
    Ok(inst)
}

fn collect_slice_indices(start: i64, stop: i64, step: i64) -> Vec<usize> {
    let mut out = Vec::new();
    let mut cur = start;
    if step > 0 {
        while cur < stop {
            out.push(cur as usize);
            cur += step;
        }
    } else {
        while cur > stop {
            out.push(cur as usize);
            cur += step;
        }
    }
    out
}

fn sl_encode_slot(val: &Value, cap: usize) -> Result<(u8, Vec<u8>)> {
    match val.kind() {
        ValueKind::None => Ok((TAG_NONE, Vec::new())),
        ValueKind::Bool(b) => Ok((TAG_BOOL, vec![u8::from(b)])),
        ValueKind::Int(i) => Ok((TAG_INT, i.to_le_bytes().to_vec())),
        ValueKind::Float(f) => Ok((TAG_FLOAT, f.to_le_bytes().to_vec())),
        ValueKind::Str(s) => {
            let b = s.as_bytes();
            if b.len() > cap {
                return Err(PyError::named(
                    "ValueError",
                    "exceeds available storage for existing str",
                ));
            }
            Ok((TAG_STR, b.to_vec()))
        }
        ValueKind::Bytes(b) => {
            if b.len() > cap {
                return Err(PyError::named(
                    "ValueError",
                    "exceeds available storage for existing bytes",
                ));
            }
            Ok((TAG_BYTES, b.as_slice().to_vec()))
        }
        _ => Err(PyError::named(
            "TypeError",
            "ShareableList only supports int, float, bool, str, bytes, and None",
        )),
    }
}

fn sl_read_item(shm: &OsSharedMem, offset: usize) -> Result<Value> {
    let mut hdr = [0u8; 5];
    shm.read_bytes(offset, &mut hdr)?;
    let tag = hdr[0];
    let len = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
    let mut data = vec![0u8; len];
    if len > 0 {
        shm.read_bytes(offset + 5, &mut data)?;
    }
    match tag {
        TAG_NONE => Ok(Value::none()),
        TAG_BOOL => Ok(Value::bool_(data.first().copied().unwrap_or(0) != 0)),
        TAG_INT => {
            if data.len() != 8 {
                return Err(PyError::named("ValueError", "corrupt int in ShareableList"));
            }
            Ok(Value::int(i64::from_le_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ])))
        }
        TAG_FLOAT => {
            if data.len() != 8 {
                return Err(PyError::named(
                    "ValueError",
                    "corrupt float in ShareableList",
                ));
            }
            Ok(Value::float(f64::from_le_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ])))
        }
        TAG_STR => {
            let s = String::from_utf8(data).map_err(|_| {
                PyError::named("UnicodeDecodeError", "invalid utf-8 in ShareableList")
            })?;
            Ok(Value::string(&s))
        }
        TAG_BYTES => Ok(Value::bytes(data)),
        _ => Err(PyError::named("ValueError", "unknown tag in ShareableList")),
    }
}

fn sl_write_item(shm: &OsSharedMem, offset: usize, cap: usize, val: &Value) -> Result<()> {
    let (tag, data) = sl_encode_slot(val, cap)?;
    if data.len() > cap {
        return Err(PyError::named(
            "ValueError",
            "exceeds available storage for ShareableList slot",
        ));
    }
    let mut hdr = [0u8; 5];
    hdr[0] = tag;
    hdr[1..5].copy_from_slice(&(data.len() as u32).to_le_bytes());
    shm.write_bytes(offset, &hdr)?;
    if !data.is_empty() {
        shm.write_bytes(offset + 5, &data)?;
    }
    Ok(())
}

fn sl_item_format_code(val: &Value) -> Result<(usize, String, u8)> {
    match val.kind() {
        ValueKind::None => Ok((8, "b".to_string(), TAG_NONE)),
        ValueKind::Bool(_) => Ok((8, "?".to_string(), TAG_BOOL)),
        ValueKind::Int(_) => Ok((8, "q".to_string(), TAG_INT)),
        ValueKind::Float(_) => Ok((8, "d".to_string(), TAG_FLOAT)),
        ValueKind::Str(s) => {
            let cap = s.len().max(8);
            Ok((cap, format!("{cap}s"), TAG_STR))
        }
        ValueKind::Bytes(b) => {
            let cap = b.len().max(8);
            Ok((cap, format!("{cap}s"), TAG_BYTES))
        }
        _ => Err(PyError::named(
            "TypeError",
            "ShareableList only supports int, float, bool, str, bytes, and None",
        )),
    }
}

pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    _module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("SharedMemory"),
            Value::string("ShareableList"),
        ]),
    }

    #[native_state(SharedMemViewState)]
    class _SharedMemoryView {
        #[getter]
        fn nbytes(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(Value::int(st.len as i64))
        }

        #[getter]
        fn readonly(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(Value::bool_(false))
        }

        #[getter]
        fn format(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(Value::string("B"))
        }

        #[getter]
        fn itemsize(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(Value::int(1))
        }

        #[getter]
        fn ndim(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(Value::int(1))
        }

        #[getter]
        fn shape(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(Value::tuple(vec![Value::int(st.len as i64)]))
        }

        #[getter]
        fn strides(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(Value::tuple(vec![Value::int(1)]))
        }

        #[getter]
        fn obj(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(st.owner.clone())
        }

        fn release(#[state_mut] st: &mut SharedMemViewState) -> Result<Value> {
            st.released = true;
            st.pending_export = None;
            Ok(Value::none())
        }

        fn tobytes(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            let mut buf = vec![0u8; st.len];
            st.shm.read_bytes(st.offset, &mut buf)?;
            Ok(Value::bytes(buf))
        }

        fn tolist(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            let mut buf = vec![0u8; st.len];
            st.shm.read_bytes(st.offset, &mut buf)?;
            Ok(Value::list(
                buf.into_iter().map(|b| Value::int(b as i64)).collect(),
            ))
        }

        fn hex(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            let mut buf = vec![0u8; st.len];
            st.shm.read_bytes(st.offset, &mut buf)?;
            let mut out = String::with_capacity(buf.len() * 2);
            for b in buf {
                use std::fmt::Write as _;
                let _ = write!(&mut out, "{b:02x}");
            }
            Ok(Value::string(&out))
        }

        fn __len__(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            Ok(Value::int(st.len as i64))
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            let (shm, owner, base_off, len) = with_state(self_val.0, |st| {
                st.ensure_alive()?;
                Ok((Arc::clone(&st.shm), st.owner.clone(), st.offset, st.len))
            })?;
            if let Some((lo, hi, stp)) = pyrust_builtins::slice::slice_fields(&key.0) {
                let (start, stop, step) =
                    pyrust_builtins::slice::compute_indices(len as i64, &lo, &hi, &stp)?;
                if step == 1 {
                    let s = start as usize;
                    let e = (stop as usize).max(s);
                    return make_shm_view(_interp, shm, owner, base_off + s, e - s);
                }
                let indices = collect_slice_indices(start, stop, step);
                let mut out = Vec::with_capacity(indices.len());
                for idx in indices {
                    out.push(shm.read_byte(base_off + idx)?);
                }
                return Ok(Value::bytes(out));
            }
            let raw_idx = _interp.call_index_protocol(&key.0, "memoryview indices")?;
            let idx_i64 = raw_idx
                .as_int()
                .ok_or_else(|| PyError::named("IndexError", "index out of range"))?;
            let norm = if idx_i64 < 0 {
                idx_i64 + len as i64
            } else {
                idx_i64
            };
            if norm < 0 || norm >= len as i64 {
                return Err(PyError::named("IndexError", "index out of bounds"));
            }
            let b = shm.read_byte(base_off + norm as usize)?;
            Ok(Value::int(b as i64))
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let (shm, base_off, len) = with_state(self_val.0, |st| {
                st.ensure_alive()?;
                Ok((Arc::clone(&st.shm), st.offset, st.len))
            })?;
            if let Some((lo, hi, stp)) = pyrust_builtins::slice::slice_fields(&key.0) {
                let rhs = extract_buffer_bytes(_interp, &value.0)?;
                let (start, stop, step) =
                    pyrust_builtins::slice::compute_indices(len as i64, &lo, &hi, &stp)?;
                if step == 1 {
                    let s = start as usize;
                    let e = (stop as usize).max(s);
                    if e - s != rhs.len() {
                        return Err(PyError::named(
                            "ValueError",
                            "memoryview assignment: lvalue and rvalue have different structures",
                        ));
                    }
                    shm.write_bytes(base_off + s, &rhs)?;
                } else {
                    let indices = collect_slice_indices(start, stop, step);
                    if indices.len() != rhs.len() {
                        return Err(PyError::named(
                            "ValueError",
                            "memoryview assignment: lvalue and rvalue have different structures",
                        ));
                    }
                    for (&idx, &b) in indices.iter().zip(rhs.iter()) {
                        shm.write_byte(base_off + idx, b)?;
                    }
                }
                return Ok(Value::none());
            }
            let raw_idx = _interp.call_index_protocol(&key.0, "memoryview indices")?;
            let idx_i64 = raw_idx
                .as_int()
                .ok_or_else(|| PyError::named("IndexError", "index out of range"))?;
            let norm = if idx_i64 < 0 {
                idx_i64 + len as i64
            } else {
                idx_i64
            };
            if norm < 0 || norm >= len as i64 {
                return Err(PyError::named("IndexError", "index out of bounds"));
            }
            let b_val = match value.0.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => return Err(PyError::named("TypeError", "an integer is required")),
            };
            if !(0..=255).contains(&b_val) {
                return Err(PyError::named("ValueError", "byte must be in range(0, 256)"));
            }
            shm.write_byte(base_off + norm as usize, b_val as u8)?;
            Ok(Value::none())
        }

        fn __iter__(#[state] st: &SharedMemViewState) -> Result<Value> {
            st.ensure_alive()?;
            let mut buf = vec![0u8; st.len];
            st.shm.read_bytes(st.offset, &mut buf)?;
            let items: Vec<Value> = buf.into_iter().map(|b| Value::int(b as i64)).collect();
            Ok(Value::generator(Box::new(NativeIterFrame::generator(items))))
        }

        fn __eq__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] other: PyValue,
        ) -> Result<Value> {
            let (shm, base_off, len) = with_state(self_val.0, |st| {
                st.ensure_alive()?;
                Ok((Arc::clone(&st.shm), st.offset, st.len))
            })?;
            let Ok(rhs) = extract_buffer_bytes(_interp, &other.0) else {
                return Ok(Value::bool_(false));
            };
            let mut lhs = vec![0u8; len];
            shm.read_bytes(base_off, &mut lhs)?;
            Ok(Value::bool_(lhs == rhs))
        }

        fn __buffer__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] flags: PyIndex,
        ) -> Result<Value> {
            let fl = flags.expect_i64(FN_NAME, "flags")?;
            let _ = fl & BUF_WRITABLE;
            with_state_mut(self_val.0, |st| {
                st.ensure_alive()?;
                let mut buf = vec![0u8; st.len];
                st.shm.read_bytes(st.offset, &mut buf)?;
                let data_rc = Rc::new(RefCell::new(buf));
                st.pending_export = Some(Rc::clone(&data_rc));
                pyrust_builtins::memoryview::from_mutable_buffer(
                    data_rc,
                    None,
                    self_val.0.clone(),
                    "B",
                    false,
                )
            })
        }

        fn __release_buffer__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] _mv: PyValueRef<'_>,
        ) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                if let Some(data_rc) = st.pending_export.take()
                    && !st.shm.is_closed()
                {
                    let bytes = data_rc.borrow();
                    let write_len = bytes.len().min(st.len);
                    let _ = st.shm.write_bytes(st.offset, &bytes[..write_len]);
                }
                Ok(Value::none())
            })
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state(self_val.0, |st| st.ensure_alive())?;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                st.released = true;
                st.pending_export = None;
                Ok(())
            })?;
            Ok(Value::none())
        }
    }

    #[native_state(SharedMemoryState)]
    class SharedMemory {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(false.into())] create: PyBool,
            #[default(0i64.into())] size: PyIndex,
        ) -> Result<Value> {
            let name_opt = match name.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => Some(
                    v.as_str()
                        .ok_or_else(|| PyError::named("TypeError", "name must be str or None"))?
                        .to_string(),
                ),
            };
            let sz_i64 = size.expect_i64(FN_NAME, "size")?;
            if sz_i64 < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "'size' must be a positive integer",
                ));
            }
            let shm = OsSharedMem::open(name_opt.as_deref(), create.0, sz_i64 as usize, false)?;
            let buf_view = make_shm_view(_interp, Arc::clone(&shm), self_val.0.clone(), 0, shm.size)?;
            set_state(
                self_val.0,
                SharedMemoryState {
                    shm,
                    buf_obj: Some(buf_view),
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn name(#[state] st: &SharedMemoryState) -> Result<Value> {
            Ok(Value::string(&st.shm.name))
        }

        #[getter]
        fn size(#[state] st: &SharedMemoryState) -> Result<Value> {
            Ok(Value::int(st.shm.size as i64))
        }

        #[getter]
        fn buf(#[state] st: &SharedMemoryState) -> Result<Value> {
            Ok(st.buf_obj.clone().unwrap_or_else(Value::none))
        }

        fn close(#[state_mut] st: &mut SharedMemoryState) -> Result<Value> {
            if let Some(buf) = st.buf_obj.take() {
                let _ = crate::builtin_modules::macros::native_state::with_native_state_mut::<
                    SharedMemViewState,
                    _,
                >(&buf, "_SharedMemoryView", |v| {
                    v.released = true;
                    v.pending_export = None;
                    Ok(())
                });
            }
            st.shm.close();
            Ok(Value::none())
        }

        fn unlink(#[state] st: &SharedMemoryState) -> Result<Value> {
            st.shm.unlink()?;
            Ok(Value::none())
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let name = with_state(self_val.0, |st| Ok(st.shm.name.clone()))?;
            let m = _interp.load_module("multiprocessing.shared_memory")?;
            let cls = _interp.get_attr(&m, "SharedMemory")?;
            Ok(Value::tuple(vec![cls, Value::tuple(vec![Value::string(&name)])]))
        }

        fn __repr__(#[state] st: &SharedMemoryState) -> Result<Value> {
            Ok(Value::string(format!(
                "SharedMemory({:?}, size={})",
                st.shm.name, st.shm.size
            )))
        }
    }

    #[native_state(ShareableListState)]
    class ShareableList {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] sequence: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
        ) -> Result<Value> {
            let name_opt = match name.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => Some(
                    v.as_str()
                        .ok_or_else(|| PyError::named("TypeError", "name must be str or None"))?
                        .to_string(),
                ),
            };
            let sm_mod = _interp.load_module("multiprocessing.shared_memory")?;
            let shm_cls = _interp.get_attr(&sm_mod, "SharedMemory")?;

            if let Some(seq_val) = sequence.as_value()
                && !seq_val.is_none()
            {
                let items = _interp.collect_iterable(seq_val)?;
                let count = items.len();
                // Header: 4B magic + 4B count + count * 8B (offset: u32, cap: u32)
                let table_end = 8 + count * 8;
                let mut slots = Vec::with_capacity(count);
                let mut fmt_parts = Vec::with_capacity(count);
                let mut cur_off = table_end;
                for it in &items {
                    let (cap, fcode, tag) = sl_item_format_code(it)?;
                    slots.push((cur_off, cap, tag));
                    fmt_parts.push(fcode);
                    // Each slot stores 1B tag + 4B actual len + `cap` bytes
                    cur_off += 5 + cap;
                }
                let total_size = cur_off.max(16);
                let name_arg = name_opt
                    .as_deref()
                    .map(Value::string)
                    .unwrap_or_else(Value::none);
                let shm_val = crate::builtin_modules::mp_rt::call_pos(
                    _interp,
                    shm_cls,
                    &[name_arg, Value::bool_(true), Value::int(total_size as i64)],
                )?;
                let shm = with_native_state::<SharedMemoryState, _>(
                    &shm_val,
                    "SharedMemory",
                    |s| Ok(Arc::clone(&s.shm)),
                )?;

                shm.write_bytes(0, SL_MAGIC)?;
                shm.write_bytes(4, &(count as u32).to_le_bytes())?;
                for (i, &(off, cap, _)) in slots.iter().enumerate() {
                    let mut entry = [0u8; 8];
                    entry[0..4].copy_from_slice(&(off as u32).to_le_bytes());
                    entry[4..8].copy_from_slice(&(cap as u32).to_le_bytes());
                    shm.write_bytes(8 + i * 8, &entry)?;
                }
                for (it, &(off, cap, _)) in items.iter().zip(slots.iter()) {
                    sl_write_item(&shm, off, cap, it)?;
                }
                set_state(
                    self_val.0,
                    ShareableListState {
                        shm_val,
                        shm,
                        count,
                        slots,
                        format: fmt_parts.join(""),
                    },
                )?;
                return Ok(Value::none());
            }

            // Attach to existing ShareableList by name.
            let Some(attach_name) = name_opt else {
                return Err(PyError::named(
                    "ValueError",
                    "'name' must be provided when sequence is None",
                ));
            };
            let shm_val = crate::builtin_modules::mp_rt::call_pos(
                _interp,
                shm_cls,
                &[Value::string(&attach_name), Value::bool_(false)],
            )?;
            let shm = with_native_state::<SharedMemoryState, _>(
                &shm_val,
                "SharedMemory",
                |s| Ok(Arc::clone(&s.shm)),
            )?;
            let mut magic = [0u8; 4];
            shm.read_bytes(0, &mut magic)?;
            if &magic != SL_MAGIC {
                return Err(PyError::named(
                    "ValueError",
                    "invalid ShareableList shared memory header",
                ));
            }
            let mut cnt_buf = [0u8; 4];
            shm.read_bytes(4, &mut cnt_buf)?;
            let count = u32::from_le_bytes(cnt_buf) as usize;
            let mut slots = Vec::with_capacity(count);
            for i in 0..count {
                let mut entry = [0u8; 8];
                shm.read_bytes(8 + i * 8, &mut entry)?;
                let off = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
                let cap = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as usize;
                let tag = shm.read_byte(off)?;
                slots.push((off, cap, tag));
            }
            set_state(
                self_val.0,
                ShareableListState {
                    shm_val,
                    shm,
                    count,
                    slots,
                    format: String::new(),
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn shm(#[state] st: &ShareableListState) -> Result<Value> {
            Ok(st.shm_val.clone())
        }

        #[getter]
        fn format(#[state] st: &ShareableListState) -> Result<Value> {
            Ok(Value::string(&st.format))
        }

        fn __len__(#[state] st: &ShareableListState) -> Result<Value> {
            Ok(Value::int(st.count as i64))
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] position: PyIndex,
        ) -> Result<Value> {
            let idx_i64 = position.expect_i64(FN_NAME, "position")?;
            with_state(self_val.0, |st| {
                let norm = if idx_i64 < 0 {
                    idx_i64 + st.count as i64
                } else {
                    idx_i64
                };
                if norm < 0 || norm >= st.count as i64 {
                    return Err(PyError::named("IndexError", "index out of range"));
                }
                let (off, _, _) = st.slots[norm as usize];
                sl_read_item(&st.shm, off)
            })
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] position: PyIndex,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let idx_i64 = position.expect_i64(FN_NAME, "position")?;
            with_state(self_val.0, |st| {
                let norm = if idx_i64 < 0 {
                    idx_i64 + st.count as i64
                } else {
                    idx_i64
                };
                if norm < 0 || norm >= st.count as i64 {
                    return Err(PyError::named("IndexError", "index out of range"));
                }
                let (off, cap, _) = st.slots[norm as usize];
                sl_write_item(&st.shm, off, cap, &value.0)?;
                Ok(Value::none())
            })
        }

        fn __iter__(#[state] st: &ShareableListState) -> Result<Value> {
            let mut items = Vec::with_capacity(st.count);
            for &(off, _, _) in &st.slots {
                items.push(sl_read_item(&st.shm, off)?);
            }
            Ok(Value::generator(Box::new(NativeIterFrame::generator(items))))
        }

        fn count(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let items: Vec<Value> = with_state(self_val.0, |st| {
                let mut out = Vec::with_capacity(st.count);
                for &(off, _, _) in &st.slots {
                    out.push(sl_read_item(&st.shm, off)?);
                }
                Ok(out)
            })?;
            let mut total = 0i64;
            for item in items {
                if crate::builtin_modules::mp_rt::py_eq(_interp, item, value.0.clone())? {
                    total += 1;
                }
            }
            Ok(Value::int(total))
        }

        fn index(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let items: Vec<Value> = with_state(self_val.0, |st| {
                let mut out = Vec::with_capacity(st.count);
                for &(off, _, _) in &st.slots {
                    out.push(sl_read_item(&st.shm, off)?);
                }
                Ok(out)
            })?;
            for (i, item) in items.iter().enumerate() {
                if crate::builtin_modules::mp_rt::py_eq(_interp, item.clone(), value.0.clone())? {
                    return Ok(Value::int(i as i64));
                }
            }
            Err(PyError::named("ValueError", "value not in ShareableList"))
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let name = with_state(self_val.0, |st| Ok(st.shm.name.clone()))?;
            let m = _interp.load_module("multiprocessing.shared_memory")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_shareable_list")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![Value::string(&name)]),
            ]))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (items, name) = with_state(self_val.0, |st| {
                let mut out = Vec::with_capacity(st.count);
                for &(off, _, _) in &st.slots {
                    out.push(sl_read_item(&st.shm, off)?);
                }
                Ok((out, st.shm.name.clone()))
            })?;
            let mut parts = Vec::with_capacity(items.len());
            for it in &items {
                parts.push(render_value_repr(_interp, it)?);
            }
            Ok(Value::string(format!(
                "ShareableList([{}], name={name:?})",
                parts.join(", ")
            )))
        }
    }

    fn _rebuild_shareable_list(
        #[positional_only] name: crate::interpreter::builtin_args::PyStr<'_>,
    ) -> Result<Value> {
        let m = _interp.load_module("multiprocessing.shared_memory")?;
        let cls = _interp.get_attr(&m, "ShareableList")?;
        crate::builtin_modules::mp_rt::call_pos_kw(
            _interp,
            cls,
            &[],
            &[("name".to_string(), Value::string(name.0))],
        )
    }
}
