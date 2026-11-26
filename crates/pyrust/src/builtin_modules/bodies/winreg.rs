// Native Rust `winreg` module (Windows-only, issue #3262).
//
// Reference: <https://docs.python.org/3.12/library/winreg.html>

use std::cell::RefCell;
use std::ffi::OsStr;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::rc::Rc;

use num_traits::ToPrimitive;
use pyrust_derive::pyrust_module;
use windows_sys::Win32::Foundation::{
    BOOL, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME, GetLastError,
};
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_CONFIG, HKEY_CURRENT_USER, HKEY_DYN_DATA,
    HKEY_LOCAL_MACHINE, HKEY_PERFORMANCE_DATA, HKEY_USERS, KEY_ALL_ACCESS, KEY_CREATE_LINK,
    KEY_CREATE_SUB_KEY, KEY_ENUMERATE_SUB_KEYS, KEY_EXECUTE, KEY_NOTIFY, KEY_QUERY_VALUE, KEY_READ,
    KEY_SET_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, KEY_WRITE, REG_BINARY, REG_CREATED_NEW_KEY,
    REG_DWORD, REG_DWORD_BIG_ENDIAN, REG_DWORD_LITTLE_ENDIAN, REG_EXPAND_SZ,
    REG_FULL_RESOURCE_DESCRIPTOR, REG_LINK, REG_MULTI_SZ, REG_NONE, REG_NOTIFY_CHANGE_ATTRIBUTES,
    REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, REG_NOTIFY_CHANGE_SECURITY,
    REG_OPENED_EXISTING_KEY, REG_OPTION_BACKUP_RESTORE, REG_OPTION_CREATE_LINK,
    REG_OPTION_NON_VOLATILE, REG_OPTION_OPEN_LINK, REG_OPTION_RESERVED, REG_OPTION_VOLATILE,
    REG_QWORD, REG_QWORD_LITTLE_ENDIAN, REG_RESOURCE_LIST, REG_RESOURCE_REQUIREMENTS_LIST, REG_SZ,
    REG_WHOLE_HIVE_VOLATILE, RegCloseKey, RegConnectRegistryW, RegCreateKeyExW, RegDeleteKeyExW,
    RegDeleteKeyW, RegDeleteValueW, RegDisableReflectionKey, RegEnableReflectionKey, RegEnumKeyExW,
    RegEnumValueW, RegFlushKey, RegLoadKeyW, RegOpenKeyExW, RegQueryInfoKeyW,
    RegQueryReflectionKey, RegQueryValueExW, RegSaveKeyW, RegSetValueExW,
};

use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state, with_native_state_mut,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike, PyIndex, PyStr, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class};
use crate::value::{InstanceAttrs, PyBigInt, PyClass, PyInstance, Value, ValueKind};

const REG_LEGAL_OPTION: u32 = 31;
const REG_REFRESH_HIVE: u32 = 2;
const REG_NO_LAZY_FLUSH: u32 = 4;
const REG_LEGAL_CHANGE_FILTER: u32 = 268_435_471;

// ── Native PyHKEY handle state (auto-close on drop) ──────────────────────────

struct PyHkeyState {
    handle: HKEY,
}

