//! Public methods on `memoryview`: `tobytes`, `tolist`, `hex`, `toreadonly`,
//! `cast`, `release`, `__enter__`, `__exit__`.

use indexmap::IndexMap;
use pyrust_core::{PyDict, PyError, PyKey, Result, Value, ValueKind};

use super::format::{NativeFormat, unpack_element};
use super::{MEMORYVIEW_OPS, MemoryViewState, released_error};

pub(super) fn call_tobytes(
    mv: &MemoryViewState,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Value> {
    if mv.released {
        return Err(released_error());
    }
    let total = args.len() + kwargs.len();
    if total > 1 {
        return Err(PyError::named(
            "TypeError",
            format!("tobytes() takes at most 1 argument ({total} given)"),
        ));
    }
    for k in kwargs.keys() {
        if k != "order" {
            return Err(PyError::named(
                "TypeError",
                format!("'{k}' is an invalid keyword argument for tobytes()"),
            ));
        }
    }
    let order_val = args.first().or_else(|| kwargs.get("order"));
    let order_char = match order_val {
        None => 'C',
        Some(v) if v.is_none() => 'C',
        Some(v) => match v.kind() {
            ValueKind::Str("C") => 'C',
            ValueKind::Str("F") => 'F',
            ValueKind::Str("A") => 'A',
            ValueKind::Str(_) => {
                return Err(PyError::named(
                    "ValueError",
                    "order must be 'C', 'F' or 'A'".to_string(),
                ));
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "tobytes() argument 'order' must be str or None, not {}",
                        pyrust_core::builtin_type_name(v)
                    ),
                ));
            }
        },
    };
    let bytes = mv.tobytes_order(order_char)?;
    Ok(Value::bytes(bytes))
}

pub(super) fn call_tolist(
    mv: &MemoryViewState,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Value> {
    if mv.released {
        return Err(released_error());
    }
    if !kwargs.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "memoryview.tolist() takes no keyword arguments".to_string(),
        ));
    }
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!(
                "memoryview.tolist() takes no arguments ({} given)",
                args.len()
            ),
        ));
    }
    mv.with_bytes(|buf| {
        build_nested_list(
            buf,
            mv.parsed_format,
            mv.itemsize,
            mv.offset,
            &mv.shape,
            &mv.strides,
        )
    })
}

fn build_nested_list(
    buf: &[u8],
    fmt: NativeFormat,
    itemsize: usize,
    offset: isize,
    shape: &[usize],
    strides: &[isize],
) -> Result<Value> {
    if shape.is_empty() {
        let pos = offset as usize;
        return Ok(unpack_element(fmt, &buf[pos..pos + itemsize]));
    }
    let dim = shape[0];
    let stride = strides[0];
    let mut out = Vec::with_capacity(dim);
    if shape.len() == 1 {
        for i in 0..dim {
            let pos = (offset + (i as isize) * stride) as usize;
            out.push(unpack_element(fmt, &buf[pos..pos + itemsize]));
        }
    } else {
        for i in 0..dim {
            let sub_offset = offset + (i as isize) * stride;
            out.push(build_nested_list(
                buf,
                fmt,
                itemsize,
                sub_offset,
                &shape[1..],
                &strides[1..],
            )?);
        }
    }
    Ok(Value::list(out))
}

pub(super) fn call_hex(
    mv: &MemoryViewState,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Value> {
    if mv.released {
        return Err(released_error());
    }
    let bytes = mv.tobytes_order('C')?;
    let pk_kwargs: PyDict = kwargs
        .iter()
        .map(|(k, v)| (PyKey::str_from(k), v.clone()))
        .collect();
    crate::bytes::hex_bytes(&bytes, args, &pk_kwargs)
}

pub(super) fn call_toreadonly(
    mv: &MemoryViewState,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Value> {
    if mv.released {
        return Err(released_error());
    }
    if !kwargs.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "memoryview.toreadonly() takes no keyword arguments".to_string(),
        ));
    }
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!(
                "memoryview.toreadonly() takes no arguments ({} given)",
                args.len()
            ),
        ));
    }
    let derived = mv.clone_view(true);
    Ok(Value::builtin_object(MEMORYVIEW_OPS, Box::new(derived)))
}

