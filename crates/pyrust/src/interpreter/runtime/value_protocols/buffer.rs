// PEP 3118 / PEP 688 buffer protocol helpers.
//
// Centralizes buffer acquisition (`__buffer__`) and release (`__release_buffer__`)
// for `memoryview()`, `bytes()`, `bytearray()`, `struct`, `io.BytesIO`,
// `binascii`, `zlib`, `hashlib`, etc.

/// PEP 688 `inspect.BufferFlags` constants.
pub(crate) const BUF_WRITABLE: i64 = 0x1;
pub(crate) const BUF_FULL_RO: i64 = 0x118 | 0x4;

/// Acquire a `memoryview` from any buffer-exporting object using PEP 3118 / PEP 688.
///
/// Built-in exporters (`bytes`, `bytearray`, `memoryview`, and their subclasses)
/// take fast native paths; user-defined classes (including `array.array`) are
/// queried through `type(obj).__buffer__(obj, flags)` and wrapped so releasing
/// the view invokes `type(obj).__release_buffer__(obj, inner_mv)` when defined.
pub(crate) fn acquire_buffer_view(
    interp: &mut Interpreter,
    obj: &Value,
    flags: i64,
) -> Result<Value> {
    let want_writable = (flags & BUF_WRITABLE) != 0;
    match obj.kind() {
        ValueKind::Bytes(rc) => {
            if want_writable {
                return Err(PyError::named(
                    "BufferError",
                    "Object is not writable.".to_string(),
                ));
            }
            Ok(pyrust_builtins::memoryview::from_bytes(
                Rc::clone(rc),
                obj.clone(),
            ))
        }
        ValueKind::BuiltinObject { ops, .. }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            let (data, exports) =
                pyrust_builtins::bytearray::as_bytearray_buffer(obj).expect("bytearray buffer");
            pyrust_builtins::memoryview::from_mutable_buffer(
                data,
                Some(exports),
                obj.clone(),
                "B",
                false,
            )
        }
        ValueKind::BuiltinObject { ops, .. }
            if pyrust_builtins::memoryview::is_memoryview_ops(ops) =>
        {
            if want_writable {
                // Check released + writable via writable_contiguous_range or readonly attr.
                if pyrust_builtins::memoryview::is_released(obj) {
                    return Err(pyrust_builtins::memoryview::released_error());
                }
                if let Some(ro) = pyrust_builtins::memoryview::getattr_checked(obj, "readonly")?
                    && matches!(ro.kind(), ValueKind::Bool(true))
                {
                    return Err(PyError::named(
                        "BufferError",
                        "Object is not writable.".to_string(),
                    ));
                }
            }
            pyrust_builtins::memoryview::from_memoryview(obj)
        }
        ValueKind::PyInstance(_) => {
            // 1. Check for a user-defined (or stdlib Python-class, e.g. `array.array`)
            //    `__buffer__` method on the class MRO first.
            if let Some(buffer_method) =
                lookup_value_special_method(obj, "__buffer__").transpose()?
            {
                let is_inherited_builtin = matches!(
                    buffer_method.kind(),
                    ValueKind::BuiltinFunction("bytes.__buffer__" | "bytearray.__buffer__")
                );
                if !is_inherited_builtin {
                    let inner_mv = invoke_class_method(
                        interp,
                        buffer_method,
                        obj.clone(),
                        &[ExpandedCallArg {
                            name: None,
                            value: Value::int(flags),
                        }],
                    )?;
                    if !pyrust_builtins::memoryview::is_memoryview(&inner_mv) {
                        return Err(PyError::named(
                            "TypeError",
                            "__buffer__ returned non-memoryview object".to_string(),
                        ));
                    }
                    if want_writable
                        && let Some(ro) =
                            pyrust_builtins::memoryview::getattr_checked(&inner_mv, "readonly")?
                        && matches!(ro.kind(), ValueKind::Bool(true))
                    {
                        release_memoryview_with_interp(interp, &inner_mv)?;
                        return Err(PyError::named(
                            "BufferError",
                            "Object is not writable.".to_string(),
                        ));
                    }
                    let has_release_hook = lookup_value_special_method(obj, "__release_buffer__")
                        .transpose()?
                        .is_some();
                    return pyrust_builtins::memoryview::wrap_pep688_view(
                        obj.clone(),
                        inner_mv,
                        has_release_hook,
                    );
                }
            }
            // 2. Fallback for `bytes` / `bytearray` subclasses backed by `__builtin_data__`.
            if let Some(backing) = builtin_data_backing(obj) {
                match backing.kind() {
                    ValueKind::Bytes(rc) => {
                        if want_writable {
                            return Err(PyError::named(
                                "BufferError",
                                "Object is not writable.".to_string(),
                            ));
                        }
                        return Ok(pyrust_builtins::memoryview::from_bytes(
                            Rc::clone(rc),
                            obj.clone(),
                        ));
                    }
                    ValueKind::BuiltinObject { ops, .. }
                        if ops.canonical_class_tag()
                            == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
                    {
                        let (data, exports) =
                            pyrust_builtins::bytearray::as_bytearray_buffer(&backing)
                                .expect("bytearray buffer");
                        return pyrust_builtins::memoryview::from_mutable_buffer(
                            data,
                            Some(exports),
                            obj.clone(),
                            "B",
                            false,
                        );
                    }
                    _ => {}
                }
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "memoryview: a bytes-like object is required, not '{}'",
                    value_type_name_str(obj)
                ),
            ))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "memoryview: a bytes-like object is required, not '{}'",
                value_type_name_str(obj)
            ),
        )),
    }
}