impl Drop for PyHkeyState {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: `self.handle` is an owned registry handle opened/created
            // by `winreg` and not yet closed or detached.
            unsafe { RegCloseKey(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// Post-load wiring invoked from `builtin_modules/finalization.rs`.
///
/// CPython exports the handle type as `winreg.HKEYType` while its `__name__` is
/// `"PyHKEY"` (and `"PyHKEY"` is not a module attribute).
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let mut m = mod_rc.borrow_mut();
    if let Some(cls_val) = m.attrs.shift_remove("PyHKEY") {
        m.insert_attr("HKEYType".to_string(), cls_val);
    }
}

// ── Conversion & error helpers ───────────────────────────────────────────────

fn u64_to_py_int(v: u64) -> Value {
    if v <= i64::MAX as u64 {
        Value::int(v as i64)
    } else {
        Value::bigint(PyBigInt::from(v))
    }
}

fn hkey_const_to_py(h: HKEY) -> Value {
    u64_to_py_int(h as usize as u64)
}

fn win32_err(code: u32) -> PyError {
    PyError::from_io_error(&std::io::Error::from_raw_os_error(code as i32), None)
}

fn encode_wide_nul(s: &str) -> Result<Vec<u16>> {
    let mut wide: Vec<u16> = OsStr::new(s).encode_wide().collect();
    if wide.contains(&0) {
        return Err(PyError::named(
            "ValueError",
            "embedded null character".to_string(),
        ));
    }
    wide.push(0);
    Ok(wide)
}

fn encode_opt_wide_nul(s: Option<&str>) -> Result<Option<Vec<u16>>> {
    match s {
        Some(text) => Ok(Some(encode_wide_nul(text)?)),
        None => Ok(None),
    }
}

fn extract_hkey(interp: &mut Interpreter, val: &Value) -> Result<HKEY> {
    if has_native_state::<PyHkeyState>(val) {
        return with_native_state::<PyHkeyState, _>(val, "PyHKEY", |st| Ok(st.handle));
    }
    if val.is_none() {
        return Ok(std::ptr::null_mut());
    }
    if let Ok(idx) = PyIndex::try_from_value_with_interp(interp, val, "winreg", "key") {
        if let Some(i) = idx.as_i64() {
            return Ok(i as isize as usize as HKEY);
        }
        let bi = idx.to_bigint();
        if let Some(u) = bi.to_u64() {
            return Ok(u as usize as HKEY);
        }
        if let Some(i) = bi.to_i64() {
            return Ok(i as isize as usize as HKEY);
        }
    }
    Err(PyError::named(
        "TypeError",
        "The object is not a PyHKEY object".to_string(),
    ))
}

fn make_pyhkey(interp: &mut Interpreter, handle: HKEY) -> Result<Value> {
    let winreg_mod = interp.load_module("winreg")?;
    let cls_val = interp.get_attr(&winreg_mod, "HKEYType")?;
    let cls: Rc<RefCell<PyClass>> = match cls_val.kind() {
        ValueKind::PyClass(rc) => Rc::clone(rc),
        _ => {
            // SAFETY: avoid leaking `handle` on unexpected internal error.
            unsafe { RegCloseKey(handle) };
            return Err(PyError::runtime(
                "internal: winreg.HKEYType is not a class".to_string(),
            ));
        }
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: cls,
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(&inst, PyHkeyState { handle })?;
    Ok(inst)
}

fn decode_utf16_bytes(data: &[u8]) -> Vec<u16> {
    data.as_chunks::<2>()
        .0
        .iter()
        .map(|&chunk| u16::from_le_bytes(chunk))
        .collect()
}

fn reg_to_py(reg_type: u32, data: &[u8]) -> Value {
    match reg_type {
        REG_DWORD => {
            if data.len() == 4 {
                let n = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                Value::int(i64::from(n))
            } else {
                Value::int(0)
            }
        }
        REG_QWORD => {
            if data.len() == 8 {
                let n = u64::from_le_bytes([
                    data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
                ]);
                u64_to_py_int(n)
            } else {
                Value::int(0)
            }
        }
        REG_SZ | REG_EXPAND_SZ => {
            let mut units = decode_utf16_bytes(data);
            while units.last() == Some(&0) {
                units.pop();
            }
            let s = std::ffi::OsString::from_wide(&units)
                .to_string_lossy()
                .into_owned();
            Value::string(s)
        }
        REG_MULTI_SZ => {
            let units = decode_utf16_bytes(data);
            let mut items = Vec::new();
            let mut start = 0usize;
            while start < units.len() {
                if units[start] == 0 {
                    break;
                }
                let end = units[start..]
                    .iter()
                    .position(|&c| c == 0)
                    .map(|off| start + off)
                    .unwrap_or(units.len());
                let s = std::ffi::OsString::from_wide(&units[start..end])
                    .to_string_lossy()
                    .into_owned();
                items.push(Value::string(s));
                start = end + 1;
            }
            Value::list(items)
        }
        _ => {
            if data.is_empty() {
                Value::none()
            } else {
                Value::bytes(data.to_vec())
            }
        }
    }
}

fn py_to_reg(interp: &mut Interpreter, reg_type: u32, val: &Value) -> Result<Vec<u8>> {
    match reg_type {
        REG_DWORD => {
            if val.is_none() {
                return Ok(0u32.to_le_bytes().to_vec());
            }
            let idx = PyIndex::try_from_value_with_interp(interp, val, "SetValueEx", "value")
                .map_err(|_| {
                    PyError::named(
                        "ValueError",
                        "Could not convert the data to the specified type.".to_string(),
                    )
                })?;
            let bi = idx.to_bigint();
            let n: u32 = if let Some(u) = bi.to_u32() {
                u
            } else if let Some(i) = bi.to_i32() {
                i as u32
            } else {
                return Err(PyError::named(
                    "OverflowError",
                    "int too large to convert to C unsigned long".to_string(),
                ));
            };
            Ok(n.to_le_bytes().to_vec())
        }
        REG_QWORD => {
            if val.is_none() {
                return Ok(0u64.to_le_bytes().to_vec());
            }
            let idx = PyIndex::try_from_value_with_interp(interp, val, "SetValueEx", "value")
                .map_err(|_| {
                    PyError::named(
                        "ValueError",
                        "Could not convert the data to the specified type.".to_string(),
                    )
                })?;
            let bi = idx.to_bigint();
            let n: u64 = bi.to_u64().ok_or_else(|| {
                PyError::named(
                    "OverflowError",
                    "int too large to convert to unsigned 64-bit integer".to_string(),
                )
            })?;
            Ok(n.to_le_bytes().to_vec())
        }
        REG_SZ | REG_EXPAND_SZ => {
            if val.is_none() {
                return Ok(vec![0u8, 0u8]);
            }
            let Some(s) = val.as_str() else {
                return Err(PyError::named(
                    "ValueError",
                    "Could not convert the data to the specified type.".to_string(),
                ));
            };
            let mut units: Vec<u16> = OsStr::new(s).encode_wide().collect();
            units.push(0);
            let mut bytes = Vec::with_capacity(units.len() * 2);
            for u in units {
                bytes.extend_from_slice(&u.to_le_bytes());
            }
            Ok(bytes)
        }
        REG_MULTI_SZ => {
            if val.is_none() {
                return Ok(vec![0u8, 0u8]);
            }
            let items = interp.collect_iterable(val).map_err(|_| {
                PyError::named(
                    "ValueError",
                    "Could not convert the data to the specified type.".to_string(),
                )
            })?;
            let mut units: Vec<u16> = Vec::new();
            for item in &items {
                let Some(s) = item.as_str() else {
                    return Err(PyError::named(
                        "ValueError",
                        "Could not convert the data to the specified type.".to_string(),
                    ));
                };
                units.extend(OsStr::new(s).encode_wide());
                units.push(0);
            }
            units.push(0);
            let mut bytes = Vec::with_capacity(units.len() * 2);
            for u in units {
                bytes.extend_from_slice(&u.to_le_bytes());
            }
            Ok(bytes)
        }
        _ => {
            if val.is_none() {
                return Ok(Vec::new());
            }
            let buf = PyBytesLike::try_from_value_with_interp(interp, val, "SetValueEx", "value")
                .map_err(|_| {
                PyError::named(
                    "TypeError",
                    "Objects of type cannot be used as binary registry values".to_string(),
                )
            })?;
            Ok(buf.as_bytes().to_vec())
        }
    }
}

fn query_value_raw(hkey: HKEY, name: Option<&[u16]>) -> std::result::Result<(u32, Vec<u8>), u32> {
    let name_ptr = name.map_or(std::ptr::null(), |w| w.as_ptr());
    let mut reg_type: u32 = 0;
    let mut buf = vec![0u8; 256];
    loop {
        let mut size = buf.len() as u32;
        // SAFETY: `name_ptr` is either null or NUL-terminated UTF-16, and `buf`
        // is writable for `size` bytes.
        let rc = unsafe {
            RegQueryValueExW(
                hkey,
                name_ptr,
                std::ptr::null(),
                &mut reg_type,
                buf.as_mut_ptr(),
                &mut size,
            )
        };
        if rc == ERROR_SUCCESS {
            buf.truncate(size as usize);
            return Ok((reg_type, buf));
        }
        if rc == ERROR_MORE_DATA {
            buf.resize((size as usize).max(buf.len() * 2), 0);
            continue;
        }
        return Err(rc);
    }
}

pyrust_module! {
    constants {
        // Root HKEY constants (unsigned pointer-sized values matching CPython 64-bit)
        "HKEY_CLASSES_ROOT" => hkey_const_to_py(HKEY_CLASSES_ROOT),
        "HKEY_CURRENT_USER" => hkey_const_to_py(HKEY_CURRENT_USER),
        "HKEY_LOCAL_MACHINE" => hkey_const_to_py(HKEY_LOCAL_MACHINE),
        "HKEY_USERS" => hkey_const_to_py(HKEY_USERS),
        "HKEY_PERFORMANCE_DATA" => hkey_const_to_py(HKEY_PERFORMANCE_DATA),
        "HKEY_CURRENT_CONFIG" => hkey_const_to_py(HKEY_CURRENT_CONFIG),
        "HKEY_DYN_DATA" => hkey_const_to_py(HKEY_DYN_DATA),

        // Access rights
        "KEY_ALL_ACCESS" => Value::int(i64::from(KEY_ALL_ACCESS)),
        "KEY_WRITE" => Value::int(i64::from(KEY_WRITE)),
        "KEY_READ" => Value::int(i64::from(KEY_READ)),
        "KEY_EXECUTE" => Value::int(i64::from(KEY_EXECUTE)),
        "KEY_QUERY_VALUE" => Value::int(i64::from(KEY_QUERY_VALUE)),
        "KEY_SET_VALUE" => Value::int(i64::from(KEY_SET_VALUE)),
        "KEY_CREATE_SUB_KEY" => Value::int(i64::from(KEY_CREATE_SUB_KEY)),
        "KEY_ENUMERATE_SUB_KEYS" => Value::int(i64::from(KEY_ENUMERATE_SUB_KEYS)),
        "KEY_NOTIFY" => Value::int(i64::from(KEY_NOTIFY)),
        "KEY_CREATE_LINK" => Value::int(i64::from(KEY_CREATE_LINK)),
        "KEY_WOW64_64KEY" => Value::int(i64::from(KEY_WOW64_64KEY)),
        "KEY_WOW64_32KEY" => Value::int(i64::from(KEY_WOW64_32KEY)),

        // Value types
        "REG_NONE" => Value::int(i64::from(REG_NONE)),
        "REG_SZ" => Value::int(i64::from(REG_SZ)),
        "REG_EXPAND_SZ" => Value::int(i64::from(REG_EXPAND_SZ)),
        "REG_BINARY" => Value::int(i64::from(REG_BINARY)),
        "REG_DWORD" => Value::int(i64::from(REG_DWORD)),
        "REG_DWORD_LITTLE_ENDIAN" => Value::int(i64::from(REG_DWORD_LITTLE_ENDIAN)),
        "REG_DWORD_BIG_ENDIAN" => Value::int(i64::from(REG_DWORD_BIG_ENDIAN)),
        "REG_LINK" => Value::int(i64::from(REG_LINK)),
        "REG_MULTI_SZ" => Value::int(i64::from(REG_MULTI_SZ)),
        "REG_RESOURCE_LIST" => Value::int(i64::from(REG_RESOURCE_LIST)),
        "REG_FULL_RESOURCE_DESCRIPTOR" => Value::int(i64::from(REG_FULL_RESOURCE_DESCRIPTOR)),
        "REG_RESOURCE_REQUIREMENTS_LIST" => Value::int(i64::from(REG_RESOURCE_REQUIREMENTS_LIST)),
        "REG_QWORD" => Value::int(i64::from(REG_QWORD)),
        "REG_QWORD_LITTLE_ENDIAN" => Value::int(i64::from(REG_QWORD_LITTLE_ENDIAN)),

        // Option / disposition / notification constants
        "REG_OPTION_RESERVED" => Value::int(i64::from(REG_OPTION_RESERVED)),
        "REG_OPTION_NON_VOLATILE" => Value::int(i64::from(REG_OPTION_NON_VOLATILE)),
        "REG_OPTION_VOLATILE" => Value::int(i64::from(REG_OPTION_VOLATILE)),
        "REG_OPTION_CREATE_LINK" => Value::int(i64::from(REG_OPTION_CREATE_LINK)),
        "REG_OPTION_BACKUP_RESTORE" => Value::int(i64::from(REG_OPTION_BACKUP_RESTORE)),
        "REG_OPTION_OPEN_LINK" => Value::int(i64::from(REG_OPTION_OPEN_LINK)),
        "REG_LEGAL_OPTION" => Value::int(i64::from(REG_LEGAL_OPTION)),
        "REG_CREATED_NEW_KEY" => Value::int(i64::from(REG_CREATED_NEW_KEY)),
        "REG_OPENED_EXISTING_KEY" => Value::int(i64::from(REG_OPENED_EXISTING_KEY)),
        "REG_WHOLE_HIVE_VOLATILE" => Value::int(i64::from(REG_WHOLE_HIVE_VOLATILE)),
        "REG_REFRESH_HIVE" => Value::int(i64::from(REG_REFRESH_HIVE)),
        "REG_NO_LAZY_FLUSH" => Value::int(i64::from(REG_NO_LAZY_FLUSH)),
        "REG_NOTIFY_CHANGE_NAME" => Value::int(i64::from(REG_NOTIFY_CHANGE_NAME)),
        "REG_NOTIFY_CHANGE_ATTRIBUTES" => Value::int(i64::from(REG_NOTIFY_CHANGE_ATTRIBUTES)),
        "REG_NOTIFY_CHANGE_LAST_SET" => Value::int(i64::from(REG_NOTIFY_CHANGE_LAST_SET)),
        "REG_NOTIFY_CHANGE_SECURITY" => Value::int(i64::from(REG_NOTIFY_CHANGE_SECURITY)),
        "REG_LEGAL_CHANGE_FILTER" => Value::int(i64::from(REG_LEGAL_CHANGE_FILTER)),

        // `winreg.error` is an alias for `OSError`.
        "error" => Value::py_class(
            lookup_exc_class("OSError").expect("canonical OSError class must exist"),
        ),
    }

    #[native_state(PyHkeyState)]
    class PyHKEY {
        fn __new__(
            #[positional_only] _cls: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            Err(PyError::named(
                "TypeError",
                "cannot create 'winreg.PyHKEY' instances".to_string(),
            ))
        }

        #[getter]
        fn handle(#[state] st: &PyHkeyState) -> Result<Value> {
            Ok(u64_to_py_int(st.handle as usize as u64))
        }

        #[py_name = "Close"]
        fn close(#[state_mut] st: &mut PyHkeyState) -> Result<Value> {
            if !st.handle.is_null() {
                let h = st.handle;
                st.handle = std::ptr::null_mut();
                // SAFETY: `h` is an owned registry handle cleared before closing.
                let rc = unsafe { RegCloseKey(h) };
                if rc != ERROR_SUCCESS {
                    return Err(win32_err(rc));
                }
            }
            Ok(Value::none())
        }

        #[py_name = "Detach"]
        fn detach(#[state_mut] st: &mut PyHkeyState) -> Result<Value> {
            let h = st.handle;
            st.handle = std::ptr::null_mut();
            Ok(u64_to_py_int(h as usize as u64))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[state_mut] st: &mut PyHkeyState,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            if !st.handle.is_null() {
                let h = st.handle;
                st.handle = std::ptr::null_mut();
                // SAFETY: `h` is an owned registry handle cleared before closing.
                let rc = unsafe { RegCloseKey(h) };
                if rc != ERROR_SUCCESS {
                    return Err(win32_err(rc));
                }
            }
            Ok(Value::none())
        }

        fn __int__(#[state] st: &PyHkeyState) -> Result<Value> {
            Ok(u64_to_py_int(st.handle as usize as u64))
        }

        fn __bool__(#[state] st: &PyHkeyState) -> Result<Value> {
            Ok(Value::bool_(!st.handle.is_null()))
        }

        fn __eq__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] other: PyValueRef<'_>,
        ) -> Result<Value> {
            if !has_native_state::<PyHkeyState>(other.0) {
                return Ok(Value::not_implemented());
            }
            let same = match (self_val.0.kind(), other.0.kind()) {
                (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) => Rc::ptr_eq(a, b),
                _ => false,
            };
            Ok(Value::bool_(same))
        }

