// `_ctypes` module — 100% native-Rust CPython 3.12 low-level FFI core (issue #3265).
//
// ZERO `libffi`, ZERO `@inject`, ZERO `*_py.py` files.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::ctypes_rt::abi::{self, AbiRetKind};
use crate::builtin_modules::ctypes_rt::dynlib;
use crate::builtin_modules::ctypes_rt::types::{
    ByteOrder, CTypeInfo, CoreCtypesClasses, PTR_SIZE, PrimKind, WCHAR_SIZE, core_classes,
    lookup_ctype, register_ctype, set_core_classes,
};
use crate::builtin_modules::ctypes_rt::{
    CArgObjectState, CDataState, CDataStorage, CFieldState, FUNCFLAG_CDECL, FUNCFLAG_HRESULT,
    FUNCFLAG_PYTHONAPI, FUNCFLAG_STDCALL, FUNCFLAG_USE_ERRNO, FUNCFLAG_USE_LASTERROR, FuncPtrMeta,
    classify_restype, create_carg_instance, create_cdata_instance, ensure_ctype_info,
    ensure_struct_layout, extract_memory_address, extract_pointer_or_cdata_addr,
    finalize_struct_fields, make_array_type, make_pointer_type, marshal_ffi_arg, read_c_string,
    read_c_wstring, read_prim_value, read_ptr_usize, read_typed_at, read_wchar_array_value,
    u64_to_py_value, unmarshal_ffi_return, value_to_u64_wrapping, write_prim_value,
    write_ptr_usize, write_typed_at, write_wchar_array_value,
};
use crate::builtin_modules::macros::native_state::{
    set_native_state, with_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyStr, PyValue, PyValueRef};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, NativeIterFrame, class_is_subclass_of, value_type_name_str,
};
use crate::value::{PyClass, PyDict, PyModule, Value, ValueKind};

const BUF_WRITABLE: i64 = 0x1;

fn extract_cdata_class(val: &Value) -> Result<Rc<RefCell<PyClass>>> {
    let ValueKind::PyInstance(inst) = val.kind() else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "expected a ctypes instance, got '{}'",
                value_type_name_str(val)
            ),
        ));
    };
    Ok(Rc::clone(&inst.borrow().class))
}

fn ctype_mul_impl(interp: &mut Interpreter, cls_val: &Value, count_val: &Value) -> Result<Value> {
    let ValueKind::PyClass(cls) = cls_val.kind() else {
        return Ok(Value::not_implemented());
    };
    let n_idx = match interp.call_index_protocol(count_val, "array length") {
        Ok(v) => v,
        Err(_) => return Ok(Value::not_implemented()),
    };
    let n_i64 = n_idx
        .as_int()
        .ok_or_else(|| PyError::named("OverflowError", "array length too large"))?;
    if n_i64 < 0 {
        return Err(PyError::named(
            "ValueError",
            format!("Array length must be >= 0, not {n_i64}"),
        ));
    }
    let arr_cls = make_array_type(interp, cls, n_i64 as usize)?;
    Ok(Value::py_class(arr_cls))
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

fn cfuncptr_call_impl(interp: &mut Interpreter, self_val: &Value, args: &[Value]) -> Result<Value> {
    let cls = extract_cdata_class(self_val)?;
    let class_info = lookup_ctype(&cls);
    let (default_rt, default_at, default_flags) = match class_info {
        Some(CTypeInfo::FuncPtr {
            restype,
            argtypes,
            flags,
        }) => (restype, argtypes, flags),
        _ => (None, None, FUNCFLAG_CDECL),
    };

    let (addr, restype, argtypes, errcheck, flags, py_callable) =
        with_native_state::<CDataState, _>(self_val, "CFuncPtr", |st| {
            let addr = read_ptr_usize(&st.storage, 0, ByteOrder::Native)?;
            if let Some(ref meta) = st.func_ptr {
                Ok((
                    addr,
                    meta.restype.clone(),
                    meta.argtypes.clone(),
                    meta.errcheck.clone(),
                    meta.flags,
                    meta.callable.clone(),
                ))
            } else {
                Ok((addr, default_rt, default_at, None, default_flags, None))
            }
        })?;

    if let Some(ref expected) = argtypes
        && args.len() < expected.len()
    {
        return Err(PyError::named(
            "TypeError",
            format!(
                "this function takes at least {} arguments ({} given)",
                expected.len(),
                args.len()
            ),
        ));
    }

    // If this CFuncPtr wraps a Python callable directly, invoke it with C-coerced args/return.
    if let Some(callable) = py_callable {
        let mut py_args = Vec::with_capacity(args.len());
        let mut temps = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let coerced = if let Some(ref at_list) = argtypes
                && let Some(at) = at_list.get(i)
            {
                let abi_v = marshal_ffi_arg(interp, a, Some(at), &mut temps)?;
                crate::builtin_modules::ctypes_rt::abi_val_to_python(interp, at, abi_v)?
            } else {
                a.clone()
            };
            py_args.push(ExpandedCallArg {
                name: None,
                value: coerced,
            });
        }
        let raw_res = interp.call_function_expanded(callable, &py_args)?;
        let res = match restype {
            None => Value::none(),
            Some(ref rt) if rt.is_none() => Value::none(),
            Some(ref rt) => {
                let abi_ret =
                    crate::builtin_modules::ctypes_rt::python_to_abi_return(interp, rt, &raw_res)?;
                crate::builtin_modules::ctypes_rt::abi_val_to_python(interp, rt, abi_ret)?
            }
        };
        if let Some(ec) = errcheck {
            return interp.call_function_expanded(
                ec,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: res,
                    },
                    ExpandedCallArg {
                        name: None,
                        value: self_val.clone(),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: Value::tuple(args.to_vec()),
                    },
                ],
            );
        }
        return Ok(res);
    }

    // Marshal Python arguments into native C ABI values.
    let mut abi_args = Vec::with_capacity(args.len());
    let mut temps: Vec<Box<[u8]>> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        let at = argtypes.as_ref().and_then(|list| list.get(i));
        abi_args.push(marshal_ffi_arg(interp, a, at, &mut temps)?);
    }

    let ret_kind: AbiRetKind = classify_restype(interp, restype.as_ref())?;
    let use_errno = (flags & FUNCFLAG_USE_ERRNO) != 0;
    let use_last_error = (flags & FUNCFLAG_USE_LASTERROR) != 0;

    let raw_ret =
        abi::invoke_c_function(interp, addr, &abi_args, ret_kind, use_errno, use_last_error)?;
    drop(temps);

    let result = unmarshal_ffi_return(interp, restype.as_ref(), raw_ret)?;

    #[cfg(windows)]
    if (flags & FUNCFLAG_HRESULT) != 0
        && let ValueKind::Int(hr) = result.kind()
        && hr < 0
    {
        let msg = dynlib::format_error(Some(hr as i32));
        return Err(PyError::named("OSError", format!("[WinError {hr}] {msg}")));
    }

    if let Some(ec) = errcheck {
        return interp.call_function_expanded(
            ec,
            &[
                ExpandedCallArg {
                    name: None,
                    value: result,
                },
                ExpandedCallArg {
                    name: None,
                    value: self_val.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::tuple(args.to_vec()),
                },
            ],
        );
    }

    Ok(result)
}

