// `ctypes` module — 100% native-Rust Python 3.12 foreign function interface (issue #3265).
//
// ZERO `libffi`, ZERO `@inject`, ZERO `*_py.py` files.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::ctypes_rt::dynlib;
use crate::builtin_modules::ctypes_rt::types::{
    ByteOrder, C_LONG_SIZE, CTypeInfo, LONGDOUBLE_SIZE, PTR_SIZE, PrimKind, clear_caches,
    core_classes, register_ctype,
};
use crate::builtin_modules::ctypes_rt::{
    CDataState, CDataStorage, FUNCFLAG_CDECL, FUNCFLAG_HRESULT, FUNCFLAG_PYTHONAPI,
    FUNCFLAG_STDCALL, FUNCFLAG_USE_ERRNO, FUNCFLAG_USE_LASTERROR, FuncPtrMeta,
    create_cdata_instance, make_array_type, make_functype_class, other_endian_type,
    u64_to_py_value, value_to_u64_wrapping, write_ptr_usize, write_wchar_array_value,
};
use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyPath, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{InstanceAttrs, PyClass, PyDict, PyInstance, PyModule, Value, ValueKind};

fn make_prim_class(
    interp: &mut Interpreter,
    name: &str,
    kind: PrimKind,
    simple_base: &Rc<RefCell<PyClass>>,
    simple_meta: &Rc<RefCell<PyClass>>,
) -> Result<Rc<RefCell<PyClass>>> {
    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("ctypes"));
    attrs.insert("_type_".to_string(), Value::string(kind.code()));
    let mut cls = PyClass::new(name, name, Some(Rc::clone(simple_base)), attrs);
    cls.metatype = Some(Rc::clone(simple_meta));
    let cls_rc = Rc::new(RefCell::new(cls));
    simple_base
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&cls_rc));

    register_ctype(
        &cls_rc,
        CTypeInfo::Simple {
            kind,
            byte_order: ByteOrder::Native,
        },
    );

    if kind.supports_byte_swap() {
        let other = other_endian_type(interp, &cls_rc)?;
        let mut m = cls_rc.borrow_mut();
        if cfg!(target_endian = "little") {
            m.attrs.insert(
                "__ctype_le__".to_string(),
                Value::py_class(Rc::clone(&cls_rc)),
            );
            m.attrs
                .insert("__ctype_be__".to_string(), Value::py_class(other));
        } else {
            m.attrs.insert(
                "__ctype_be__".to_string(),
                Value::py_class(Rc::clone(&cls_rc)),
            );
            m.attrs
                .insert("__ctype_le__".to_string(), Value::py_class(other));
        }
    }

    Ok(cls_rc)
}

fn get_ctypes_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let ctypes_mod = interp.load_module("ctypes")?;
    interp.get_attr(&ctypes_mod, name)
}

fn build_dll_funcptr(
    interp: &mut Interpreter,
    dll_val: &Value,
    name_or_ordinal: &Value,
) -> Result<Value> {
    let handle_val = interp.get_attr(dll_val, "_handle")?;
    let handle = value_to_u64_wrapping(interp, &handle_val)? as usize;

    let flags_val = interp
        .get_attr(dll_val, "_flags_")
        .unwrap_or_else(|_| Value::int(i64::from(FUNCFLAG_CDECL)));
    let flags = flags_val.as_int().unwrap_or(i64::from(FUNCFLAG_CDECL)) as i32;

    let default_restype = interp.get_attr(dll_val, "_func_restype_").ok();
    let restype = match default_restype {
        Some(v) if !v.is_none() => Some(v),
        _ => Some(get_ctypes_attr(interp, "c_int")?),
    };

    let (addr, sym_name) = match name_or_ordinal.kind() {
        ValueKind::Str(sym) => (dynlib::lookup_symbol(handle, sym)?, sym.to_string()),
        #[cfg(windows)]
        ValueKind::Int(ord) => (
            dynlib::get_proc_address_ordinal(handle, ord as u16)?,
            format!("#{ord}"),
        ),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "function name must be a string, not '{}'",
                    value_type_name_str(name_or_ordinal)
                ),
            ));
        }
    };

    let core = core_classes()?;
    let mut state = CDataState::new(CDataStorage::allocate_zeroed(PTR_SIZE));
    write_ptr_usize(&state.storage, 0, addr, ByteOrder::Native)?;
    state
        .keep_alive_vals
        .insert("dll".to_string(), dll_val.clone());
    state.func_ptr = Some(FuncPtrMeta {
        restype,
        argtypes: None,
        errcheck: None,
        flags,
        name: Some(sym_name),
        callable: None,
        _callback_handle: None,
    });

    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: core.funcptr_base,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(&inst, state)?;
    Ok(inst)
}

