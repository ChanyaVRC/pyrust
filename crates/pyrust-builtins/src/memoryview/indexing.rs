//! Indexing, slicing, assignment, and equality helpers for `memoryview`.

use std::rc::Rc;

use pyrust_core::{PyError, Result, Value, ValueKind};

use super::format::{NativeFormat, pack_element, unpack_element};
use super::{BufferBacking, MEMORYVIEW_OPS, MemoryViewState, is_memoryview_ops, released_error};

fn normalize_dim_index(key: &Value, dim_len: usize) -> Result<usize> {
    let raw: i64 = match key.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        ValueKind::BigInt(_) => {
            return Err(PyError::named(
                "IndexError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            ));
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "memoryview: invalid slice key (got {})",
                    pyrust_core::builtin_type_name(key)
                ),
            ));
        }
    };
    let resolved = if raw < 0 {
        let from_end = raw.unsigned_abs();
        if from_end > dim_len as u64 {
            return Err(PyError::named(
                "IndexError",
                "index out of bounds on dimension 1".to_string(),
            ));
        }
        dim_len - from_end as usize
    } else {
        let u = raw as u64;
        if u >= dim_len as u64 {
            return Err(PyError::named(
                "IndexError",
                "index out of bounds on dimension 1".to_string(),
            ));
        }
        u as usize
    };
    Ok(resolved)
}

fn normalize_nd_index(key: &Value, dim_len: usize, dim_1based: usize) -> Result<usize> {
    let raw: i64 = match key.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        ValueKind::BigInt(_) => {
            return Err(PyError::named(
                "IndexError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            ));
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                "memoryview: invalid slice key".to_string(),
            ));
        }
    };
    let resolved = if raw < 0 {
        let from_end = raw.unsigned_abs();
        if from_end > dim_len as u64 {
            return Err(PyError::named(
                "IndexError",
                format!("index out of bounds on dimension {dim_1based}"),
            ));
        }
        dim_len - from_end as usize
    } else {
        let u = raw as u64;
        if u >= dim_len as u64 {
            return Err(PyError::named(
                "IndexError",
                format!("index out of bounds on dimension {dim_1based}"),
            ));
        }
        u as usize
    };
    Ok(resolved)
}

fn slice_count(start: i64, stop: i64, step: i64) -> usize {
    let span = i128::from(stop) - i128::from(start);
    let step128 = i128::from(step);
    let count = if step > 0 {
        if span > 0 {
            ((span - 1) / step128) + 1
        } else {
            0
        }
    } else if span < 0 {
        ((span + 1) / step128) + 1
    } else {
        0
    };
    count.max(0) as usize
}

pub(super) fn get_item(mv: &MemoryViewState, key: &Value) -> Result<Value> {
    if mv.released {
        return Err(released_error());
    }
    let ndim = mv.shape.len();
    // Slice subscript
    if let ValueKind::BuiltinObject {
        ops,
        state: slice_state,
    } = key.kind()
        && crate::slice::is_slice_ops(ops)
    {
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
        let sb = slice_state.borrow();
        let sl = sb
            .downcast_ref::<crate::slice::SliceState>()
            .expect("slice state");
        let len = mv.shape[0] as i64;
        let (start, stop, step) =
            crate::slice::compute_indices(len, &sl.start, &sl.stop, &sl.step)?;
        drop(sb);
        let new_len = slice_count(start, stop, step);
        let new_offset = if new_len == 0 {
            mv.offset
        } else {
            mv.offset + (start as isize) * mv.strides[0]
        };
        let new_stride = mv.strides[0] * (step as isize);
        let derived = mv.derive_view(super::ViewLayout {
            readonly: mv.readonly,
            format: mv.format.clone(),
            parsed_format: mv.parsed_format,
            itemsize: mv.itemsize,
            offset: new_offset,
            shape: vec![new_len],
            strides: vec![new_stride],
        });
        return Ok(Value::builtin_object(MEMORYVIEW_OPS, Box::new(derived)));
    }

    // Tuple subscript (supports 0D `mv[()]`, 1D `mv[(i,)]`, ND `mv[i, j, ...]`)
    if let ValueKind::Tuple(items) = key.kind() {
        if items.len() == ndim {
            let mut byte_off = mv.offset;
            for (d, (idx_val, (&dim_len, &stride))) in items
                .iter()
                .zip(mv.shape.iter().zip(mv.strides.iter()))
                .enumerate()
            {
                let idx = normalize_nd_index(idx_val, dim_len, d + 1)?;
                byte_off += (idx as isize) * stride;
            }
            return mv.read_element_at(byte_off);
        }
        if ndim == 0 {
            return Err(PyError::named(
                "TypeError",
                "invalid indexing of 0-dim memory".to_string(),
            ));
        }
        if items.len() < ndim {
            return Err(PyError::named(
                "NotImplementedError",
                "multi-dimensional sub-views are not implemented".to_string(),
            ));
        }
        return Err(PyError::named(
            "TypeError",
            format!(
                "cannot index {ndim}-dimension view with {}-element tuple",
                items.len()
            ),
        ));
    }

    // Scalar integer subscript
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
    let idx = normalize_dim_index(key, mv.shape[0])?;
    let byte_off = mv.offset + (idx as isize) * mv.strides[0];
    mv.read_element_at(byte_off)
}