pyrust_module! {
    constants {
        "RTLD_GLOBAL" => Value::int(dynlib::rtld_global()),
        "RTLD_LOCAL" => Value::int(dynlib::rtld_local()),
        "SIZEOF_TIME_T" => Value::int(std::mem::size_of::<i64>() as i64),
        "CTYPES_MAX_ARGCOUNT" => Value::int(1024),
        "FUNCFLAG_STDCALL" => Value::int(i64::from(FUNCFLAG_STDCALL)),
        "FUNCFLAG_CDECL" => Value::int(i64::from(FUNCFLAG_CDECL)),
        "FUNCFLAG_HRESULT" => Value::int(i64::from(FUNCFLAG_HRESULT)),
        "FUNCFLAG_PYTHONAPI" => Value::int(i64::from(FUNCFLAG_PYTHONAPI)),
        "FUNCFLAG_USE_ERRNO" => Value::int(i64::from(FUNCFLAG_USE_ERRNO)),
        "FUNCFLAG_USE_LASTERROR" => Value::int(i64::from(FUNCFLAG_USE_LASTERROR)),
        "_pointer_type_cache" => Value::dict(PyDict::default()),
        "_cast_addr" => Value::int(0x0001_0001),
        "_memmove_addr" => Value::int(0x0001_0002),
        "_memset_addr" => Value::int(0x0001_0003),
        "_string_at_addr" => Value::int(0x0001_0004),
        "_wstring_at_addr" => Value::int(0x0001_0005),
    }

    exception ArgumentError;
    #[cfg(windows)]
    exception COMError;

    // ── Metaclasses ──────────────────────────────────────────────────────────

    class _CDataMeta("type") {
        fn __mul__(
            #[positional_only] self_cls: PyValueRef<'_>,
            #[positional_only] other: PyValue,
        ) -> Result<Value> {
            ctype_mul_impl(_interp, self_cls.0, &other.0)
        }

        fn __rmul__(
            #[positional_only] self_cls: PyValueRef<'_>,
            #[positional_only] other: PyValue,
        ) -> Result<Value> {
            ctype_mul_impl(_interp, self_cls.0, &other.0)
        }
    }

    class PyCSimpleType("_CDataMeta") {
        fn __init__(
            #[positional_only] self_cls: PyValueRef<'_>,
            #[varargs] _args: &[Value],
            #[varkwargs] _kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            if let ValueKind::PyClass(cls) = self_cls.0.kind() {
                let code_val = cls.borrow().attrs.get("_type_").cloned();
                if let Some(cv) = code_val
                    && let ValueKind::Str(code) = cv.kind()
                    && let Some(kind) = PrimKind::from_code(code)
                {
                    register_ctype(
                        cls,
                        CTypeInfo::Simple {
                            kind,
                            byte_order: ByteOrder::Native,
                        },
                    );
                }
            }
            Ok(Value::none())
        }
    }

    class PyCArrayType("_CDataMeta") {}

    class PyCPointerType("_CDataMeta") {
        fn set_type(
            #[positional_only] self_cls: PyValueRef<'_>,
            #[positional_only] target: PyValue,
        ) -> Result<Value> {
            let ValueKind::PyClass(ptr_cls) = self_cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected a pointer type"));
            };
            let ValueKind::PyClass(t_cls) = target.0.kind() else {
                return Err(PyError::named("TypeError", "_type_ must be a ctypes type"));
            };
            let core = core_classes()?;
            if !class_is_subclass_of(t_cls, &core.cdata_base) {
                return Err(PyError::named("TypeError", "_type_ must be a ctypes type"));
            }
            ptr_cls
                .borrow_mut()
                .attrs
                .insert("_type_".to_string(), Value::py_class(Rc::clone(t_cls)));
            register_ctype(
                ptr_cls,
                CTypeInfo::Pointer {
                    target_cls: Some(Rc::clone(t_cls)),
                },
            );
            Ok(Value::none())
        }
    }

    class PyCStructType("_CDataMeta") {
        fn __init__(
            #[positional_only] self_cls: PyValueRef<'_>,
            #[varargs] _args: &[Value],
            #[varkwargs] _kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            if let ValueKind::PyClass(cls) = self_cls.0.kind() {
                let fields_val = cls.borrow().attrs.get("_fields_").cloned();
                if let Some(fv) = fields_val {
                    finalize_struct_fields(_interp, cls, &fv)?;
                }
            }
            Ok(Value::none())
        }

        #[getter("_fields_")]
        fn get_fields(#[positional_only] self_cls: PyValueRef<'_>) -> Result<Value> {
            let ValueKind::PyClass(cls) = self_cls.0.kind() else {
                return Err(PyError::named("AttributeError", "_fields_"));
            };
            if let Some(v) = cls.borrow().attrs.get("_fields_").cloned() {
                return Ok(v);
            }
            Err(PyError::named(
                "AttributeError",
                format!("type object '{}' has no attribute '_fields_'", cls.borrow().name),
            ))
        }

        #[setter("_fields_")]
        fn set_fields(
            #[positional_only] self_cls: PyValueRef<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let ValueKind::PyClass(cls) = self_cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected a Structure type"));
            };
            finalize_struct_fields(_interp, cls, &value.0)?;
            Ok(Value::none())
        }
    }

    class UnionType("_CDataMeta") {
        fn __init__(
            #[positional_only] self_cls: PyValueRef<'_>,
            #[varargs] _args: &[Value],
            #[varkwargs] _kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            if let ValueKind::PyClass(cls) = self_cls.0.kind() {
                let fields_val = cls.borrow().attrs.get("_fields_").cloned();
                if let Some(fv) = fields_val {
                    finalize_struct_fields(_interp, cls, &fv)?;
                }
            }
            Ok(Value::none())
        }

        #[getter("_fields_")]
        fn get_fields(#[positional_only] self_cls: PyValueRef<'_>) -> Result<Value> {
            let ValueKind::PyClass(cls) = self_cls.0.kind() else {
                return Err(PyError::named("AttributeError", "_fields_"));
            };
            if let Some(v) = cls.borrow().attrs.get("_fields_").cloned() {
                return Ok(v);
            }
            Err(PyError::named(
                "AttributeError",
                format!("type object '{}' has no attribute '_fields_'", cls.borrow().name),
            ))
        }

        #[setter("_fields_")]
        fn set_fields(
            #[positional_only] self_cls: PyValueRef<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let ValueKind::PyClass(cls) = self_cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected a Union type"));
            };
            finalize_struct_fields(_interp, cls, &value.0)?;
            Ok(Value::none())
        }
    }

    class PyCFuncPtrType("_CDataMeta") {}

    // ── Helper Descriptor / Parameter Classes ────────────────────────────────

    #[native_state(CArgObjectState)]
    class CArgObject {
        #[getter]
        fn _obj(#[state] st: &CArgObjectState) -> Result<Value> {
            Ok(st.obj.clone())
        }

        fn __repr__(#[state] st: &CArgObjectState) -> Result<Value> {
            Ok(Value::string(format!(
                "<cparam '{}' ({:#x})>",
                st.tag, st.addr
            )))
        }
    }

    #[native_state(CFieldState)]
    class CField {
        #[getter]
        fn offset(#[state] st: &CFieldState) -> Result<Value> {
            Ok(Value::int(st.field.offset as i64))
        }

        #[getter]
        fn size(#[state] st: &CFieldState) -> Result<Value> {
            Ok(Value::int(st.field.py_size_attr()))
        }

        fn __get__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] obj: PyValue,
            #[positional_only]
            #[default(None)]
            _owner: Option<PyValue>,
        ) -> Result<Value> {
            if obj.0.is_none() {
                return Ok(self_val.0.clone());
            }
            let (field, owner_swapped) =
                with_native_state::<CFieldState, _>(self_val.0, "CField", |st| {
                    Ok((st.field.clone(), st.owner_swapped))
                })?;
            let storage = with_native_state::<CDataState, _>(&obj.0, "_CData", |st| {
                Ok(st.storage.clone())
            })?;
            read_typed_at(
                _interp,
                &storage,
                Some(obj.0.clone()),
                field.offset,
                &field.field_cls,
                Some(&field),
                owner_swapped,
            )
        }

        fn __set__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] obj: PyValue,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let (field, owner_swapped) =
                with_native_state::<CFieldState, _>(self_val.0, "CField", |st| {
                    Ok((st.field.clone(), st.owner_swapped))
                })?;
            with_native_state_mut::<CDataState, _>(&obj.0, "_CData", |st| {
                let keep_key = format!("field:{}:{}", field.name, field.offset);
                write_typed_at(
                    _interp,
                    st,
                    field.offset,
                    &field.field_cls,
                    &value.0,
                    Some(&field),
                    owner_swapped,
                    &keep_key,
                )
            })?;
            Ok(Value::none())
        }

        fn __repr__(#[state] st: &CFieldState) -> Result<Value> {
            if let (Some(bo), Some(bw)) = (st.field.bit_offset, st.field.bit_width) {
                Ok(Value::string(format!(
                    "<Field type={}, ofs={}:{bo}, bits={bw}>",
                    st.field.field_cls.borrow().name,
                    st.field.offset
                )))
            } else {
                Ok(Value::string(format!(
                    "<Field type={}, ofs={}, size={}>",
                    st.field.field_cls.borrow().name,
                    st.field.offset,
                    st.field.size
                )))
            }
        }
    }

    // ── Base `_CData` Class ──────────────────────────────────────────────────

    #[native_state(CDataState)]
    class _CData {
        #[classmethod]
        fn from_buffer(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] source: PyValue,
            #[positional_only]
            #[default(0_i64.into())]
            offset: PyIndex,
        ) -> Result<Value> {
            let ValueKind::PyClass(cls_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected a ctypes type"));
            };
            let info = ensure_ctype_info(_interp, cls_rc)?;
            let needed = info.size();
            let off_i64 = offset.expect_i64(FN_NAME, "offset")?;
            if off_i64 < 0 {
                return Err(PyError::named("ValueError", "offset cannot be negative"));
            }
            let off = off_i64 as usize;

            // Writable buffer sharing for `bytearray`
            if let Some((buf_rc, _)) = pyrust_builtins::bytearray::as_bytearray_buffer(&source.0) {
                let len = buf_rc.borrow().len();
                if off.checked_add(needed).is_none_or(|end| end > len) {
                    return Err(PyError::named(
                        "ValueError",
                        format!(
                            "Buffer size too small ({len} instead of at least {} bytes)",
                            off.saturating_add(needed)
                        ),
                    ));
                }
                let storage =
                    CDataStorage::from_shared_vec(buf_rc, off, needed, Some(source.0.clone()));
                return create_cdata_instance(Rc::clone(cls_rc), storage);
            }

            // Writable buffer sharing for another `_CData` object
            if let Ok(src_storage) =
                with_native_state::<CDataState, _>(&source.0, "_CData", |st| Ok(st.storage.clone()))
            {
                let len = src_storage.size();
                if off.checked_add(needed).is_none_or(|end| end > len) {
                    return Err(PyError::named(
                        "ValueError",
                        format!(
                            "Buffer size too small ({len} instead of at least {} bytes)",
                            off.saturating_add(needed)
                        ),
                    ));
                }
                let sub = src_storage.subview(off, needed, Some(source.0.clone()));
                return create_cdata_instance(Rc::clone(cls_rc), sub);
            }

            // Reject immutable `bytes` matching CPython (`TypeError: underlying buffer is not writable`)
            if matches!(source.0.kind(), ValueKind::Bytes(_)) {
                return Err(PyError::named(
                    "TypeError",
                    "underlying buffer is not writable",
                ));
            }

            // General writable `memoryview` / PEP 688 `__buffer__` exporter
            let mv = crate::interpreter::acquire_buffer_view(_interp, &source.0, BUF_WRITABLE)
                .map_err(|_| PyError::named("TypeError", "underlying buffer is not writable"))?;
            let bytes = pyrust_builtins::memoryview::to_bytes_checked(&mv)?;
            crate::interpreter::release_memoryview_with_interp(_interp, &mv)?;
            if off.checked_add(needed).is_none_or(|end| end > bytes.len()) {
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "Buffer size too small ({} instead of at least {} bytes)",
                        bytes.len(),
                        off.saturating_add(needed)
                    ),
                ));
            }
            let storage = CDataStorage::allocate_zeroed(needed);
            storage.write_bytes(0, &bytes[off..off + needed])?;
            create_cdata_instance(Rc::clone(cls_rc), storage)
        }

        #[classmethod]
        fn from_buffer_copy(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] source: PyValue,
            #[positional_only]
            #[default(0_i64.into())]
            offset: PyIndex,
        ) -> Result<Value> {
            let ValueKind::PyClass(cls_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected a ctypes type"));
            };
            let info = ensure_ctype_info(_interp, cls_rc)?;
            let needed = info.size();
            let off_i64 = offset.expect_i64(FN_NAME, "offset")?;
            if off_i64 < 0 {
                return Err(PyError::named("ValueError", "offset cannot be negative"));
            }
            let off = off_i64 as usize;

            let bytes = crate::interpreter::extract_buffer_bytes(_interp, &source.0)?;
            if off.checked_add(needed).is_none_or(|end| end > bytes.len()) {
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "Buffer size too small ({} instead of at least {} bytes)",
                        bytes.len(),
                        off.saturating_add(needed)
                    ),
                ));
            }
            let storage = CDataStorage::allocate_zeroed(needed);
            storage.write_bytes(0, &bytes[off..off + needed])?;
            create_cdata_instance(Rc::clone(cls_rc), storage)
        }

        #[classmethod]
        fn from_address(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] address: PyIndex,
        ) -> Result<Value> {
            let ValueKind::PyClass(cls_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected a ctypes type"));
            };
            let info = ensure_ctype_info(_interp, cls_rc)?;
            let addr = address.expect_i64(FN_NAME, "address")? as usize;
            let storage = CDataStorage::from_raw(addr as *mut u8, info.size(), None);
            create_cdata_instance(Rc::clone(cls_rc), storage)
        }

        #[classmethod]
        fn in_dll(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] dll: PyValue,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            let ValueKind::PyClass(cls_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected a ctypes type"));
            };
            let info = ensure_ctype_info(_interp, cls_rc)?;
            let handle_val = _interp.get_attr(&dll.0, "_handle")?;
            let handle = value_to_u64_wrapping(_interp, &handle_val)? as usize;
            let sym_addr = dynlib::lookup_symbol(handle, &name.0).map_err(|_| {
                PyError::named("ValueError", format!("symbol '{}' not found", name.0))
            })?;
            let storage =
                CDataStorage::from_raw(sym_addr as *mut u8, info.size(), Some(dll.0.clone()));
            create_cdata_instance(Rc::clone(cls_rc), storage)
        }

        #[classmethod]
        fn from_param(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let ValueKind::PyClass(cls_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected a ctypes type"));
            };
            let is_instance = matches!(
                value.0.kind(),
                ValueKind::PyInstance(inst) if class_is_subclass_of(&inst.borrow().class, cls_rc)
            );
            if is_instance {
                return Ok(value.0);
            }
            _interp.call_function_expanded(
                cls.0.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: value.0,
                }],
            )
        }

        fn __buffer__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] flags: PyIndex,
        ) -> Result<Value> {
            let fl = flags.expect_i64(FN_NAME, "flags")?;
            let writable = (fl & BUF_WRITABLE) != 0;
            with_native_state::<CDataState, _>(self_val.0, "_CData", |st| {
                st.storage.export_memoryview(self_val.0.clone(), writable)
            })
        }

        fn __ctypes_from_outparam__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        #[getter]
        fn _b_needsfree_(#[state] st: &CDataState) -> Result<Value> {
            let owns = matches!(st.storage, CDataStorage::Managed { offset: 0, .. });
            Ok(Value::int(i64::from(owns)))
        }
    }

    // ── `_SimpleCData` ───────────────────────────────────────────────────────

    class _SimpleCData("_CData") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("expected at most 1 argument, got {}", args.len()),
                ));
            }
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Simple { kind, byte_order } = info else {
                return Err(PyError::named("TypeError", "invalid _SimpleCData subtype"));
            };
            let mut state = CDataState::new(CDataStorage::allocate_zeroed(kind.size()));
            if let Some(init_val) = args.first() {
                write_prim_value(_interp, &mut state, 0, kind, byte_order, init_val, "value")?;
            }
            set_native_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[getter]
        fn value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Simple { kind, byte_order } = info else {
                return Err(PyError::named("TypeError", "invalid _SimpleCData subtype"));
            };
            with_native_state::<CDataState, _>(self_val.0, "_SimpleCData", |st| {
                read_prim_value(&st.storage, 0, kind, byte_order)
            })
        }

        #[setter("value")]
        fn set_value(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Simple { kind, byte_order } = info else {
                return Err(PyError::named("TypeError", "invalid _SimpleCData subtype"));
            };
            with_native_state_mut::<CDataState, _>(self_val.0, "_SimpleCData", |st| {
                write_prim_value(_interp, st, 0, kind, byte_order, &val.0, "value")
            })?;
            Ok(Value::none())
        }

        fn __bool__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Simple { kind, byte_order } = info else {
                return Ok(Value::bool_(false));
            };
            if kind == PrimKind::PyObject {
                let is_non_null = with_native_state::<CDataState, _>(self_val.0, "_SimpleCData", |st| {
                    Ok(read_ptr_usize(&st.storage, 0, byte_order)? != 0)
                })?;
                return Ok(Value::bool_(is_non_null));
            }
            let val = with_native_state::<CDataState, _>(self_val.0, "_SimpleCData", |st| {
                read_prim_value(&st.storage, 0, kind, byte_order)
            })?;
            match val.kind() {
                ValueKind::None => Ok(Value::bool_(false)),
                ValueKind::Bool(b) => Ok(Value::bool_(b)),
                ValueKind::Int(n) => Ok(Value::bool_(n != 0)),
                ValueKind::Float(f) => Ok(Value::bool_(f != 0.0)),
                ValueKind::Bytes(b) => Ok(Value::bool_(b.iter().any(|&x| x != 0))),
                ValueKind::Str(s) => Ok(Value::bool_(s.chars().any(|c| c != '\0'))),
                _ => Ok(Value::bool_(_interp.truthy_value(&val)?)),
            }
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let cls_name = cls.borrow().name.clone();
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Simple { kind, byte_order } = info else {
                return Ok(Value::string(format!("{cls_name}()")));
            };
            if kind == PrimKind::PyObject {
                let id = with_native_state::<CDataState, _>(self_val.0, "_SimpleCData", |st| {
                    read_ptr_usize(&st.storage, 0, byte_order)
                })?;
                if id == 0 {
                    return Ok(Value::string(format!("{cls_name}(<NULL>)")));
                }
            }
            let val = with_native_state::<CDataState, _>(self_val.0, "_SimpleCData", |st| {
                read_prim_value(&st.storage, 0, kind, byte_order)
            })?;
            let val_repr = _interp.call_function_expanded(
                Value::builtin_function("repr"),
                &[ExpandedCallArg {
                    name: None,
                    value: val,
                }],
            )?;
            let repr_str = match val_repr.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => val_repr.repr_raw(),
            };
            Ok(Value::string(format!("{cls_name}({repr_str})")))
        }

        fn __ctypes_from_outparam__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "value")
        }
    }

    // ── `Array` ──────────────────────────────────────────────────────────────

    class Array("_CData") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array {
                elem_cls,
                length,
                elem_size,
                ..
            } = info
            else {
                return Err(PyError::named("TypeError", "abstract class"));
            };
            if args.len() > length {
                return Err(PyError::named("IndexError", "too many initializers"));
            }
            let mut state = CDataState::new(CDataStorage::allocate_zeroed(length * elem_size));
            for (i, item) in args.iter().enumerate() {
                let key = format!("[{i}]");
                write_typed_at(
                    _interp,
                    &mut state,
                    i * elem_size,
                    &elem_cls,
                    item,
                    None,
                    false,
                    &key,
                )?;
            }
            set_native_state(self_val.0, state)?;
            Ok(Value::none())
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array { length, .. } = info else {
                return Err(PyError::named("TypeError", "invalid Array type"));
            };
            Ok(Value::int(length as i64))
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array {
                elem_cls,
                length,
                elem_size,
                ..
            } = info
            else {
                return Err(PyError::named("TypeError", "invalid Array type"));
            };
            let storage = with_native_state::<CDataState, _>(self_val.0, "Array", |st| {
                Ok(st.storage.clone())
            })?;

            if let Some((lo, hi, stp)) = pyrust_builtins::slice::slice_fields(&key.0) {
                let (start, stop, step) =
                    pyrust_builtins::slice::compute_indices(length as i64, &lo, &hi, &stp)?;
                let indices = collect_slice_indices(start, stop, step);
                // CPython returns `bytes` when slicing `c_char * n` and `str` for `c_wchar * n`!
                if let Some(CTypeInfo::Simple {
                    kind: PrimKind::Char,
                    ..
                }) = lookup_ctype(&elem_cls)
                {
                    let mut out = Vec::with_capacity(indices.len());
                    let mut b = [0u8; 1];
                    for idx in indices {
                        storage.read_bytes(idx, &mut b)?;
                        out.push(b[0]);
                    }
                    return Ok(Value::bytes(out));
                }
                if let Some(CTypeInfo::Simple {
                    kind: PrimKind::WChar,
                    byte_order,
                }) = lookup_ctype(&elem_cls)
                {
                    let mut out = String::with_capacity(indices.len());
                    for idx in indices {
                        let ch_val =
                            read_prim_value(&storage, idx * WCHAR_SIZE, PrimKind::WChar, byte_order)?;
                        if let ValueKind::Str(s) = ch_val.kind() {
                            out.push_str(s);
                        }
                    }
                    return Ok(Value::string(out));
                }
                let mut items = Vec::with_capacity(indices.len());
                for idx in indices {
                    items.push(read_typed_at(
                        _interp,
                        &storage,
                        Some(self_val.0.clone()),
                        idx * elem_size,
                        &elem_cls,
                        None,
                        false,
                    )?);
                }
                return Ok(Value::list(items));
            }

            let raw_idx = _interp.call_index_protocol(&key.0, "array indices")?;
            let idx_i64 = raw_idx
                .as_int()
                .ok_or_else(|| PyError::named("IndexError", "invalid index"))?;
            let norm = if idx_i64 < 0 {
                idx_i64 + length as i64
            } else {
                idx_i64
            };
            if norm < 0 || norm >= length as i64 {
                return Err(PyError::named("IndexError", "invalid index"));
            }
            read_typed_at(
                _interp,
                &storage,
                Some(self_val.0.clone()),
                (norm as usize) * elem_size,
                &elem_cls,
                None,
                false,
            )
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array {
                elem_cls,
                length,
                elem_size,
                ..
            } = info
            else {
                return Err(PyError::named("TypeError", "invalid Array type"));
            };

            if let Some((lo, hi, stp)) = pyrust_builtins::slice::slice_fields(&key.0) {
                let (start, stop, step) =
                    pyrust_builtins::slice::compute_indices(length as i64, &lo, &hi, &stp)?;
                let indices = collect_slice_indices(start, stop, step);

                // Special fast paths for bytes -> c_char*n and str -> c_wchar*n
                if let ValueKind::Bytes(b) = value.0.kind()
                    && matches!(
                        lookup_ctype(&elem_cls),
                        Some(CTypeInfo::Simple {
                            kind: PrimKind::Char,
                            ..
                        })
                    )
                {
                    if b.len() != indices.len() {
                        return Err(PyError::named(
                            "ValueError",
                            "Can only assign sequence of same size",
                        ));
                    }
                    with_native_state_mut::<CDataState, _>(self_val.0, "Array", |st| {
                        for (&idx, &byte) in indices.iter().zip(b.iter()) {
                            st.storage.write_bytes(idx, &[byte])?;
                        }
                        Ok(())
                    })?;
                    return Ok(Value::none());
                }
                if let ValueKind::Str(s) = value.0.kind()
                    && matches!(
                        lookup_ctype(&elem_cls),
                        Some(CTypeInfo::Simple {
                            kind: PrimKind::WChar,
                            ..
                        })
                    )
                {
                    let chars: Vec<String> = s.chars().map(|c| c.to_string()).collect();
                    if chars.len() != indices.len() {
                        return Err(PyError::named(
                            "ValueError",
                            "Can only assign sequence of same size",
                        ));
                    }
                    with_native_state_mut::<CDataState, _>(self_val.0, "Array", |st| {
                        for (&idx, ch) in indices.iter().zip(chars.iter()) {
                            let k = format!("[{idx}]");
                            write_typed_at(
                                _interp,
                                st,
                                idx * elem_size,
                                &elem_cls,
                                &Value::string(ch.clone()),
                                None,
                                false,
                                &k,
                            )?;
                        }
                        Ok(())
                    })?;
                    return Ok(Value::none());
                }

                let rhs = _interp.collect_iterable(&value.0)?;
                if rhs.len() != indices.len() {
                    return Err(PyError::named(
                        "ValueError",
                        "Can only assign sequence of same size",
                    ));
                }
                with_native_state_mut::<CDataState, _>(self_val.0, "Array", |st| {
                    for (&idx, item) in indices.iter().zip(rhs.iter()) {
                        let k = format!("[{idx}]");
                        write_typed_at(
                            _interp,
                            st,
                            idx * elem_size,
                            &elem_cls,
                            item,
                            None,
                            false,
                            &k,
                        )?;
                    }
                    Ok(())
                })?;
                return Ok(Value::none());
            }

            let raw_idx = _interp.call_index_protocol(&key.0, "array indices")?;
            let idx_i64 = raw_idx
                .as_int()
                .ok_or_else(|| PyError::named("IndexError", "invalid index"))?;
            let norm = if idx_i64 < 0 {
                idx_i64 + length as i64
            } else {
                idx_i64
            };
            if norm < 0 || norm >= length as i64 {
                return Err(PyError::named("IndexError", "invalid index"));
            }
            let u = norm as usize;
            with_native_state_mut::<CDataState, _>(self_val.0, "Array", |st| {
                let k = format!("[{u}]");
                write_typed_at(
                    _interp,
                    st,
                    u * elem_size,
                    &elem_cls,
                    &value.0,
                    None,
                    false,
                    &k,
                )
            })?;
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array {
                elem_cls,
                length,
                elem_size,
                ..
            } = info
            else {
                return Err(PyError::named("TypeError", "invalid Array type"));
            };
            let storage = with_native_state::<CDataState, _>(self_val.0, "Array", |st| {
                Ok(st.storage.clone())
            })?;
            let mut items = Vec::with_capacity(length);
            for i in 0..length {
                items.push(read_typed_at(
                    _interp,
                    &storage,
                    Some(self_val.0.clone()),
                    i * elem_size,
                    &elem_cls,
                    None,
                    false,
                )?);
            }
            Ok(Value::generator(Box::new(NativeIterFrame::generator(items))))
        }

        #[getter]
        fn raw(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array {
                elem_cls, length, ..
            } = info
            else {
                return Err(PyError::named("AttributeError", "raw"));
            };
            if !matches!(
                lookup_ctype(&elem_cls),
                Some(CTypeInfo::Simple {
                    kind: PrimKind::Char,
                    ..
                })
            ) {
                return Err(PyError::named(
                    "AttributeError",
                    format!("'{}' object has no attribute 'raw'", cls.borrow().name),
                ));
            }
            with_native_state::<CDataState, _>(self_val.0, "Array", |st| {
                let mut buf = vec![0u8; length];
                st.storage.read_bytes(0, &mut buf)?;
                Ok(Value::bytes(buf))
            })
        }

        #[setter("raw")]
        fn set_raw(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array {
                elem_cls, length, ..
            } = info
            else {
                return Err(PyError::named("AttributeError", "raw"));
            };
            if !matches!(
                lookup_ctype(&elem_cls),
                Some(CTypeInfo::Simple {
                    kind: PrimKind::Char,
                    ..
                })
            ) {
                return Err(PyError::named(
                    "AttributeError",
                    format!("'{}' object has no attribute 'raw'", cls.borrow().name),
                ));
            }
            let src = crate::interpreter::extract_buffer_bytes(_interp, &val.0)?;
            if src.len() > length {
                return Err(PyError::named("ValueError", "byte string too long"));
            }
            with_native_state_mut::<CDataState, _>(self_val.0, "Array", |st| {
                st.storage.write_bytes(0, &src)
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array {
                elem_cls, length, ..
            } = info
            else {
                return Err(PyError::named("AttributeError", "value"));
            };
            match lookup_ctype(&elem_cls) {
                Some(CTypeInfo::Simple {
                    kind: PrimKind::Char,
                    ..
                }) => with_native_state::<CDataState, _>(self_val.0, "Array", |st| {
                    let mut buf = vec![0u8; length];
                    st.storage.read_bytes(0, &mut buf)?;
                    let nul = buf.iter().position(|&b| b == 0).unwrap_or(length);
                    Ok(Value::bytes(buf[..nul].to_vec()))
                }),
                Some(CTypeInfo::Simple {
                    kind: PrimKind::WChar,
                    byte_order,
                }) => with_native_state::<CDataState, _>(self_val.0, "Array", |st| {
                    Ok(Value::string(read_wchar_array_value(
                        &st.storage,
                        length,
                        byte_order,
                    )?))
                }),
                _ => Err(PyError::named(
                    "AttributeError",
                    format!("'{}' object has no attribute 'value'", cls.borrow().name),
                )),
            }
        }

        #[setter("value")]
        fn set_value(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Array {
                elem_cls, length, ..
            } = info
            else {
                return Err(PyError::named("AttributeError", "value"));
            };
            match lookup_ctype(&elem_cls) {
                Some(CTypeInfo::Simple {
                    kind: PrimKind::Char,
                    ..
                }) => {
                    let ValueKind::Bytes(b) = val.0.kind() else {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "expected bytes, {} found",
                                value_type_name_str(&val.0)
                            ),
                        ));
                    };
                    if b.len() > length {
                        return Err(PyError::named("ValueError", "byte string too long"));
                    }
                    with_native_state_mut::<CDataState, _>(self_val.0, "Array", |st| {
                        st.storage.write_bytes(0, b)?;
                        if b.len() < length {
                            st.storage.write_bytes(b.len(), &[0u8])?;
                        }
                        Ok(())
                    })?;
                    Ok(Value::none())
                }
                Some(CTypeInfo::Simple {
                    kind: PrimKind::WChar,
                    byte_order,
                }) => {
                    let ValueKind::Str(s) = val.0.kind() else {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "unicode string expected instead of {} instance",
                                value_type_name_str(&val.0)
                            ),
                        ));
                    };
                    with_native_state_mut::<CDataState, _>(self_val.0, "Array", |st| {
                        write_wchar_array_value(&st.storage, 0, length, s, byte_order)
                    })?;
                    Ok(Value::none())
                }
                _ => Err(PyError::named(
                    "AttributeError",
                    format!("'{}' object has no attribute 'value'", cls.borrow().name),
                )),
            }
        }
    }

    // ── `_Pointer` ───────────────────────────────────────────────────────────

    class _Pointer("_CData") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("expected at most 1 argument, got {}", args.len()),
                ));
            }
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Pointer { target_cls } = info else {
                return Err(PyError::named("TypeError", "invalid _Pointer subtype"));
            };
            let mut state = CDataState::new(CDataStorage::allocate_zeroed(PTR_SIZE));
            if let Some(arg) = args.first()
                && !arg.is_none()
            {
                let Some(ref tcls) = target_cls else {
                    return Err(PyError::named(
                        "TypeError",
                        "cannot create pointer to incomplete type",
                    ));
                };
                if let ValueKind::PyInstance(inst) = arg.kind()
                    && class_is_subclass_of(&inst.borrow().class, tcls)
                {
                    let addr = with_native_state::<CDataState, _>(arg, "_CData", |st| {
                        Ok(st.storage.address())
                    })?;
                    write_ptr_usize(&state.storage, 0, addr, ByteOrder::Native)?;
                    state.keep_alive_vals.insert("target".to_string(), arg.clone());
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "expected {} instead of {}",
                            tcls.borrow().name,
                            value_type_name_str(arg)
                        ),
                    ));
                }
            }
            set_native_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[getter]
        fn contents(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Pointer {
                target_cls: Some(tcls),
            } = info
            else {
                return Err(PyError::named("ValueError", "NULL pointer access"));
            };
            let t_info = ensure_ctype_info(_interp, &tcls)?;
            let (addr, keep) = with_native_state::<CDataState, _>(self_val.0, "_Pointer", |st| {
                let a = read_ptr_usize(&st.storage, 0, ByteOrder::Native)?;
                Ok((a, st.keep_alive_vals.get("target").cloned()))
            })?;
            if addr == 0 {
                return Err(PyError::named("ValueError", "NULL pointer access"));
            }
            let raw = CDataStorage::from_raw(
                addr as *mut u8,
                t_info.size(),
                keep.or_else(|| Some(self_val.0.clone())),
            );
            create_cdata_instance(tcls, raw)
        }

        #[setter("contents")]
        fn set_contents(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Pointer {
                target_cls: Some(tcls),
            } = info
            else {
                return Err(PyError::named("TypeError", "incomplete pointer type"));
            };
            let ValueKind::PyInstance(inst) = val.0.kind() else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "expected {} instead of {}",
                        tcls.borrow().name,
                        value_type_name_str(&val.0)
                    ),
                ));
            };
            if !class_is_subclass_of(&inst.borrow().class, &tcls) {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "expected {} instead of {}",
                        tcls.borrow().name,
                        value_type_name_str(&val.0)
                    ),
                ));
            }
            let addr = with_native_state::<CDataState, _>(&val.0, "_CData", |st| {
                Ok(st.storage.address())
            })?;
            with_native_state_mut::<CDataState, _>(self_val.0, "_Pointer", |st| {
                write_ptr_usize(&st.storage, 0, addr, ByteOrder::Native)?;
                st.keep_alive_vals.insert("target".to_string(), val.0.clone());
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Pointer {
                target_cls: Some(tcls),
            } = info
            else {
                return Err(PyError::named("ValueError", "NULL pointer access"));
            };
            let t_info = ensure_ctype_info(_interp, &tcls)?;
            let t_size = t_info.size();
            let base_addr = with_native_state::<CDataState, _>(self_val.0, "_Pointer", |st| {
                read_ptr_usize(&st.storage, 0, ByteOrder::Native)
            })?;
            if base_addr == 0 {
                return Err(PyError::named("ValueError", "NULL pointer access"));
            }

            if let Some((lo, hi, stp)) = pyrust_builtins::slice::slice_fields(&key.0) {
                let start = match lo.kind() {
                    ValueKind::None => 0i64,
                    ValueKind::Int(n) => n,
                    _ => _interp
                        .call_index_protocol(&lo, "slice start")?
                        .as_int()
                        .ok_or_else(|| PyError::named("ValueError", "invalid slice start"))?,
                };
                let stop = match hi.kind() {
                    ValueKind::None => {
                        return Err(PyError::named("ValueError", "slice stop is required"));
                    }
                    ValueKind::Int(n) => n,
                    _ => _interp
                        .call_index_protocol(&hi, "slice stop")?
                        .as_int()
                        .ok_or_else(|| PyError::named("ValueError", "invalid slice stop"))?,
                };
                let step = match stp.kind() {
                    ValueKind::None => 1i64,
                    ValueKind::Int(n) => n,
                    _ => _interp
                        .call_index_protocol(&stp, "slice step")?
                        .as_int()
                        .ok_or_else(|| PyError::named("ValueError", "invalid slice step"))?,
                };
                if step == 0 {
                    return Err(PyError::named("ValueError", "slice step cannot be zero"));
                }
                let mut cur = start;
                let mut idxs = Vec::new();
                if step > 0 {
                    while cur < stop {
                        idxs.push(cur);
                        cur += step;
                    }
                } else {
                    while cur > stop {
                        idxs.push(cur);
                        cur += step;
                    }
                }
                if let CTypeInfo::Simple {
                    kind: PrimKind::Char,
                    ..
                } = t_info
                {
                    let mut out = Vec::with_capacity(idxs.len());
                    for i in idxs {
                        let ptr = (base_addr as isize).wrapping_add(i as isize) as *const u8;
                        // SAFETY: caller explicitly indexed valid pointer range.
                        out.push(unsafe { *ptr });
                    }
                    return Ok(Value::bytes(out));
                }
                let mut items = Vec::with_capacity(idxs.len());
                for i in idxs {
                    let elem_addr = (base_addr as isize)
                        .wrapping_add((i as isize).wrapping_mul(t_size as isize))
                        as *mut u8;
                    let raw = CDataStorage::from_raw(elem_addr, t_size, Some(self_val.0.clone()));
                    items.push(read_typed_at(
                        _interp,
                        &raw,
                        Some(self_val.0.clone()),
                        0,
                        &tcls,
                        None,
                        false,
                    )?);
                }
                return Ok(Value::list(items));
            }

            let idx = _interp
                .call_index_protocol(&key.0, "pointer indices")?
                .as_int()
                .ok_or_else(|| PyError::named("IndexError", "pointer index out of range"))?;
            let elem_addr = (base_addr as isize)
                .wrapping_add((idx as isize).wrapping_mul(t_size as isize))
                as *mut u8;
            let raw = CDataStorage::from_raw(elem_addr, t_size, Some(self_val.0.clone()));
            read_typed_at(
                _interp,
                &raw,
                Some(self_val.0.clone()),
                0,
                &tcls,
                None,
                false,
            )
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] value: PyValue,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let info = ensure_ctype_info(_interp, &cls)?;
            let CTypeInfo::Pointer {
                target_cls: Some(tcls),
            } = info
            else {
                return Err(PyError::named("ValueError", "NULL pointer access"));
            };
            let t_info = ensure_ctype_info(_interp, &tcls)?;
            let t_size = t_info.size();
            let base_addr = with_native_state::<CDataState, _>(self_val.0, "_Pointer", |st| {
                read_ptr_usize(&st.storage, 0, ByteOrder::Native)
            })?;
            if base_addr == 0 {
                return Err(PyError::named("ValueError", "NULL pointer access"));
            }
            let idx = _interp
                .call_index_protocol(&key.0, "pointer indices")?
                .as_int()
                .ok_or_else(|| PyError::named("IndexError", "pointer index out of range"))?;
            let elem_addr = (base_addr as isize)
                .wrapping_add((idx as isize).wrapping_mul(t_size as isize))
                as *mut u8;
            let mut temp_state =
                CDataState::new(CDataStorage::from_raw(elem_addr, t_size, None));
            let k = format!("ptr[{idx}]");
            write_typed_at(_interp, &mut temp_state, 0, &tcls, &value.0, None, false, &k)?;
            // Preserve any keep-alives created during assignment onto the pointer instance.
            if !temp_state.keep_alive_bufs.is_empty() || !temp_state.keep_alive_vals.is_empty() {
                with_native_state_mut::<CDataState, _>(self_val.0, "_Pointer", |st| {
                    st.keep_alive_bufs.extend(temp_state.keep_alive_bufs);
                    st.keep_alive_vals.extend(temp_state.keep_alive_vals);
                    Ok(())
                })?;
            }
            Ok(Value::none())
        }

        fn __bool__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let addr = with_native_state::<CDataState, _>(self_val.0, "_Pointer", |st| {
                read_ptr_usize(&st.storage, 0, ByteOrder::Native)
            })?;
            Ok(Value::bool_(addr != 0))
        }
    }

    // ── `Structure` & `Union` ────────────────────────────────────────────────

    class Structure("_CData") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let layout = ensure_struct_layout(_interp, &cls)?;
            let state = CDataState::new(CDataStorage::allocate_zeroed(layout.size));
            set_native_state(self_val.0, state)?;

            if args.len() > layout.fields.len() {
                return Err(PyError::named("TypeError", "too many initializers"));
            }
            let mut assigned: Vec<String> = Vec::with_capacity(args.len());
            for (i, val) in args.iter().enumerate() {
                let f = &layout.fields[i];
                assigned.push(f.name.clone());
                with_native_state_mut::<CDataState, _>(self_val.0, "Structure", |st| {
                    let k = format!("field:{}:{}", f.name, f.offset);
                    write_typed_at(
                        _interp,
                        st,
                        f.offset,
                        &f.field_cls,
                        val,
                        Some(f),
                        layout.swapped_bytes,
                        &k,
                    )
                })?;
            }
            for (k, v) in kwargs {
                if assigned.contains(&k) {
                    return Err(PyError::named(
                        "TypeError",
                        format!("duplicate values for field {k:?}"),
                    ));
                }
                _interp.assign_attr(self_val.0.clone(), &k, v)?;
            }
            Ok(Value::none())
        }
    }

    class Union("_CData") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let layout = ensure_struct_layout(_interp, &cls)?;
            let state = CDataState::new(CDataStorage::allocate_zeroed(layout.size));
            set_native_state(self_val.0, state)?;

            if args.len() > layout.fields.len() {
                return Err(PyError::named("TypeError", "too many initializers"));
            }
            let mut assigned: Vec<String> = Vec::with_capacity(args.len());
            for (i, val) in args.iter().enumerate() {
                let f = &layout.fields[i];
                assigned.push(f.name.clone());
                with_native_state_mut::<CDataState, _>(self_val.0, "Union", |st| {
                    let k = format!("field:{}:{}", f.name, f.offset);
                    write_typed_at(
                        _interp,
                        st,
                        f.offset,
                        &f.field_cls,
                        val,
                        Some(f),
                        layout.swapped_bytes,
                        &k,
                    )
                })?;
            }
            for (k, v) in kwargs {
                if assigned.contains(&k) {
                    return Err(PyError::named(
                        "TypeError",
                        format!("duplicate values for field {k:?}"),
                    ));
                }
                _interp.assign_attr(self_val.0.clone(), &k, v)?;
            }
            Ok(Value::none())
        }
    }

    // ── `CFuncPtr` ───────────────────────────────────────────────────────────

    class CFuncPtr("_CData") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let cls = extract_cdata_class(self_val.0)?;
            let (cls_rt, cls_at, cls_flags) = match lookup_ctype(&cls) {
                Some(CTypeInfo::FuncPtr {
                    restype,
                    argtypes,
                    flags,
                }) => (restype, argtypes, flags),
                _ => {
                    // Fallback: check class attributes `_restype_`, `_argtypes_`, `_flags_`
                    let rt = cls.borrow().attrs.get("_restype_").cloned().filter(|v| !v.is_none());
                    let at = cls.borrow().attrs.get("_argtypes_").cloned().and_then(|v| {
                        _interp.collect_iterable(&v).ok()
                    });
                    let fl = cls
                        .borrow()
                        .attrs
                        .get("_flags_")
                        .and_then(|v| v.as_int())
                        .unwrap_or(i64::from(FUNCFLAG_CDECL)) as i32;
                    (rt, at, fl)
                }
            };

            let mut state = CDataState::new(CDataStorage::allocate_zeroed(PTR_SIZE));
            let mut addr: usize = 0;
            let mut name: Option<String> = None;
            let mut py_callable: Option<Value> = None;
            let mut cb_handle = None;

            if let Some(first) = args.first() {
                if first.is_none() {
                    addr = 0;
                } else if matches!(
                    first.kind(),
                    ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Bool(_)
                ) {
                    addr = value_to_u64_wrapping(_interp, first)? as usize;
                } else if let ValueKind::Tuple(tup) = first.kind() {
                    if tup.len() != 2 {
                        return Err(PyError::named(
                            "TypeError",
                            "CFuncPtr tuple argument must be (name_or_ordinal, dll)",
                        ));
                    }
                    let handle_val = _interp.get_attr(&tup[1], "_handle")?;
                    let handle = value_to_u64_wrapping(_interp, &handle_val)? as usize;
                    match tup[0].kind() {
                        ValueKind::Str(sym) => {
                            addr = dynlib::lookup_symbol(handle, sym)?;
                            name = Some(sym.to_string());
                        }
                        #[cfg(windows)]
                        ValueKind::Int(ord) => {
                            addr = dynlib::get_proc_address_ordinal(handle, ord as u16)?;
                            name = Some(format!("#{ord}"));
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "symbol name must be a string",
                            ));
                        }
                    }
                    state.keep_alive_vals.insert("dll".to_string(), tup[1].clone());
                } else if crate::interpreter::slot_is_callable(first) {
                    let argtypes_vec = cls_at.clone().unwrap_or_default();
                    let (tramp_addr, handle) = abi::allocate_callback(
                        first.clone(),
                        cls_rt.clone(),
                        argtypes_vec,
                    )?;
                    addr = tramp_addr;
                    py_callable = Some(first.clone());
                    cb_handle = Some(handle);
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "the first argument must be a callable, integer address, or (name, dll) tuple, got {}",
                            value_type_name_str(first)
                        ),
                    ));
                }
            }

            write_ptr_usize(&state.storage, 0, addr, ByteOrder::Native)?;
            state.func_ptr = Some(FuncPtrMeta {
                restype: cls_rt,
                argtypes: cls_at,
                errcheck: None,
                flags: cls_flags,
                name,
                callable: py_callable,
                _callback_handle: cb_handle,
            });
            set_native_state(self_val.0, state)?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            cfuncptr_call_impl(_interp, self_val.0, args)
        }

        #[getter]
        fn restype(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_native_state::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                Ok(st
                    .func_ptr
                    .as_ref()
                    .and_then(|m| m.restype.clone())
                    .unwrap_or_else(Value::none))
            })
        }

        #[setter("restype")]
        fn set_restype(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let new_rt = if val.0.is_none() {
                None
            } else if matches!(val.0.kind(), ValueKind::PyClass(_))
                || crate::interpreter::slot_is_callable(&val.0)
            {
                Some(val.0)
            } else {
                return Err(PyError::named(
                    "TypeError",
                    "restype must be a type, a callable, or None",
                ));
            };
            with_native_state_mut::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                if let Some(ref mut meta) = st.func_ptr {
                    meta.restype = new_rt;
                }
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn argtypes(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_native_state::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                Ok(match st.func_ptr.as_ref().and_then(|m| m.argtypes.as_ref()) {
                    Some(v) => Value::tuple(v.clone()),
                    None => Value::none(),
                })
            })
        }

        #[setter("argtypes")]
        fn set_argtypes(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let new_at = if val.0.is_none() {
                None
            } else {
                let items = _interp.collect_iterable(&val.0).map_err(|_| {
                    PyError::named("TypeError", "argtypes must be a sequence of types")
                })?;
                Some(items)
            };
            with_native_state_mut::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                if let Some(ref mut meta) = st.func_ptr {
                    meta.argtypes = new_at;
                }
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn errcheck(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_native_state::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                Ok(st
                    .func_ptr
                    .as_ref()
                    .and_then(|m| m.errcheck.clone())
                    .unwrap_or_else(Value::none))
            })
        }

        #[setter("errcheck")]
        fn set_errcheck(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let new_ec = if val.0.is_none() {
                None
            } else if crate::interpreter::slot_is_callable(&val.0) {
                Some(val.0)
            } else {
                return Err(PyError::named("TypeError", "errcheck must be callable"));
            };
            with_native_state_mut::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                if let Some(ref mut meta) = st.func_ptr {
                    meta.errcheck = new_ec;
                }
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn __name__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_native_state::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                Ok(Value::string(
                    st.func_ptr
                        .as_ref()
                        .and_then(|m| m.name.clone())
                        .unwrap_or_else(|| "<CFuncPtr>".to_string()),
                ))
            })
        }

        fn __bool__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let addr = with_native_state::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                read_ptr_usize(&st.storage, 0, ByteOrder::Native)
            })?;
            Ok(Value::bool_(addr != 0))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (name, addr) = with_native_state::<CDataState, _>(self_val.0, "CFuncPtr", |st| {
                let a = read_ptr_usize(&st.storage, 0, ByteOrder::Native)?;
                let n = st
                    .func_ptr
                    .as_ref()
                    .and_then(|m| m.name.clone())
                    .unwrap_or_else(|| "CFuncPtr".to_string());
                Ok((n, a))
            })?;
            Ok(Value::string(format!("<_FuncPtr '{name}' at {addr:#x}>")))
        }
    }

    // ── Module-level `_ctypes` Functions ─────────────────────────────────────

    fn sizeof(#[positional_only] obj: PyValue) -> Result<Value> {
        match obj.0.kind() {
            ValueKind::PyClass(cls) => {
                let info = ensure_ctype_info(_interp, cls)?;
                Ok(Value::int(info.size() as i64))
            }
            ValueKind::PyInstance(_) => {
                let sz = with_native_state::<CDataState, _>(&obj.0, "_CData", |st| {
                    Ok(st.storage.size())
                })
                .map_err(|_| PyError::named("TypeError", "this type has no size"))?;
                Ok(Value::int(sz as i64))
            }
            _ => Err(PyError::named("TypeError", "this type has no size")),
        }
    }

    fn alignment(#[positional_only] obj: PyValue) -> Result<Value> {
        let cls = match obj.0.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            ValueKind::PyInstance(inst) => Rc::clone(&inst.borrow().class),
            _ => return Err(PyError::named("TypeError", "no alignment info")),
        };
        let info = ensure_ctype_info(_interp, &cls)
            .map_err(|_| PyError::named("TypeError", "no alignment info"))?;
        Ok(Value::int(info.align() as i64))
    }

    fn addressof(#[positional_only] obj: PyValue) -> Result<Value> {
        let addr = with_native_state::<CDataState, _>(&obj.0, "_CData", |st| {
            Ok(st.storage.address())
        })
        .map_err(|_| PyError::named("TypeError", "invalid type"))?;
        Ok(u64_to_py_value(addr as u64))
    }

    fn byref(
        #[positional_only] obj: PyValue,
        #[positional_only]
        #[default(0_i64.into())]
        offset: PyIndex,
    ) -> Result<Value> {
        let off = offset.expect_isize(FN_NAME, "offset")?;
        let base_addr = with_native_state::<CDataState, _>(&obj.0, "_CData", |st| {
            Ok(st.storage.address())
        })
        .map_err(|_| {
            PyError::named(
                "TypeError",
                format!(
                    "byref() argument must be a ctypes instance, not '{}'",
                    value_type_name_str(&obj.0)
                ),
            )
        })?;
        let addr = (base_addr as isize).wrapping_add(off) as usize;
        let tag = "LP".to_string();
        create_carg_instance(obj.0, off, addr, tag)
    }

    fn resize(
        #[positional_only] obj: PyValue,
        #[positional_only] size: PyIndex,
    ) -> Result<Value> {
        let new_sz_i64 = size.expect_i64(FN_NAME, "size")?;
        let cls = extract_cdata_class(&obj.0)?;
        let info = ensure_ctype_info(_interp, &cls)?;
        let min_size = info.size();
        if new_sz_i64 < 0 || (new_sz_i64 as usize) < min_size {
            return Err(PyError::named(
                "ValueError",
                format!("minimum size is {min_size}"),
            ));
        }
        with_native_state_mut::<CDataState, _>(&obj.0, "_CData", |st| {
            st.storage.resize(new_sz_i64 as usize)
        })?;
        Ok(Value::none())
    }

    #[py_name = "POINTER"]
    fn pointer_type(#[positional_only] cls: PyValue) -> Result<Value> {
        if cls.0.is_none() {
            let ptr_cls = make_pointer_type(_interp, None)?;
            return Ok(Value::py_class(ptr_cls));
        }
        if let ValueKind::Str(name) = cls.0.kind() {
            // Incomplete named pointer type (e.g. `POINTER("Node")`)
            let ptr_cls = make_pointer_type(_interp, None)?;
            ptr_cls.borrow_mut().name = format!("LP_{name}");
            return Ok(Value::py_class(ptr_cls));
        }
        let ValueKind::PyClass(target_cls) = cls.0.kind() else {
            return Err(PyError::named("TypeError", "must have storage info"));
        };
        let ptr_cls = make_pointer_type(_interp, Some(target_cls))?;
        Ok(Value::py_class(ptr_cls))
    }

    fn pointer(#[positional_only] obj: PyValue) -> Result<Value> {
        let cls = extract_cdata_class(&obj.0)?;
        let ptr_cls = make_pointer_type(_interp, Some(&cls))?;
        let addr = with_native_state::<CDataState, _>(&obj.0, "_CData", |st| {
            Ok(st.storage.address())
        })?;
        let mut state = CDataState::new(CDataStorage::allocate_zeroed(PTR_SIZE));
        write_ptr_usize(&state.storage, 0, addr, ByteOrder::Native)?;
        state.keep_alive_vals.insert("target".to_string(), obj.0);
        let inst = Value::py_instance(Rc::new(RefCell::new(crate::value::PyInstance {
            class: ptr_cls,
            attrs: crate::value::InstanceAttrs::new(),
        })));
        set_native_state(&inst, state)?;
        Ok(inst)
    }

    fn get_errno() -> Result<Value> {
        Ok(Value::int(i64::from(dynlib::get_errno())))
    }

    fn set_errno(#[positional_only] value: PyIndex) -> Result<Value> {
        let v = value.expect_i64(FN_NAME, "value")? as i32;
        Ok(Value::int(i64::from(dynlib::set_errno(v))))
    }

    #[cfg(windows)]
    fn get_last_error() -> Result<Value> {
        Ok(Value::int(i64::from(dynlib::get_last_error())))
    }

    #[cfg(windows)]
    fn set_last_error(#[positional_only] value: PyIndex) -> Result<Value> {
        let v = value.expect_i64(FN_NAME, "value")? as u32;
        Ok(Value::int(i64::from(dynlib::set_last_error(v))))
    }

    #[cfg(windows)]
    #[py_name = "FormatError"]
    fn format_error(
        #[positional_only]
        #[default(None)]
        code: Option<PyIndex>,
    ) -> Result<Value> {
        let c = match code {
            Some(idx) => Some(idx.expect_i64(FN_NAME, "code")? as i32),
            None => None,
        };
        Ok(Value::string(dynlib::format_error(c)))
    }

    #[cfg(windows)]
    #[py_name = "LoadLibrary"]
    fn load_library(
        #[positional_only] name: PyStr<'_>,
        #[positional_only]
        #[default(0_i64.into())]
        _mode: PyIndex,
    ) -> Result<Value> {
        let handle = dynlib::load_library(&name.0)?;
        Ok(u64_to_py_value(handle as u64))
    }

    #[cfg(windows)]
    #[py_name = "FreeLibrary"]
    fn free_library(#[positional_only] handle: PyIndex) -> Result<Value> {
        let h = handle.expect_i64(FN_NAME, "handle")? as usize;
        dynlib::free_library(h)?;
        Ok(Value::none())
    }

    #[cfg(windows)]
    fn _check_HRESULT(#[positional_only] hr: PyIndex) -> Result<Value> {
        let v = hr.expect_i64(FN_NAME, "hr")? as i32;
        if v < 0 {
            let msg = dynlib::format_error(Some(v));
            return Err(PyError::named("OSError", format!("[WinError {v}] {msg}")));
        }
        Ok(Value::int(i64::from(v)))
    }

    #[cfg(unix)]
    fn dlopen(
        #[positional_only] name: Option<PyStr<'_>>,
        #[positional_only]
        #[default(None)]
        mode: Option<PyIndex>,
    ) -> Result<Value> {
        let m = match mode {
            Some(idx) => idx.expect_i64(FN_NAME, "mode")? as i32,
            None => dynlib::default_mode() as i32,
        };
        let handle = dynlib::dlopen(name.as_deref(), m)?;
        Ok(u64_to_py_value(handle as u64))
    }

    #[cfg(unix)]
    fn dlclose(#[positional_only] handle: PyIndex) -> Result<Value> {
        let h = handle.expect_i64(FN_NAME, "handle")? as usize;
        dynlib::dlclose(h)?;
        Ok(Value::none())
    }

    #[cfg(unix)]
    fn dlsym(
        #[positional_only] handle: PyIndex,
        #[positional_only] name: PyStr<'_>,
    ) -> Result<Value> {
        let h = handle.expect_i64(FN_NAME, "handle")? as usize;
        let addr = dynlib::dlsym(h, &name.0)?;
        Ok(u64_to_py_value(addr as u64))
    }

    fn buffer_info(#[positional_only] obj: PyValue) -> Result<Value> {
        let cls = match obj.0.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            ValueKind::PyInstance(inst) => Rc::clone(&inst.borrow().class),
            _ => return Err(PyError::named("TypeError", "not a ctypes type or instance")),
        };
        let info = ensure_ctype_info(_interp, &cls)?;
        match info {
            CTypeInfo::Simple { kind, .. } => Ok(Value::tuple(vec![
                Value::string(kind.code()),
                Value::int(0),
                Value::tuple(vec![]),
            ])),
            CTypeInfo::Array { length, .. } => Ok(Value::tuple(vec![
                Value::string("B"),
                Value::int(1),
                Value::tuple(vec![Value::int(length as i64)]),
            ])),
            _ => Ok(Value::tuple(vec![
                Value::string("B"),
                Value::int(0),
                Value::tuple(vec![]),
            ])),
        }
    }

    fn _cast(
        #[positional_only] obj: PyValue,
        #[positional_only] _obj_orig: PyValue,
        #[positional_only] typ: PyValue,
    ) -> Result<Value> {
        let ValueKind::PyClass(target_cls) = typ.0.kind() else {
            return Err(PyError::named("TypeError", "cast() argument 2 must be a pointer type"));
        };
        let info = ensure_ctype_info(_interp, target_cls)?;
        let is_ptr_like = matches!(
            info,
            CTypeInfo::Pointer { .. }
                | CTypeInfo::FuncPtr { .. }
                | CTypeInfo::Simple {
                    kind: PrimKind::VoidP | PrimKind::CharP | PrimKind::WCharP,
                    ..
                }
        );
        if !is_ptr_like {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "cast() argument 2 must be a pointer type, not {}",
                    target_cls.borrow().name
                ),
            ));
        }

        let (addr, temp_buf) = if obj.0.is_none() {
            (0usize, None)
        } else if let Ok(a) = extract_pointer_or_cdata_addr(&obj.0) {
            (a, None)
        } else {
            extract_memory_address(_interp, &obj.0)?
        };

        let mut state = CDataState::new(CDataStorage::allocate_zeroed(PTR_SIZE));
        write_ptr_usize(&state.storage, 0, addr, ByteOrder::Native)?;
        if let Some(buf) = temp_buf {
            state.keep_alive_bufs.insert("cast_buf".to_string(), buf);
        }
        state.keep_alive_vals.insert("cast_src".to_string(), obj.0);

        let inst = Value::py_instance(Rc::new(RefCell::new(crate::value::PyInstance {
            class: Rc::clone(target_cls),
            attrs: crate::value::InstanceAttrs::new(),
        })));
        set_native_state(&inst, state)?;
        Ok(inst)
    }

    fn _memmove(
        #[positional_only] dst: PyValue,
        #[positional_only] src: PyValue,
        #[positional_only] count: PyIndex,
    ) -> Result<Value> {
        let n_i64 = count.expect_i64(FN_NAME, "count")?;
        if n_i64 < 0 {
            return Err(PyError::named("ValueError", "count must be non-negative"));
        }
        let n = n_i64 as usize;
        let (dst_addr, _dst_keep) = extract_memory_address(_interp, &dst.0)?;
        let (src_addr, _src_keep) = extract_memory_address(_interp, &src.0)?;
        if n > 0 {
            if dst_addr == 0 || src_addr == 0 {
                return Err(PyError::named("ValueError", "NULL pointer access"));
            }
            // SAFETY: caller explicitly requested `memmove(dst, src, n)` on valid memory.
            unsafe {
                std::ptr::copy(src_addr as *const u8, dst_addr as *mut u8, n);
            }
        }
        Ok(u64_to_py_value(dst_addr as u64))
    }

    fn _memset(
        #[positional_only] dst: PyValue,
        #[positional_only] c: PyIndex,
        #[positional_only] count: PyIndex,
    ) -> Result<Value> {
        let byte_val = c.expect_i64(FN_NAME, "c")? as u8;
        let n_i64 = count.expect_i64(FN_NAME, "count")?;
        if n_i64 < 0 {
            return Err(PyError::named("ValueError", "count must be non-negative"));
        }
        let n = n_i64 as usize;
        let (dst_addr, _dst_keep) = extract_memory_address(_interp, &dst.0)?;
        if n > 0 {
            if dst_addr == 0 {
                return Err(PyError::named("ValueError", "NULL pointer access"));
            }
            // SAFETY: caller explicitly requested `memset(dst, c, n)` on valid memory.
            unsafe {
                std::ptr::write_bytes(dst_addr as *mut u8, byte_val, n);
            }
        }
        Ok(u64_to_py_value(dst_addr as u64))
    }

    fn _string_at(
        #[positional_only] ptr: PyValue,
        #[positional_only]
        #[default((-1_i64).into())]
        size: PyIndex,
    ) -> Result<Value> {
        let sz = size.expect_i64(FN_NAME, "size")?;
        let (addr, _keep) = extract_memory_address(_interp, &ptr.0)?;
        let max_len = if sz < 0 { None } else { Some(sz as usize) };
        let bytes = read_c_string(addr, max_len)?;
        Ok(Value::bytes(bytes))
    }

    fn _wstring_at(
        #[positional_only] ptr: PyValue,
        #[positional_only]
        #[default((-1_i64).into())]
        size: PyIndex,
    ) -> Result<Value> {
        let sz = size.expect_i64(FN_NAME, "size")?;
        let (addr, _keep) = extract_memory_address(_interp, &ptr.0)?;
        let max_chars = if sz < 0 { None } else { Some(sz as usize) };
        let text = read_c_wstring(addr, max_chars)?;
        Ok(Value::string(text))
    }
}