/// Release a `memoryview` and, if it was the last view on a PEP 688 managed
/// buffer with `__release_buffer__`, invoke `owner.__release_buffer__(inner_mv)`.
pub(crate) fn release_memoryview_with_interp(interp: &mut Interpreter, mv: &Value) -> Result<()> {
    if let Some((owner, inner_mv)) = pyrust_builtins::memoryview::release_and_take_callback(mv)
        && let Some(rel_method) =
            lookup_value_special_method(&owner, "__release_buffer__").transpose()?
    {
        let _ = invoke_class_method(
            interp,
            rel_method,
            owner,
            &[ExpandedCallArg {
                name: None,
                value: inner_mv,
            }],
        )?;
    }
    Ok(())
}

/// Try to extract read-only C-order bytes from any bytes-like / PEP 688 buffer object.
///
/// Returns `Ok(None)` when `obj` does not implement the buffer protocol.
/// Propagates `ValueError` (e.g. released `memoryview`) or exceptions raised by
/// a user `__buffer__` / `__release_buffer__` implementation.
pub(crate) fn try_extract_buffer_bytes(
    interp: &mut Interpreter,
    obj: &Value,
) -> Result<Option<Vec<u8>>> {
    match obj.kind() {
        ValueKind::Bytes(rc) => Ok(Some(rc.as_ref().clone())),
        ValueKind::BuiltinObject { ops, .. }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            Ok(pyrust_builtins::bytearray::as_bytearray_snapshot(obj))
        }
        ValueKind::BuiltinObject { ops, .. }
            if pyrust_builtins::memoryview::is_memoryview_ops(ops) =>
        {
            Ok(Some(pyrust_builtins::memoryview::to_bytes_checked(obj)?))
        }
        ValueKind::PyInstance(_) => {
            if let Some(buffer_method) =
                lookup_value_special_method(obj, "__buffer__").transpose()?
            {
                let is_inherited_builtin = matches!(
                    buffer_method.kind(),
                    ValueKind::BuiltinFunction("bytes.__buffer__" | "bytearray.__buffer__")
                );
                if !is_inherited_builtin {
                    let mv = acquire_buffer_view(interp, obj, BUF_FULL_RO)?;
                    let bytes_res = pyrust_builtins::memoryview::to_bytes_checked(&mv);
                    let rel_res = release_memoryview_with_interp(interp, &mv);
                    let bytes = bytes_res?;
                    rel_res?;
                    return Ok(Some(bytes));
                }
            }
            if let Some(backing) = builtin_data_backing(obj) {
                if let ValueKind::Bytes(rc) = backing.kind() {
                    return Ok(Some(rc.as_ref().clone()));
                }
                if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
                    return Ok(Some(snap));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// Extract read-only C-order bytes from a bytes-like object, raising CPython's
/// canonical `TypeError: a bytes-like object is required, not '<type>'` if `obj`
/// does not support the buffer protocol.
pub(crate) fn extract_buffer_bytes(interp: &mut Interpreter, obj: &Value) -> Result<Vec<u8>> {
    try_extract_buffer_bytes(interp, obj)?.ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(obj)
            ),
        )
    })
}