/// Extract (format, itemsize, ndim, shape, packed C-order bytes) from a RHS buffer
/// for memoryview slice assignment.
struct RhsBuffer {
    format: NativeFormat,
    itemsize: usize,
    shape: Vec<usize>,
    bytes: Vec<u8>,
}

fn extract_rhs_buffer(val: &Value) -> Result<RhsBuffer> {
    match val.kind() {
        ValueKind::Bytes(rc) => Ok(RhsBuffer {
            format: NativeFormat::UnsignedChar,
            itemsize: 1,
            shape: vec![rc.len()],
            bytes: rc.as_ref().clone(),
        }),
        ValueKind::BuiltinObject { ops, state: _ }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            let snap = crate::bytearray::as_bytearray_snapshot(val).unwrap_or_default();
            let len = snap.len();
            Ok(RhsBuffer {
                format: NativeFormat::UnsignedChar,
                itemsize: 1,
                shape: vec![len],
                bytes: snap,
            })
        }
        ValueKind::BuiltinObject { ops, state } if is_memoryview_ops(ops) => {
            let borrow = state.borrow();
            let rhs = borrow
                .downcast_ref::<MemoryViewState>()
                .expect("memoryview state");
            if rhs.released {
                return Err(released_error());
            }
            Ok(RhsBuffer {
                format: rhs.parsed_format,
                itemsize: rhs.itemsize,
                shape: rhs.shape.clone(),
                bytes: rhs.tobytes_order('C')?,
            })
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "memoryview: a bytes-like object is required, not '{}'",
                pyrust_core::builtin_type_name(val)
            ),
        )),
    }
}

pub(super) fn set_item(mv: &MemoryViewState, key: &Value, value: Value) -> Result<()> {
    if mv.released {
        return Err(released_error());
    }
    if mv.readonly {
        return Err(PyError::named(
            "TypeError",
            "cannot modify read-only memory".to_string(),
        ));
    }
    let ndim = mv.shape.len();

    // Slice assignment: `mv[a:b:step] = rhs`
    if let ValueKind::BuiltinObject {
        ops,
        state: slice_state,
    } = key.kind()
        && crate::slice::is_slice_ops(ops)
    {
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
        let sb = slice_state.borrow();
        let sl = sb
            .downcast_ref::<crate::slice::SliceState>()
            .expect("slice state");
        let len = mv.shape[0] as i64;
        let (start, stop, step) =
            crate::slice::compute_indices(len, &sl.start, &sl.stop, &sl.step)?;
        drop(sb);
        let target_len = slice_count(start, stop, step);
        let rhs = extract_rhs_buffer(&value)?;
        if rhs.shape.len() != 1
            || rhs.shape[0] != target_len
            || rhs.itemsize != mv.itemsize
            || rhs.format != mv.parsed_format
        {
            return Err(PyError::named(
                "ValueError",
                "memoryview assignment: lvalue and rvalue have different structures".to_string(),
            ));
        }
        if target_len == 0 {
            return Ok(());
        }
        let first_offset = mv.offset + (start as isize) * mv.strides[0];
        let elem_stride = mv.strides[0] * (step as isize);
        mv.write_1d_strided(first_offset, elem_stride, target_len, &rhs.bytes)?;
        return Ok(());
    }

    // Tuple assignment: `mv[()] = v` or `mv[i, j] = v`
    if let ValueKind::Tuple(items) = key.kind() {
        if items.len() == ndim {
            let mut byte_off = mv.offset;
            for (d, (idx_val, (&dim_len, &stride))) in items
                .iter()
                .zip(mv.shape.iter().zip(mv.strides.iter()))
                .enumerate()
            {
                let idx = normalize_nd_index(idx_val, dim_len, d + 1)?;
                byte_off += (idx as isize) * stride;
            }
            let packed = pack_element(mv.parsed_format, &mv.format, &value)?;
            return mv.write_bytes_at(byte_off, &packed);
        }
        if ndim == 0 {
            return Err(PyError::named(
                "TypeError",
                "invalid indexing of 0-dim memory".to_string(),
            ));
        }
        if items.len() < ndim {
            return Err(PyError::named(
                "NotImplementedError",
                "multi-dimensional sub-views are not implemented".to_string(),
            ));
        }
        return Err(PyError::named(
            "TypeError",
            format!(
                "cannot index {ndim}-dimension view with {}-element tuple",
                items.len()
            ),
        ));
    }

    // Scalar integer assignment
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
    let idx = normalize_dim_index(key, mv.shape[0])?;
    let byte_off = mv.offset + (idx as isize) * mv.strides[0];
    let packed = pack_element(mv.parsed_format, &mv.format, &value)?;
    mv.write_bytes_at(byte_off, &packed)
}