pub(super) fn call_cast(
    mv: &MemoryViewState,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Value> {
    if mv.released {
        return Err(released_error());
    }
    let total = args.len() + kwargs.len();
    if total > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("cast() takes at most 2 arguments ({total} given)"),
        ));
    }
    for k in kwargs.keys() {
        if k != "format" && k != "shape" {
            return Err(PyError::named(
                "TypeError",
                format!("'{k}' is an invalid keyword argument for cast()"),
            ));
        }
    }
    if !args.is_empty() && kwargs.contains_key("format") {
        return Err(PyError::named(
            "TypeError",
            "argument for cast() given by name ('format') and position (1)".to_string(),
        ));
    }
    if args.len() >= 2 && kwargs.contains_key("shape") {
        return Err(PyError::named(
            "TypeError",
            "argument for cast() given by name ('shape') and position (2)".to_string(),
        ));
    }
    let fmt_val = args
        .first()
        .or_else(|| kwargs.get("format"))
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "cast() missing required argument 'format' (pos 1)".to_string(),
            )
        })?;
    let shape_val = args.get(1).or_else(|| kwargs.get("shape"));

    let ValueKind::Str(fmt_str) = fmt_val.kind() else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "cast() argument 'format' must be str, not {}",
                pyrust_core::builtin_type_name(fmt_val)
            ),
        ));
    };

    if !mv.is_c_contiguous() {
        return Err(PyError::named(
            "TypeError",
            "memoryview: casts are restricted to C-contiguous views".to_string(),
        ));
    }

    let dest_fmt = NativeFormat::parse_for_cast(fmt_str)?;
    let dest_itemsize = dest_fmt.itemsize();

    // CPython 3.12: multi-dimensional cast requires either source or dest to be byte format
    // unless reshaping from/to 1D byte view.
    let nbytes = mv.nbytes();
    if shape_val.is_some() && (mv.shape.contains(&0) || mv.strides.contains(&0)) {
        return Err(PyError::named(
            "TypeError",
            "memoryview: cannot cast view with zeros in shape or strides".to_string(),
        ));
    }
    if !mv.parsed_format.is_byte_format() && !dest_fmt.is_byte_format() {
        return Err(PyError::named(
            "TypeError",
            "memoryview: cannot cast between two non-byte formats".to_string(),
        ));
    }
    if !nbytes.is_multiple_of(dest_itemsize) {
        return Err(PyError::named(
            "TypeError",
            "memoryview: length is not a multiple of itemsize".to_string(),
        ));
    }

    let (new_shape, new_strides) = if let Some(sv) = shape_val {
        let dims: Vec<Value> = match sv.kind() {
            ValueKind::Tuple(t) => t.to_vec(),
            ValueKind::List(l) => l.to_vec(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "shape must be a list or a tuple of ints".to_string(),
                ));
            }
        };
        if dims.len() > 64 {
            return Err(PyError::named(
                "ValueError",
                "memoryview: number of dimensions must not exceed 64".to_string(),
            ));
        }
        let mut parsed_shape = Vec::with_capacity(dims.len());
        let mut product: usize = 1;
        for d in &dims {
            let dim_i64 = match d.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "memoryview.cast(): elements of shape must be integers".to_string(),
                    ));
                }
            };
            if dim_i64 <= 0 {
                return Err(PyError::named(
                    "ValueError",
                    "memoryview.cast(): elements of shape must be integers > 0".to_string(),
                ));
            }
            let dim = dim_i64 as usize;
            product = product.checked_mul(dim).ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    "memoryview: product(shape) * itemsize != buffer size".to_string(),
                )
            })?;
            parsed_shape.push(dim);
        }
        let expected_bytes = product.checked_mul(dest_itemsize).ok_or_else(|| {
            PyError::named(
                "TypeError",
                "memoryview: product(shape) * itemsize != buffer size".to_string(),
            )
        })?;
        if expected_bytes != nbytes {
            return Err(PyError::named(
                "TypeError",
                "memoryview: product(shape) * itemsize != buffer size".to_string(),
            ));
        }
        let strides = compute_c_strides(&parsed_shape, dest_itemsize);
        (parsed_shape, strides)
    } else {
        let count = nbytes / dest_itemsize;
        (vec![count], vec![dest_itemsize as isize])
    };

    let derived = mv.derive_view(super::ViewLayout {
        readonly: mv.readonly,
        format: fmt_str.to_string(),
        parsed_format: dest_fmt,
        itemsize: dest_itemsize,
        offset: mv.offset,
        shape: new_shape,
        strides: new_strides,
    });
    Ok(Value::builtin_object(MEMORYVIEW_OPS, Box::new(derived)))
}

pub(crate) fn compute_c_strides(shape: &[usize], itemsize: usize) -> Vec<isize> {
    if shape.is_empty() {
        return Vec::new();
    }
    let mut strides = vec![0isize; shape.len()];
    let mut stride = itemsize as isize;
    for i in (0..shape.len()).rev() {
        strides[i] = stride;
        stride *= shape[i] as isize;
    }
    strides
}