pyrust_module! {
    constants {
        "DEFAULT_MODE" => Value::int(dynlib::default_mode()),
        "RTLD_GLOBAL" => Value::int(dynlib::rtld_global()),
        "RTLD_LOCAL" => Value::int(dynlib::rtld_local()),
        "SIZEOF_TIME_T" => Value::int(std::mem::size_of::<i64>() as i64),
        "_FUNCFLAG_STDCALL" => Value::int(i64::from(FUNCFLAG_STDCALL)),
        "_FUNCFLAG_CDECL" => Value::int(i64::from(FUNCFLAG_CDECL)),
        "_FUNCFLAG_HRESULT" => Value::int(i64::from(FUNCFLAG_HRESULT)),
        "_FUNCFLAG_PYTHONAPI" => Value::int(i64::from(FUNCFLAG_PYTHONAPI)),
        "_FUNCFLAG_USE_ERRNO" => Value::int(i64::from(FUNCFLAG_USE_ERRNO)),
        "_FUNCFLAG_USE_LASTERROR" => Value::int(i64::from(FUNCFLAG_USE_LASTERROR)),
        "_ctypes_version" => Value::string("1.1.0"),
        "_c_functype_cache" => Value::dict(PyDict::default()),
        #[cfg(windows)]
        "_win_functype_cache" => Value::dict(PyDict::default()),
    }

    // ── Dynamic Library Classes ──────────────────────────────────────────────

    class CDLL {
        constants {
            "_func_flags_" => Value::int(i64::from(FUNCFLAG_CDECL)),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: Option<PyPath>,
            #[default(dynlib::default_mode().into())] mode: PyIndex,
            #[default(None)] handle: Option<PyIndex>,
            #[default(false.into())] use_errno: PyBool,
            #[default(false.into())] use_last_error: PyBool,
            #[default(None)] _winmode: Option<PyValue>,
        ) -> Result<Value> {
            let mode_i32 = mode.expect_i64(FN_NAME, "mode")? as i32;
            let base_flags = _interp
                .get_attr(self_val.0, "_func_flags_")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(i64::from(FUNCFLAG_CDECL)) as i32;
            let mut flags = base_flags;
            if use_errno.0 {
                flags |= FUNCFLAG_USE_ERRNO;
            }
            if use_last_error.0 {
                flags |= FUNCFLAG_USE_LASTERROR;
            }

            let name_str: Option<String> = name.map(|p| p.as_path().to_string_lossy().into_owned());
            let handle_usize = match handle {
                Some(h) => h.expect_i64(FN_NAME, "handle")? as usize,
                None => dynlib::open_library(name_str.as_deref(), mode_i32)?,
            };

            let name_val = match name_str {
                Some(s) => Value::string(s),
                None => Value::none(),
            };
            _interp.assign_attr(self_val.0.clone(), "_name", name_val)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_handle",
                u64_to_py_value(handle_usize as u64),
            )?;
            _interp.assign_attr(self_val.0.clone(), "_flags_", Value::int(i64::from(flags)))?;
            Ok(Value::none())
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            let s: &str = &name.0;
            if s.starts_with("__") && s.ends_with("__") {
                return Err(PyError::named("AttributeError", s));
            }
            let func = build_dll_funcptr(_interp, self_val.0, &Value::string(s))?;
            if !s.starts_with('_') {
                let _ = _interp.assign_attr(self_val.0.clone(), s, func.clone());
            }
            Ok(func)
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name_or_ordinal: PyValue,
        ) -> Result<Value> {
            build_dll_funcptr(_interp, self_val.0, &name_or_ordinal.0)
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls_name = match self_val.0.kind() {
                ValueKind::PyInstance(i) => i.borrow().class.borrow().name.clone(),
                _ => "CDLL".to_string(),
            };
            let name_val = _interp
                .get_attr(self_val.0, "_name")
                .unwrap_or_else(|_| Value::none());
            let handle_val = _interp
                .get_attr(self_val.0, "_handle")
                .unwrap_or_else(|_| Value::int(0));
            let h = value_to_u64_wrapping(_interp, &handle_val).unwrap_or(0);
            let name_str = match name_val.kind() {
                ValueKind::Str(s) => format!("'{s}'"),
                _ => "None".to_string(),
            };
            Ok(Value::string(format!(
                "<{cls_name} {name_str}, handle {h:#x}>"
            )))
        }
    }

    class PyDLL("CDLL") {
        constants {
            "_func_flags_" => Value::int(i64::from(FUNCFLAG_CDECL | FUNCFLAG_PYTHONAPI)),
        }
    }

    #[cfg(windows)]
    class WinDLL("CDLL") {
        constants {
            "_func_flags_" => Value::int(i64::from(FUNCFLAG_STDCALL)),
        }
    }

    #[cfg(windows)]
    class OleDLL("CDLL") {
        constants {
            "_func_flags_" => Value::int(i64::from(FUNCFLAG_STDCALL | FUNCFLAG_HRESULT)),
        }
    }

    class LibraryLoader {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            dlltype: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_dlltype", dlltype.0)?;
            Ok(Value::none())
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            let s: &str = &name.0;
            if s.starts_with('_') {
                return Err(PyError::named("AttributeError", s));
            }
            let dlltype = _interp.get_attr(self_val.0, "_dlltype")?;
            let dll = _interp.call_function_expanded(
                dlltype,
                &[ExpandedCallArg {
                    name: None,
                    value: Value::string(s),
                }],
            )?;
            _interp.assign_attr(self_val.0.clone(), s, dll.clone())?;
            Ok(dll)
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            _interp.get_attr(self_val.0, &name.0)
        }

        #[py_name = "LoadLibrary"]
        fn load_library(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyPath,
        ) -> Result<Value> {
            let dlltype = _interp.get_attr(self_val.0, "_dlltype")?;
            _interp.call_function_expanded(
                dlltype,
                &[ExpandedCallArg {
                    name: None,
                    value: Value::string(name.as_path().to_string_lossy().as_ref()),
                }],
            )
        }
    }

    // ── High-Level `ctypes` Functions ────────────────────────────────────────

    #[py_name = "ARRAY"]
    fn array_fn(
        #[positional_only] typ: PyValue,
        #[positional_only] len: PyIndex,
    ) -> Result<Value> {
        let ValueKind::PyClass(cls) = typ.0.kind() else {
            return Err(PyError::named("TypeError", "first argument must be a ctypes type"));
        };
        let n = len.expect_i64(FN_NAME, "len")?;
        if n < 0 {
            return Err(PyError::named(
                "ValueError",
                format!("Array length must be >= 0, not {n}"),
            ));
        }
        let arr_cls = make_array_type(_interp, cls, n as usize)?;
        Ok(Value::py_class(arr_cls))
    }

    #[py_name = "SetPointerType"]
    fn set_pointer_type(
        #[positional_only] pointer_cls: PyValue,
        #[positional_only] cls: PyValue,
    ) -> Result<Value> {
        let method = _interp.get_attr(&pointer_cls.0, "set_type")?;
        _interp.call_function_expanded(
            method,
            &[ExpandedCallArg {
                name: None,
                value: cls.0,
            }],
        )
    }

    fn create_string_buffer(
        init: PyValue,
        #[default(None)] size: Option<PyIndex>,
    ) -> Result<Value> {
        let c_char_val = get_ctypes_attr(_interp, "c_char")?;
        let ValueKind::PyClass(c_char_cls) = c_char_val.kind() else {
            return Err(PyError::runtime("internal: ctypes.c_char not a class".to_string()));
        };
        let c_char_cls = Rc::clone(c_char_cls);

        match init.0.kind() {
            ValueKind::Bytes(b) => {
                let bytes = b.to_vec();
                let buf_size = match size {
                    None => bytes.len() + 1,
                    Some(sz_idx) => {
                        let sz = sz_idx.expect_i64(FN_NAME, "size")?;
                        if sz < 0 || (sz as usize) < bytes.len() {
                            return Err(PyError::named(
                                "ValueError",
                                format!("Buffer size ({sz}) too small for bytes of length {}", bytes.len()),
                            ));
                        }
                        sz as usize
                    }
                };
                let arr_cls = make_array_type(_interp, &c_char_cls, buf_size)?;
                let storage = CDataStorage::allocate_zeroed(buf_size);
                storage.write_bytes(0, &bytes)?;
                create_cdata_instance(arr_cls, storage)
            }
            ValueKind::Int(n) => {
                if size.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "size argument not allowed when init is an int",
                    ));
                }
                if n < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        format!("Buffer size must be >= 0, not {n}"),
                    ));
                }
                let len = n as usize;
                let arr_cls = make_array_type(_interp, &c_char_cls, len)?;
                let storage = CDataStorage::allocate_zeroed(len);
                create_cdata_instance(arr_cls, storage)
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "bytes or int expected, got {}",
                    value_type_name_str(&init.0)
                ),
            )),
        }
    }

    fn c_buffer(
        init: PyValue,
        #[default(None)] size: Option<PyIndex>,
    ) -> Result<Value> {
        let mut args = vec![ExpandedCallArg {
            name: None,
            value: init.0,
        }];
        if let Some(s) = size {
            args.push(ExpandedCallArg {
                name: None,
                value: Value::int(s.expect_i64(FN_NAME, "size")?),
            });
        }
        let f = get_ctypes_attr(_interp, "create_string_buffer")?;
        _interp.call_function_expanded(f, &args)
    }

    fn create_unicode_buffer(
        init: PyValue,
        #[default(None)] size: Option<PyIndex>,
    ) -> Result<Value> {
        let c_wchar_val = get_ctypes_attr(_interp, "c_wchar")?;
        let ValueKind::PyClass(c_wchar_cls) = c_wchar_val.kind() else {
            return Err(PyError::runtime("internal: ctypes.c_wchar not a class".to_string()));
        };
        let c_wchar_cls = Rc::clone(c_wchar_cls);
        let wchar_sz = crate::builtin_modules::ctypes_rt::types::WCHAR_SIZE;

        match init.0.kind() {
            ValueKind::Str(s) => {
                let text = s.to_string();
                let unit_len = if wchar_sz == 2 {
                    text.encode_utf16().count()
                } else {
                    text.chars().count()
                };
                let buf_len = match size {
                    None => unit_len + 1,
                    Some(sz_idx) => {
                        let sz = sz_idx.expect_i64(FN_NAME, "size")?;
                        if sz < 0 || (sz as usize) < unit_len {
                            return Err(PyError::named(
                                "ValueError",
                                format!("Buffer size ({sz}) too small for string of length {unit_len}"),
                            ));
                        }
                        sz as usize
                    }
                };
                let arr_cls = make_array_type(_interp, &c_wchar_cls, buf_len)?;
                let storage = CDataStorage::allocate_zeroed(buf_len * wchar_sz);
                write_wchar_array_value(&storage, 0, buf_len, &text, ByteOrder::Native)?;
                create_cdata_instance(arr_cls, storage)
            }
            ValueKind::Int(n) => {
                if size.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "size argument not allowed when init is an int",
                    ));
                }
                if n < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        format!("Buffer size must be >= 0, not {n}"),
                    ));
                }
                let len = n as usize;
                let arr_cls = make_array_type(_interp, &c_wchar_cls, len)?;
                let storage = CDataStorage::allocate_zeroed(len * wchar_sz);
                create_cdata_instance(arr_cls, storage)
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "str or int expected, got {}",
                    value_type_name_str(&init.0)
                ),
            )),
        }
    }

    fn cast(obj: PyValue, typ: PyValue) -> Result<Value> {
        let _ctypes_mod = _interp.load_module("_ctypes")?;
        let cast_fn = _interp.get_attr(&_ctypes_mod, "_cast")?;
        _interp.call_function_expanded(
            cast_fn,
            &[
                ExpandedCallArg {
                    name: None,
                    value: obj.0.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: obj.0,
                },
                ExpandedCallArg {
                    name: None,
                    value: typ.0,
                },
            ],
        )
    }

    fn memmove(dst: PyValue, src: PyValue, count: PyIndex) -> Result<Value> {
        let _ctypes_mod = _interp.load_module("_ctypes")?;
        let fn_val = _interp.get_attr(&_ctypes_mod, "_memmove")?;
        _interp.call_function_expanded(
            fn_val,
            &[
                ExpandedCallArg {
                    name: None,
                    value: dst.0,
                },
                ExpandedCallArg {
                    name: None,
                    value: src.0,
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::int(count.expect_i64(FN_NAME, "count")?),
                },
            ],
        )
    }

    fn memset(dst: PyValue, c: PyIndex, count: PyIndex) -> Result<Value> {
        let _ctypes_mod = _interp.load_module("_ctypes")?;
        let fn_val = _interp.get_attr(&_ctypes_mod, "_memset")?;
        _interp.call_function_expanded(
            fn_val,
            &[
                ExpandedCallArg {
                    name: None,
                    value: dst.0,
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::int(c.expect_i64(FN_NAME, "c")?),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::int(count.expect_i64(FN_NAME, "count")?),
                },
            ],
        )
    }

    fn string_at(
        ptr: PyValue,
        #[default((-1_i64).into())] size: PyIndex,
    ) -> Result<Value> {
        let _ctypes_mod = _interp.load_module("_ctypes")?;
        let fn_val = _interp.get_attr(&_ctypes_mod, "_string_at")?;
        _interp.call_function_expanded(
            fn_val,
            &[
                ExpandedCallArg {
                    name: None,
                    value: ptr.0,
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::int(size.expect_i64(FN_NAME, "size")?),
                },
            ],
        )
    }

    fn wstring_at(
        ptr: PyValue,
        #[default((-1_i64).into())] size: PyIndex,
    ) -> Result<Value> {
        let _ctypes_mod = _interp.load_module("_ctypes")?;
        let fn_val = _interp.get_attr(&_ctypes_mod, "_wstring_at")?;
        _interp.call_function_expanded(
            fn_val,
            &[
                ExpandedCallArg {
                    name: None,
                    value: ptr.0,
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::int(size.expect_i64(FN_NAME, "size")?),
                },
            ],
        )
    }

    #[py_name = "CFUNCTYPE"]
    fn cfunctype(
        restype: PyValue,
        #[varargs] argtypes: &[Value],
        #[default(false.into())] use_errno: PyBool,
        #[default(false.into())] use_last_error: PyBool,
    ) -> Result<Value> {
        let mut flags = FUNCFLAG_CDECL;
        if use_errno.0 {
            flags |= FUNCFLAG_USE_ERRNO;
        }
        if use_last_error.0 {
            flags |= FUNCFLAG_USE_LASTERROR;
        }
        let rt = if restype.0.is_none() {
            None
        } else {
            Some(restype.0)
        };
        let cls = make_functype_class(rt, argtypes.to_vec(), flags)?;
        Ok(Value::py_class(cls))
    }

    #[py_name = "PYFUNCTYPE"]
    fn pyfunctype(
        restype: PyValue,
        #[varargs] argtypes: &[Value],
    ) -> Result<Value> {
        let flags = FUNCFLAG_CDECL | FUNCFLAG_PYTHONAPI;
        let rt = if restype.0.is_none() {
            None
        } else {
            Some(restype.0)
        };
        let cls = make_functype_class(rt, argtypes.to_vec(), flags)?;
        Ok(Value::py_class(cls))
    }

    #[cfg(windows)]
    #[py_name = "WINFUNCTYPE"]
    fn winfunctype(
        restype: PyValue,
        #[varargs] argtypes: &[Value],
        #[default(false.into())] use_errno: PyBool,
        #[default(false.into())] use_last_error: PyBool,
    ) -> Result<Value> {
        let mut flags = FUNCFLAG_STDCALL;
        if use_errno.0 {
            flags |= FUNCFLAG_USE_ERRNO;
        }
        if use_last_error.0 {
            flags |= FUNCFLAG_USE_LASTERROR;
        }
        let rt = if restype.0.is_none() {
            None
        } else {
            Some(restype.0)
        };
        let cls = make_functype_class(rt, argtypes.to_vec(), flags)?;
        Ok(Value::py_class(cls))
    }

    #[cfg(windows)]
    #[py_name = "GetLastError"]
    fn get_os_last_error_fn() -> Result<Value> {
        Ok(Value::int(i64::from(dynlib::get_os_last_error())))
    }

    #[cfg(windows)]
    #[py_name = "WinError"]
    fn win_error(
        #[default(None)] code: Option<PyIndex>,
        #[default(None)] descr: Option<PyStr<'_>>,
    ) -> Result<Value> {
        let err_code = match code {
            Some(c) => c.expect_i64(FN_NAME, "code")? as i32,
            None => dynlib::get_os_last_error() as i32,
        };
        let msg = match descr {
            Some(s) => s.0.to_string(),
            None => dynlib::format_error(Some(err_code)),
        };
        let oserror_cls = crate::interpreter::lookup_exc_class("OSError")
            .ok_or_else(|| PyError::runtime("OSError class not found".to_string()))?;
        let exc = _interp.call_function_expanded(
            Value::py_class(oserror_cls),
            &[
                ExpandedCallArg {
                    name: None,
                    value: Value::int(i64::from(err_code)),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string(msg.clone()),
                },
            ],
        )?;
        let _ = _interp.assign_attr(exc.clone(), "winerror", Value::int(i64::from(err_code)));
        let _ = _interp.assign_attr(exc.clone(), "strerror", Value::string(msg));
        Ok(exc)
    }

    fn _reset_cache() -> Result<Value> {
        clear_caches();
        Ok(Value::none())
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let _ctypes_mod = interp.load_module("_ctypes")?;
    let endian_mod = interp.load_module("ctypes._endian")?;
    let core = core_classes()?;

    // Re-export core `_ctypes` symbols
    let reexports: &[(&str, &str)] = &[
        ("ArgumentError", "ArgumentError"),
        ("Array", "Array"),
        ("Structure", "Structure"),
        ("Union", "Union"),
        ("_Pointer", "_Pointer"),
        ("_SimpleCData", "_SimpleCData"),
        ("_CFuncPtr", "CFuncPtr"),
        ("POINTER", "POINTER"),
        ("pointer", "pointer"),
        ("byref", "byref"),
        ("sizeof", "sizeof"),
        ("alignment", "alignment"),
        ("addressof", "addressof"),
        ("resize", "resize"),
        ("get_errno", "get_errno"),
        ("set_errno", "set_errno"),
        ("_pointer_type_cache", "_pointer_type_cache"),
        ("_cast_addr", "_cast_addr"),
        ("_memmove_addr", "_memmove_addr"),
        ("_memset_addr", "_memset_addr"),
        ("_string_at_addr", "_string_at_addr"),
        ("_wstring_at_addr", "_wstring_at_addr"),
        #[cfg(windows)]
        ("FormatError", "FormatError"),
        #[cfg(windows)]
        ("get_last_error", "get_last_error"),
        #[cfg(windows)]
        ("set_last_error", "set_last_error"),
        #[cfg(windows)]
        ("_check_HRESULT", "_check_HRESULT"),
        #[cfg(unix)]
        ("_dlopen", "dlopen"),
        #[cfg(windows)]
        ("_dlopen", "LoadLibrary"),
    ];
    for &(dst_name, src_name) in reexports {
        if let Ok(v) = interp.get_attr(&_ctypes_mod, src_name) {
            module.borrow_mut().insert_attr(dst_name.to_string(), v);
        }
    }

    // Re-export endianness structures/unions from `ctypes._endian`
    for name in [
        "BigEndianStructure",
        "LittleEndianStructure",
        "BigEndianUnion",
        "LittleEndianUnion",
    ] {
        let v = interp.get_attr(&endian_mod, name)?;
        module.borrow_mut().insert_attr(name.to_string(), v);
    }
    module
        .borrow_mut()
        .insert_attr("_endian".to_string(), endian_mod);

    // Build all primitive C types with exact CPython 3.12 platform aliasing
    let c_bool = make_prim_class(
        interp,
        "c_bool",
        PrimKind::Bool,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_char = make_prim_class(
        interp,
        "c_char",
        PrimKind::Char,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_wchar = make_prim_class(
        interp,
        "c_wchar",
        PrimKind::WChar,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_byte = make_prim_class(
        interp,
        "c_byte",
        PrimKind::Byte,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_ubyte = make_prim_class(
        interp,
        "c_ubyte",
        PrimKind::UByte,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_short = make_prim_class(
        interp,
        "c_short",
        PrimKind::Short,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_ushort = make_prim_class(
        interp,
        "c_ushort",
        PrimKind::UShort,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_long = make_prim_class(
        interp,
        "c_long",
        PrimKind::Long,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_ulong = make_prim_class(
        interp,
        "c_ulong",
        PrimKind::ULong,
        &core.simple_base,
        &core.simple_meta,
    )?;

    let (c_int, c_uint) = if C_LONG_SIZE == 4 {
        (Rc::clone(&c_long), Rc::clone(&c_ulong))
    } else {
        (
            make_prim_class(
                interp,
                "c_int",
                PrimKind::Int,
                &core.simple_base,
                &core.simple_meta,
            )?,
            make_prim_class(
                interp,
                "c_uint",
                PrimKind::UInt,
                &core.simple_base,
                &core.simple_meta,
            )?,
        )
    };

    let (c_longlong, c_ulonglong) = if C_LONG_SIZE == 8 {
        (Rc::clone(&c_long), Rc::clone(&c_ulong))
    } else {
        (
            make_prim_class(
                interp,
                "c_longlong",
                PrimKind::LongLong,
                &core.simple_base,
                &core.simple_meta,
            )?,
            make_prim_class(
                interp,
                "c_ulonglong",
                PrimKind::ULongLong,
                &core.simple_base,
                &core.simple_meta,
            )?,
        )
    };

    let c_float = make_prim_class(
        interp,
        "c_float",
        PrimKind::Float,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_double = make_prim_class(
        interp,
        "c_double",
        PrimKind::Double,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_longdouble = if LONGDOUBLE_SIZE == 8 {
        Rc::clone(&c_double)
    } else {
        make_prim_class(
            interp,
            "c_longdouble",
            PrimKind::LongDouble,
            &core.simple_base,
            &core.simple_meta,
        )?
    };

    let c_char_p = make_prim_class(
        interp,
        "c_char_p",
        PrimKind::CharP,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_wchar_p = make_prim_class(
        interp,
        "c_wchar_p",
        PrimKind::WCharP,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let c_void_p = make_prim_class(
        interp,
        "c_void_p",
        PrimKind::VoidP,
        &core.simple_base,
        &core.simple_meta,
    )?;
    let py_object = make_prim_class(
        interp,
        "py_object",
        PrimKind::PyObject,
        &core.simple_base,
        &core.simple_meta,
    )?;

    #[cfg(windows)]
    let hresult = make_prim_class(
        interp,
        "HRESULT",
        PrimKind::Long,
        &core.simple_base,
        &core.simple_meta,
    )?;

    // Fixed-width & POSIX/size aliases
    let c_int8 = Rc::clone(&c_byte);
    let c_uint8 = Rc::clone(&c_ubyte);
    let c_int16 = Rc::clone(&c_short);
    let c_uint16 = Rc::clone(&c_ushort);
    let c_int32 = Rc::clone(&c_int);
    let c_uint32 = Rc::clone(&c_uint);
    let c_int64 = Rc::clone(&c_longlong);
    let c_uint64 = Rc::clone(&c_ulonglong);
    let c_size_t = if PTR_SIZE == 8 {
        Rc::clone(&c_uint64)
    } else {
        Rc::clone(&c_uint32)
    };
    let c_ssize_t = if PTR_SIZE == 8 {
        Rc::clone(&c_int64)
    } else {
        Rc::clone(&c_int32)
    };
    let c_time_t = Rc::clone(&c_int64);

    // Attach default `_func_restype_ = c_int` on `CDLL` (and `HRESULT` on `OleDLL`)
    if let Some(cdll_val) = module.borrow().get_attr_value("CDLL")
        && let ValueKind::PyClass(cdll_cls) = cdll_val.kind()
    {
        cdll_cls.borrow_mut().attrs.insert(
            "_func_restype_".to_string(),
            Value::py_class(Rc::clone(&c_int)),
        );
    }
    #[cfg(windows)]
    if let Some(oledll_val) = module.borrow().get_attr_value("OleDLL")
        && let ValueKind::PyClass(oledll_cls) = oledll_val.kind()
    {
        oledll_cls.borrow_mut().attrs.insert(
            "_func_restype_".to_string(),
            Value::py_class(Rc::clone(&hresult)),
        );
    }

    {
        let mut m = module.borrow_mut();
        for (name, cls) in [
            ("c_bool", c_bool),
            ("c_char", c_char),
            ("c_wchar", c_wchar),
            ("c_byte", c_byte),
            ("c_ubyte", c_ubyte),
            ("c_short", c_short),
            ("c_ushort", c_ushort),
            ("c_int", c_int),
            ("c_uint", c_uint),
            ("c_long", c_long),
            ("c_ulong", c_ulong),
            ("c_longlong", c_longlong),
            ("c_ulonglong", c_ulonglong),
            ("c_size_t", c_size_t),
            ("c_ssize_t", c_ssize_t),
            ("c_time_t", c_time_t),
            ("c_float", c_float),
            ("c_double", c_double),
            ("c_longdouble", c_longdouble),
            ("c_char_p", c_char_p),
            ("c_wchar_p", c_wchar_p),
            ("c_void_p", Rc::clone(&c_void_p)),
            ("c_voidp", c_void_p),
            ("py_object", py_object),
            ("c_int8", c_int8),
            ("c_uint8", c_uint8),
            ("c_int16", c_int16),
            ("c_uint16", c_uint16),
            ("c_int32", c_int32),
            ("c_uint32", c_uint32),
            ("c_int64", c_int64),
            ("c_uint64", c_uint64),
            #[cfg(windows)]
            ("HRESULT", hresult),
        ] {
            m.insert_attr(name.to_string(), Value::py_class(cls));
        }
    }

    // Instantiate `cdll`, `pydll`, `windll`, `oledll`, and `pythonapi`
    let loader_cls = module
        .borrow()
        .get_attr_value("LibraryLoader")
        .ok_or_else(|| PyError::runtime("internal: LibraryLoader missing".to_string()))?;
    let cdll_cls = module
        .borrow()
        .get_attr_value("CDLL")
        .ok_or_else(|| PyError::runtime("internal: CDLL missing".to_string()))?;
    let pydll_cls = module
        .borrow()
        .get_attr_value("PyDLL")
        .ok_or_else(|| PyError::runtime("internal: PyDLL missing".to_string()))?;

    let cdll_inst = interp.call_function_expanded(
        loader_cls.clone(),
        &[ExpandedCallArg {
            name: None,
            value: cdll_cls,
        }],
    )?;
    let pydll_inst = interp.call_function_expanded(
        loader_cls.clone(),
        &[ExpandedCallArg {
            name: None,
            value: pydll_cls.clone(),
        }],
    )?;
    let pythonapi_inst = interp.call_function_expanded(
        pydll_cls,
        &[ExpandedCallArg {
            name: None,
            value: Value::none(),
        }],
    )?;

    module
        .borrow_mut()
        .insert_attr("cdll".to_string(), cdll_inst);
    module
        .borrow_mut()
        .insert_attr("pydll".to_string(), pydll_inst);
    module
        .borrow_mut()
        .insert_attr("pythonapi".to_string(), pythonapi_inst);

    #[cfg(windows)]
    {
        let windll_cls = module
            .borrow()
            .get_attr_value("WinDLL")
            .ok_or_else(|| PyError::runtime("internal: WinDLL missing".to_string()))?;
        let oledll_cls = module
            .borrow()
            .get_attr_value("OleDLL")
            .ok_or_else(|| PyError::runtime("internal: OleDLL missing".to_string()))?;
        let windll_inst = interp.call_function_expanded(
            loader_cls.clone(),
            &[ExpandedCallArg {
                name: None,
                value: windll_cls,
            }],
        )?;
        let oledll_inst = interp.call_function_expanded(
            loader_cls,
            &[ExpandedCallArg {
                name: None,
                value: oledll_cls,
            }],
        )?;
        module
            .borrow_mut()
            .insert_attr("windll".to_string(), windll_inst);
        module
            .borrow_mut()
            .insert_attr("oledll".to_string(), oledll_inst);
    }

    Ok(())
}