        fn __hash__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let id = match self_val.0.kind() {
                ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as usize as i64,
                _ => 0,
            };
            Ok(Value::int(id))
        }

        fn __repr__(#[state] st: &PyHkeyState) -> Result<Value> {
            Ok(Value::string(format!(
                "<winreg.PyHKEY object at 0x{:016X}>",
                st.handle as usize
            )))
        }
    }

    // ── Registry Key Operations ──────────────────────────────────────────────

    #[py_name = "OpenKey"]
    fn open_key(
        key: PyValueRef<'_>,
        sub_key: Option<PyStr<'_>>,
        #[default(0_i64.into())] reserved: PyIndex,
        #[default(131097_i64.into())] access: PyIndex,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let sub_wide = encode_opt_wide_nul(sub_key.as_deref())?;
        let reserved_u = reserved.expect_i64(FN_NAME, "reserved")? as u32;
        let access_u = access.expect_i64(FN_NAME, "access")? as u32;
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: `sub_wide` is either null or NUL-terminated UTF-16; `out` is a
        // valid output pointer wrapped immediately in an owning `PyHKEY`.
        let rc = unsafe {
            RegOpenKeyExW(
                hkey,
                sub_wide.as_ref().map_or(std::ptr::null(), Vec::as_ptr),
                reserved_u,
                access_u,
                &mut out,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        make_pyhkey(_interp, out)
    }

    #[py_name = "OpenKeyEx"]
    fn open_key_ex(
        key: PyValueRef<'_>,
        sub_key: Option<PyStr<'_>>,
        #[default(0_i64.into())] reserved: PyIndex,
        #[default(131097_i64.into())] access: PyIndex,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let sub_wide = encode_opt_wide_nul(sub_key.as_deref())?;
        let reserved_u = reserved.expect_i64(FN_NAME, "reserved")? as u32;
        let access_u = access.expect_i64(FN_NAME, "access")? as u32;
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: `sub_wide` is either null or NUL-terminated UTF-16.
        let rc = unsafe {
            RegOpenKeyExW(
                hkey,
                sub_wide.as_ref().map_or(std::ptr::null(), Vec::as_ptr),
                reserved_u,
                access_u,
                &mut out,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        make_pyhkey(_interp, out)
    }

    #[py_name = "CreateKey"]
    fn create_key(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] sub_key: Option<PyStr<'_>>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let sub_str = sub_key.as_deref().unwrap_or("");
        let sub_wide = encode_wide_nul(sub_str)?;
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: `sub_wide` is NUL-terminated UTF-16; returned handle is owned.
        let rc = unsafe {
            RegCreateKeyExW(
                hkey,
                sub_wide.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut out,
                std::ptr::null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        make_pyhkey(_interp, out)
    }

    #[py_name = "CreateKeyEx"]
    fn create_key_ex(
        key: PyValueRef<'_>,
        sub_key: Option<PyStr<'_>>,
        #[default(0_i64.into())] reserved: PyIndex,
        #[default(131078_i64.into())] access: PyIndex,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let sub_str = sub_key.as_deref().unwrap_or("");
        let sub_wide = encode_wide_nul(sub_str)?;
        let reserved_u = reserved.expect_i64(FN_NAME, "reserved")? as u32;
        let access_u = access.expect_i64(FN_NAME, "access")? as u32;
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: `sub_wide` is NUL-terminated UTF-16; returned handle is owned.
        let rc = unsafe {
            RegCreateKeyExW(
                hkey,
                sub_wide.as_ptr(),
                reserved_u,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                access_u,
                std::ptr::null(),
                &mut out,
                std::ptr::null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        make_pyhkey(_interp, out)
    }

    #[py_name = "CloseKey"]
    fn close_key(#[positional_only] hkey: PyValueRef<'_>) -> Result<Value> {
        if has_native_state::<PyHkeyState>(hkey.0) {
            return with_native_state_mut::<PyHkeyState, _>(hkey.0, "PyHKEY", |st| {
                if !st.handle.is_null() {
                    let h = st.handle;
                    st.handle = std::ptr::null_mut();
                    // SAFETY: `h` is an owned registry handle cleared before closing.
                    let rc = unsafe { RegCloseKey(h) };
                    if rc != ERROR_SUCCESS {
                        return Err(win32_err(rc));
                    }
                }
                Ok(Value::none())
            });
        }
        let raw = extract_hkey(_interp, hkey.0)?;
        if !raw.is_null() {
            // SAFETY: caller passed a detached/raw registry handle to close.
            let rc = unsafe { RegCloseKey(raw) };
            if rc != ERROR_SUCCESS {
                return Err(win32_err(rc));
            }
        }
        Ok(Value::none())
    }

    #[py_name = "DeleteKey"]
    fn delete_key(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] sub_key: PyStr<'_>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let sub_wide = encode_wide_nul(&sub_key)?;
        // SAFETY: `sub_wide` is NUL-terminated UTF-16.
        let rc = unsafe { RegDeleteKeyW(hkey, sub_wide.as_ptr()) };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "DeleteKeyEx"]
    fn delete_key_ex(
        key: PyValueRef<'_>,
        sub_key: PyStr<'_>,
        #[default(256_i64.into())] access: PyIndex,
        #[default(0_i64.into())] reserved: PyIndex,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let sub_wide = encode_wide_nul(&sub_key)?;
        let access_u = access.expect_i64(FN_NAME, "access")? as u32;
        let reserved_u = reserved.expect_i64(FN_NAME, "reserved")? as u32;
        // SAFETY: `sub_wide` is NUL-terminated UTF-16.
        let rc = unsafe { RegDeleteKeyExW(hkey, sub_wide.as_ptr(), access_u, reserved_u) };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "DeleteValue"]
    fn delete_value(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] value: Option<PyStr<'_>>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let val_wide = encode_opt_wide_nul(value.as_deref())?;
        // SAFETY: `val_wide` is either null or NUL-terminated UTF-16.
        let rc = unsafe {
            RegDeleteValueW(
                hkey,
                val_wide.as_ref().map_or(std::ptr::null(), Vec::as_ptr),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "EnumKey"]
    fn enum_key(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] index: PyIndex,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let idx = index.expect_i64(FN_NAME, "index")?;
        if idx < 0 || idx > u32::MAX as i64 {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C unsigned long".to_string(),
            ));
        }
        // Registry key names are at most 255 UTF-16 characters plus NUL.
        let mut buf = vec![0u16; 257];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` is writable for `len` UTF-16 units.
        let rc = unsafe {
            RegEnumKeyExW(
                hkey,
                idx as u32,
                buf.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        let s = std::ffi::OsString::from_wide(&buf[..len as usize])
            .to_string_lossy()
            .into_owned();
        Ok(Value::string(s))
    }

    #[py_name = "EnumValue"]
    fn enum_value(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] index: PyIndex,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let idx = index.expect_i64(FN_NAME, "index")?;
        if idx < 0 || idx > u32::MAX as i64 {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C unsigned long".to_string(),
            ));
        }
        let mut name_buf = vec![0u16; 256];
        let mut data_buf = vec![0u8; 256];
        loop {
            let mut name_len = name_buf.len() as u32;
            let mut data_len = data_buf.len() as u32;
            let mut reg_type: u32 = 0;
            // SAFETY: `name_buf` and `data_buf` are valid for their reported lengths.
            let rc = unsafe {
                RegEnumValueW(
                    hkey,
                    idx as u32,
                    name_buf.as_mut_ptr(),
                    &mut name_len,
                    std::ptr::null(),
                    &mut reg_type,
                    data_buf.as_mut_ptr(),
                    &mut data_len,
                )
            };
            if rc == ERROR_SUCCESS {
                let name = std::ffi::OsString::from_wide(&name_buf[..name_len as usize])
                    .to_string_lossy()
                    .into_owned();
                data_buf.truncate(data_len as usize);
                let py_val = reg_to_py(reg_type, &data_buf);
                return Ok(Value::tuple(vec![
                    Value::string(name),
                    py_val,
                    Value::int(i64::from(reg_type)),
                ]));
            }
            if rc == ERROR_MORE_DATA {
                if (data_len as usize) > data_buf.len() {
                    data_buf.resize(data_len as usize, 0);
                } else {
                    name_buf.resize(name_buf.len() * 2, 0);
                    data_buf.resize(data_buf.len() * 2, 0);
                }
                continue;
            }
            return Err(win32_err(rc));
        }
    }

    #[py_name = "QueryInfoKey"]
    fn query_info_key(#[positional_only] key: PyValueRef<'_>) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let mut sub_keys: u32 = 0;
        let mut values: u32 = 0;
        let mut ft = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        // SAFETY: all output pointers are valid stack locals.
        let rc = unsafe {
            RegQueryInfoKeyW(
                hkey,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                &mut sub_keys,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut values,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut ft,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        let ticks = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
        Ok(Value::tuple(vec![
            Value::int(i64::from(sub_keys)),
            Value::int(i64::from(values)),
            u64_to_py_int(ticks),
        ]))
    }

    #[py_name = "QueryValue"]
    fn query_value(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] sub_key: Option<PyStr<'_>>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let mut owned_child: HKEY = std::ptr::null_mut();
        let target = match sub_key.as_deref() {
            Some(s) if !s.is_empty() => {
                let sub_wide = encode_wide_nul(s)?;
                // SAFETY: `sub_wide` is NUL-terminated UTF-16; `owned_child` is
                // closed before returning.
                let rc = unsafe {
                    RegOpenKeyExW(hkey, sub_wide.as_ptr(), 0, KEY_QUERY_VALUE, &mut owned_child)
                };
                if rc != ERROR_SUCCESS {
                    return Err(win32_err(rc));
                }
                owned_child
            }
            _ => hkey,
        };

        let result = query_value_raw(target, None);
        if !owned_child.is_null() {
            // SAFETY: `owned_child` was opened above and is no longer needed.
            unsafe { RegCloseKey(owned_child) };
        }
        match result {
            Ok((_typ, data)) => {
                let mut units = decode_utf16_bytes(&data);
                while units.last() == Some(&0) {
                    units.pop();
                }
                let s = std::ffi::OsString::from_wide(&units)
                    .to_string_lossy()
                    .into_owned();
                Ok(Value::string(s))
            }
            Err(ERROR_FILE_NOT_FOUND) => Ok(Value::string("")),
            Err(code) => Err(win32_err(code)),
        }
    }

    #[py_name = "QueryValueEx"]
    fn query_value_ex(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] name: Option<PyStr<'_>>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let name_wide = encode_opt_wide_nul(name.as_deref())?;
        let (reg_type, data) = query_value_raw(hkey, name_wide.as_deref()).map_err(win32_err)?;
        let py_val = reg_to_py(reg_type, &data);
        Ok(Value::tuple(vec![py_val, Value::int(i64::from(reg_type))]))
    }

    #[py_name = "SetValue"]
    fn set_value(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] sub_key: Option<PyStr<'_>>,
        #[positional_only] r#type: PyIndex,
        #[positional_only] value: PyStr<'_>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let typ = r#type.expect_i64(FN_NAME, "type")? as u32;
        if typ != REG_SZ {
            return Err(PyError::named(
                "TypeError",
                "Type must be winreg.REG_SZ".to_string(),
            ));
        }
        let mut units: Vec<u16> = OsStr::new(&*value).encode_wide().collect();
        units.push(0);
        let mut bytes = Vec::with_capacity(units.len() * 2);
        for u in units {
            bytes.extend_from_slice(&u.to_le_bytes());
        }

        let mut owned_child: HKEY = std::ptr::null_mut();
        let target = match sub_key.as_deref() {
            Some(s) if !s.is_empty() => {
                let sub_wide = encode_wide_nul(s)?;
                // SAFETY: `sub_wide` is NUL-terminated UTF-16; `owned_child` is
                // closed before returning.
                let rc = unsafe {
                    RegCreateKeyExW(
                        hkey,
                        sub_wide.as_ptr(),
                        0,
                        std::ptr::null(),
                        REG_OPTION_NON_VOLATILE,
                        KEY_SET_VALUE,
                        std::ptr::null(),
                        &mut owned_child,
                        std::ptr::null_mut(),
                    )
                };
                if rc != ERROR_SUCCESS {
                    return Err(win32_err(rc));
                }
                owned_child
            }
            _ => hkey,
        };

        // SAFETY: `bytes` is valid for `bytes.len()` bytes.
        let rc = unsafe {
            RegSetValueExW(
                target,
                std::ptr::null(),
                0,
                REG_SZ,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        if !owned_child.is_null() {
            // SAFETY: `owned_child` was opened above.
            unsafe { RegCloseKey(owned_child) };
        }
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "SetValueEx"]
    fn set_value_ex(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] value_name: Option<PyStr<'_>>,
        #[positional_only] reserved: PyValueRef<'_>,
        #[positional_only] r#type: PyIndex,
        #[positional_only] value: PyValueRef<'_>,
    ) -> Result<Value> {
        let _ = reserved;
        let hkey = extract_hkey(_interp, key.0)?;
        let name_wide = encode_opt_wide_nul(value_name.as_deref())?;
        let typ = r#type.expect_i64(FN_NAME, "type")? as u32;
        let data = py_to_reg(_interp, typ, value.0)?;
        let data_ptr = if data.is_empty() {
            std::ptr::null()
        } else {
            data.as_ptr()
        };
        // SAFETY: `name_wide` is null or NUL-terminated UTF-16; `data_ptr` is
        // valid for `data.len()` bytes.
        let rc = unsafe {
            RegSetValueExW(
                hkey,
                name_wide.as_ref().map_or(std::ptr::null(), Vec::as_ptr),
                0,
                typ,
                data_ptr,
                data.len() as u32,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "ConnectRegistry"]
    fn connect_registry(
        #[positional_only] computer_name: Option<PyStr<'_>>,
        #[positional_only] key: PyValueRef<'_>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let comp_wide = encode_opt_wide_nul(computer_name.as_deref())?;
        let mut out: HKEY = std::ptr::null_mut();
        // SAFETY: `comp_wide` is null or NUL-terminated UTF-16.
        let rc = unsafe {
            RegConnectRegistryW(
                comp_wide.as_ref().map_or(std::ptr::null(), Vec::as_ptr),
                hkey,
                &mut out,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        make_pyhkey(_interp, out)
    }

    #[py_name = "FlushKey"]
    fn flush_key(#[positional_only] key: PyValueRef<'_>) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        // SAFETY: `RegFlushKey` validates `hkey`.
        let rc = unsafe { RegFlushKey(hkey) };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "ExpandEnvironmentStrings"]
    fn expand_environment_strings(#[positional_only] string: PyStr<'_>) -> Result<Value> {
        let src = encode_wide_nul(&string)?;
        // SAFETY: first call with null dst returns required buffer size in UTF-16 units including NUL.
        let needed = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), std::ptr::null_mut(), 0) };
        if needed == 0 {
            // SAFETY: `GetLastError` reads the error set by the failed call above.
            return Err(win32_err(unsafe { GetLastError() }));
        }
        let mut dst = vec![0u16; needed as usize];
        // SAFETY: `dst` has length `needed` UTF-16 code units.
        let written = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), dst.as_mut_ptr(), needed) };
        if written == 0 {
            return Err(win32_err(unsafe { GetLastError() }));
        }
        let end = dst.iter().position(|&c| c == 0).unwrap_or(written as usize);
        let out = std::ffi::OsString::from_wide(&dst[..end])
            .to_string_lossy()
            .into_owned();
        Ok(Value::string(out))
    }

    #[py_name = "DisableReflectionKey"]
    fn disable_reflection_key(#[positional_only] key: PyValueRef<'_>) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        // SAFETY: `RegDisableReflectionKey` validates `hkey`.
        let rc = unsafe { RegDisableReflectionKey(hkey) };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "EnableReflectionKey"]
    fn enable_reflection_key(#[positional_only] key: PyValueRef<'_>) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        // SAFETY: `RegEnableReflectionKey` validates `hkey`.
        let rc = unsafe { RegEnableReflectionKey(hkey) };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "QueryReflectionKey"]
    fn query_reflection_key(#[positional_only] key: PyValueRef<'_>) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let mut disabled: BOOL = 0;
        // SAFETY: `disabled` is a valid out pointer.
        let rc = unsafe { RegQueryReflectionKey(hkey, &mut disabled) };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::bool_(disabled != 0))
    }

    #[py_name = "SaveKey"]
    fn save_key(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] file_name: PyStr<'_>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let file_wide = encode_wide_nul(&file_name)?;
        // SAFETY: `file_wide` is NUL-terminated UTF-16.
        let rc = unsafe { RegSaveKeyW(hkey, file_wide.as_ptr(), std::ptr::null()) };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }

    #[py_name = "LoadKey"]
    fn load_key(
        #[positional_only] key: PyValueRef<'_>,
        #[positional_only] sub_key: PyStr<'_>,
        #[positional_only] file_name: PyStr<'_>,
    ) -> Result<Value> {
        let hkey = extract_hkey(_interp, key.0)?;
        let sub_wide = encode_wide_nul(&sub_key)?;
        let file_wide = encode_wide_nul(&file_name)?;
        // SAFETY: both wide strings are NUL-terminated UTF-16.
        let rc = unsafe { RegLoadKeyW(hkey, sub_wide.as_ptr(), file_wide.as_ptr()) };
        if rc != ERROR_SUCCESS {
            return Err(win32_err(rc));
        }
        Ok(Value::none())
    }
}