fn get_class_attr(module: &Rc<RefCell<PyModule>>, name: &str) -> Result<Rc<RefCell<PyClass>>> {
    let val = module
        .borrow()
        .get_attr_value(name)
        .ok_or_else(|| PyError::runtime(format!("internal: _ctypes missing class '{name}'")))?;
    match val.kind() {
        ValueKind::PyClass(cls) => Ok(Rc::clone(cls)),
        _ => Err(PyError::runtime(format!(
            "internal: _ctypes.{name} is not a class"
        ))),
    }
}

/// Wire up metaclasses on `_ctypes` base types and register core class handles.
pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let cdata_meta = get_class_attr(module, "_CDataMeta")?;
    let simple_meta = get_class_attr(module, "PyCSimpleType")?;
    let array_meta = get_class_attr(module, "PyCArrayType")?;
    let pointer_meta = get_class_attr(module, "PyCPointerType")?;
    let struct_meta = get_class_attr(module, "PyCStructType")?;
    let union_meta = get_class_attr(module, "UnionType")?;
    let funcptr_meta = get_class_attr(module, "PyCFuncPtrType")?;

    let cdata_base = get_class_attr(module, "_CData")?;
    let simple_base = get_class_attr(module, "_SimpleCData")?;
    let array_base = get_class_attr(module, "Array")?;
    let pointer_base = get_class_attr(module, "_Pointer")?;
    let struct_base = get_class_attr(module, "Structure")?;
    let union_base = get_class_attr(module, "Union")?;
    let funcptr_base = get_class_attr(module, "CFuncPtr")?;
    let carg_cls = get_class_attr(module, "CArgObject")?;
    let cfield_cls = get_class_attr(module, "CField")?;
    let arg_error_cls = get_class_attr(module, "ArgumentError")?;

    // Assign metatypes matching CPython's type hierarchy so `c_type * n`,
    // `Structure`/`Union` subclass `_fields_` finalization, etc. work natively.
    cdata_base.borrow_mut().metatype = Some(Rc::clone(&cdata_meta));
    simple_base.borrow_mut().metatype = Some(Rc::clone(&simple_meta));
    array_base.borrow_mut().metatype = Some(Rc::clone(&array_meta));
    pointer_base.borrow_mut().metatype = Some(Rc::clone(&pointer_meta));
    struct_base.borrow_mut().metatype = Some(Rc::clone(&struct_meta));
    union_base.borrow_mut().metatype = Some(Rc::clone(&union_meta));
    funcptr_base.borrow_mut().metatype = Some(Rc::clone(&funcptr_meta));

    set_core_classes(CoreCtypesClasses {
        _cdata_meta: cdata_meta,
        simple_meta,
        array_meta,
        pointer_meta,
        struct_meta,
        union_meta,
        funcptr_meta,
        cdata_base,
        simple_base,
        array_base,
        pointer_base,
        struct_base,
        union_base,
        funcptr_base,
        carg_cls,
        cfield_cls,
        _arg_error_cls: arg_error_cls,
    });
    Ok(())
}