/// Compare an active `MemoryViewState` against raw unsigned byte slice (`bytes` / `bytearray`).
pub(super) fn eq_with_raw_bytes(mv: &MemoryViewState, other: &[u8]) -> bool {
    if mv.released {
        return false;
    }
    if mv.shape.len() != 1 || mv.shape[0] != other.len() {
        return false;
    }
    let n = other.len();
    if n == 0 {
        return true;
    }
    mv.with_bytes(|buf| {
        if mv.parsed_format.is_unsigned_byte() {
            if mv.strides[0] == 1 {
                let start = mv.offset as usize;
                return &buf[start..start + n] == other;
            }
            for (i, &b) in other.iter().enumerate() {
                let pos = (mv.offset + (i as isize) * mv.strides[0]) as usize;
                if buf[pos] != b {
                    return false;
                }
            }
            return true;
        }
        for (i, &b) in other.iter().enumerate() {
            let pos = (mv.offset + (i as isize) * mv.strides[0]) as usize;
            let elem = unpack_element(mv.parsed_format, &buf[pos..pos + mv.itemsize]);
            if elem != Value::int(i64::from(b)) {
                return false;
            }
        }
        true
    })
}

/// Compare two active `MemoryViewState`s by shape and element values (CPython 3.12 parity).
pub(super) fn eq_memoryviews(a: &MemoryViewState, b: &MemoryViewState) -> bool {
    if a.released || b.released {
        return false;
    }
    if a.shape != b.shape {
        return false;
    }
    let total_elems: usize = a.shape.iter().product();
    if total_elems == 0 {
        return true;
    }
    // Fast path: both are contiguous unsigned bytes ('B')
    if a.parsed_format.is_unsigned_byte()
        && b.parsed_format.is_unsigned_byte()
        && a.is_c_contiguous()
        && b.is_c_contiguous()
    {
        let a_nbytes = a.nbytes();
        let a_off = a.offset as usize;
        let b_off = b.offset as usize;
        return a.with_bytes(|abuf| {
            b.with_bytes(|bbuf| abuf[a_off..a_off + a_nbytes] == bbuf[b_off..b_off + a_nbytes])
        });
    }
    // General path: compare element-by-element in C order.
    let a_bytes = match a.tobytes_order('C') {
        Ok(v) => v,
        Err(_) => return false,
    };
    let b_bytes = match b.tobytes_order('C') {
        Ok(v) => v,
        Err(_) => return false,
    };
    if a.parsed_format == b.parsed_format
        && !matches!(
            a.parsed_format,
            NativeFormat::Float16 | NativeFormat::Float32 | NativeFormat::Float64
        )
    {
        return a_bytes == b_bytes;
    }
    for i in 0..total_elems {
        let av = unpack_element(
            a.parsed_format,
            &a_bytes[i * a.itemsize..(i + 1) * a.itemsize],
        );
        let bv = unpack_element(
            b.parsed_format,
            &b_bytes[i * b.itemsize..(i + 1) * b.itemsize],
        );
        if av != bv {
            return false;
        }
    }
    true
}

/// Helper for comparing a `memoryview` with a `bytearray` without risking a
/// double borrow when both share the same `Rc<RefCell<Vec<u8>>>`.
pub(super) fn eq_with_bytearray(
    mv: &MemoryViewState,
    ba_rc: &Rc<std::cell::RefCell<Vec<u8>>>,
) -> bool {
    if mv.released {
        return false;
    }
    let same_rc = {
        let managed = mv.managed.borrow();
        matches!(&managed.backing, BufferBacking::Mutable { data, .. } if Rc::ptr_eq(data, ba_rc))
    };
    if same_rc {
        // Borrow once!
        let buf = ba_rc.borrow();
        if mv.shape.len() != 1 || mv.shape[0] != buf.len() {
            return false;
        }
        let n = buf.len();
        if n == 0 {
            return true;
        }
        if mv.parsed_format.is_unsigned_byte() {
            if mv.offset == 0 && mv.strides[0] == 1 {
                return true;
            }
            for i in 0..n {
                let pos = (mv.offset + (i as isize) * mv.strides[0]) as usize;
                if buf[pos] != buf[i] {
                    return false;
                }
            }
            return true;
        }
        for i in 0..n {
            let pos = (mv.offset + (i as isize) * mv.strides[0]) as usize;
            let elem = unpack_element(mv.parsed_format, &buf[pos..pos + mv.itemsize]);
            if elem != Value::int(i64::from(buf[i])) {
                return false;
            }
        }
        return true;
    }
    let other = ba_rc.borrow();
    eq_with_raw_bytes(mv, &other)
}