/// Handle to a writable contiguous byte buffer (used by `struct.pack_into` and
/// `io.BytesIO.readinto`).
pub(crate) struct WritableBufferHandle {
    pub(crate) data: Rc<RefCell<Vec<u8>>>,
    pub(crate) offset: usize,
    pub(crate) len: usize,
    /// Temporary `memoryview` acquired from a PEP 688 exporter that must be
    /// released when the operation finishes.
    temp_mv: Option<Value>,
}

impl WritableBufferHandle {
    pub(crate) fn release(self, interp: &mut Interpreter) -> Result<()> {
        if let Some(mv) = self.temp_mv {
            release_memoryview_with_interp(interp, &mv)?;
        }
        Ok(())
    }
}

/// Acquire a writable contiguous buffer handle from `bytearray`, writable
/// contiguous `memoryview`, `array.array`, `bytearray` subclass, or custom
/// PEP 688 `__buffer__` exporter.
pub(crate) fn acquire_writable_buffer(
    interp: &mut Interpreter,
    obj: &Value,
) -> Result<WritableBufferHandle> {
    if let Some(rc) = pyrust_builtins::bytearray::as_bytearray_rc(obj) {
        let len = rc.borrow().len();
        return Ok(WritableBufferHandle {
            data: rc,
            offset: 0,
            len,
            temp_mv: None,
        });
    }
    if pyrust_builtins::memoryview::is_memoryview(obj) {
        let (data, offset, len) = pyrust_builtins::memoryview::writable_contiguous_range(obj)?;
        return Ok(WritableBufferHandle {
            data,
            offset,
            len,
            temp_mv: None,
        });
    }
    if matches!(obj.kind(), ValueKind::PyInstance(_)) {
        if let Some(buffer_method) = lookup_value_special_method(obj, "__buffer__").transpose()? {
            let is_inherited_builtin = matches!(
                buffer_method.kind(),
                ValueKind::BuiltinFunction("bytes.__buffer__" | "bytearray.__buffer__")
            );
            if !is_inherited_builtin {
                let mv = acquire_buffer_view(interp, obj, BUF_WRITABLE)?;
                match pyrust_builtins::memoryview::writable_contiguous_range(&mv) {
                    Ok((data, offset, len)) => {
                        return Ok(WritableBufferHandle {
                            data,
                            offset,
                            len,
                            temp_mv: Some(mv),
                        });
                    }
                    Err(e) => {
                        let _ = release_memoryview_with_interp(interp, &mv);
                        return Err(e);
                    }
                }
            }
        }
        if let Some(backing) = builtin_data_backing(obj)
            && let Some(rc) = pyrust_builtins::bytearray::as_bytearray_rc(&backing)
        {
            let len = rc.borrow().len();
            return Ok(WritableBufferHandle {
                data: rc,
                offset: 0,
                len,
                temp_mv: None,
            });
        }
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "argument must be read-write bytes-like object, not {}",
            value_type_name_str(obj)
        ),
    ))
}
