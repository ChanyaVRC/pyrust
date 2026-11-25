//! Construction and presentation of `os` result objects (`terminal_size`,
//! `stat_result`, `times_result`).
//!
//! Host operations pass metadata or scalar fields into this module. Python
//! class identity, instance attributes, and repr formatting stay here.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyKey, Value, ValueKind};

use super::arguments::require_self;
use super::{
    STAT_RESULT_NEW_REGISTRY, STAT_RESULT_REPR_REGISTRY, TERMINAL_SIZE_NEW_REGISTRY,
    TERMINAL_SIZE_REPR_REGISTRY,
};

pyrust_struct_seq! {
    pub(crate) times_result in "os" {
        fields: [user, system, children_user, children_system, elapsed],
    }
}

// Thread-local justification:
// (a) Why `thread_local!`: Holds `!Send` `Rc<RefCell<PyClass>>` singletons for
//     `os.terminal_size` and `os.stat_result` structseq classes.
// (b) Multi-interpreter / multi-threaded impact: Multiple interpreters on one
//     thread share the immutable `os` result-type class singletons; separate
//     threads build independent singletons.
thread_local! {
    static TERMINAL_SIZE_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs = indexmap::IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("os"));
        attrs.insert(
            "__new__".to_string(),
            Value::builtin_function(TERMINAL_SIZE_NEW_REGISTRY),
        );
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function(TERMINAL_SIZE_REPR_REGISTRY),
        );
        attrs.insert("n_fields".to_string(), Value::int(2));
        attrs.insert("n_sequence_fields".to_string(), Value::int(2));
        attrs.insert("n_unnamed_fields".to_string(), Value::int(0));
        attrs.insert(
            "__match_args__".to_string(),
            Value::tuple(vec![Value::string("columns"), Value::string("lines")]),
        );
        let mut class = PyClass::new(
            "terminal_size",
            "terminal_size",
            Some(tuple_class()),
            attrs,
        );
        class.non_subclassable_name = Some("os.terminal_size");
        Rc::new(RefCell::new(class))
    };

    static STAT_RESULT_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs = indexmap::IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("os"));
        attrs.insert(
            "__new__".to_string(),
            Value::builtin_function(STAT_RESULT_NEW_REGISTRY),
        );
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function(STAT_RESULT_REPR_REGISTRY),
        );
        attrs.insert("n_fields".to_string(), Value::int(19));
        attrs.insert("n_sequence_fields".to_string(), Value::int(10));
        attrs.insert("n_unnamed_fields".to_string(), Value::int(3));
        attrs.insert(
            "__match_args__".to_string(),
            Value::tuple(
                [
                    "st_mode", "st_ino", "st_dev", "st_nlink", "st_uid", "st_gid",
                    "st_size",
                ]
                .into_iter()
                .map(Value::string)
                .collect(),
            ),
        );
        let mut class = PyClass::new(
            "stat_result",
            "stat_result",
            Some(tuple_class()),
            attrs,
        );
        class.non_subclassable_name = Some("os.stat_result");
        Rc::new(RefCell::new(class))
    };
}

fn tuple_class() -> Rc<RefCell<PyClass>> {
    crate::interpreter::primitive_class_by_name("tuple")
        .expect("the canonical tuple class must exist before os result classes")
}

pub(super) fn terminal_size_class_value() -> Value {
    TERMINAL_SIZE_CLASS.with(|class| Value::py_class(Rc::clone(class)))
}

pub(super) fn stat_result_class_value() -> Value {
    STAT_RESULT_CLASS.with(|class| Value::py_class(Rc::clone(class)))
}

pub(super) fn times_result_class_value() -> Value {
    times_result::class()
}

pub(super) fn make_times_result(
    user: f64,
    system: f64,
    children_user: f64,
    children_system: f64,
    elapsed: f64,
) -> Value {
    times_result::make([
        Value::float(user),
        Value::float(system),
        Value::float(children_user),
        Value::float(children_system),
        Value::float(elapsed),
    ])
}

fn make_result_instance(
    class: Rc<RefCell<PyClass>>,
    sequence: Vec<Value>,
    named_attrs: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.set_builtin_data(Value::tuple(sequence));
    for (name, value) in named_attrs {
        attrs.insert(name, value);
    }
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

fn struct_sequence_args(
    args: &[ExpandedCallArg],
    type_name: &str,
) -> Result<(Rc<RefCell<PyClass>>, Value, Option<Value>)> {
    let class = match args.first().map(|arg| arg.value.kind()) {
        Some(ValueKind::PyClass(class)) => Rc::clone(class),
        _ => {
            return Err(PyError::runtime(format!(
                "internal: {type_name}.__new__ first argument must be a class"
            )));
        }
    };
    let supplied = &args[1..];
    let positional: Vec<&ExpandedCallArg> =
        supplied.iter().filter(|arg| arg.name.is_none()).collect();
    if positional.len() > 2 {
        return Err(pyrust_core::type_err!(
            "os.{type_name}() takes a dict as second arg, if any"
        ));
    }

    let mut sequence = positional.first().map(|arg| arg.value.clone());
    let mut overrides = positional.get(1).map(|arg| arg.value.clone());
    let mut invalid_keyword = None;
    for arg in supplied.iter().filter(|arg| arg.name.is_some()) {
        match arg.name.as_deref().unwrap_or_default() {
            "sequence" => {
                if sequence.is_some() {
                    return Err(pyrust_core::type_err!(
                        "argument for structseq() given by name ('sequence') and position (1)"
                    ));
                }
                sequence = Some(arg.value.clone());
            }
            "dict" => {
                if overrides.is_some() {
                    return Err(pyrust_core::type_err!(
                        "argument for structseq() given by name ('dict') and position (2)"
                    ));
                }
                overrides = Some(arg.value.clone());
            }
            other => {
                invalid_keyword = Some(other.to_string());
            }
        }
    }
    let sequence = sequence.ok_or_else(|| {
        pyrust_core::type_err!("structseq() missing required argument 'sequence' (pos 1)")
    })?;
    if let Some(other) = invalid_keyword {
        return Err(pyrust_core::type_err!(
            "'{other}' is an invalid keyword argument for structseq()"
        ));
    }
    if overrides
        .as_ref()
        .is_some_and(|value| !matches!(value.kind(), ValueKind::Dict(_)))
    {
        return Err(pyrust_core::type_err!(
            "os.{type_name}() takes a dict as second arg, if any"
        ));
    }
    Ok((class, sequence, overrides))
}

fn dict_value(overrides: Option<&Value>, name: &str) -> Option<Value> {
    let ValueKind::Dict(dict) = overrides?.kind() else {
        return None;
    };
    dict.get(&PyKey::str_from(name)).cloned()
}

pub(super) fn terminal_size_new(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    _fn_name: &str,
) -> Result<Value> {
    let (class, sequence, _overrides) = struct_sequence_args(args, "terminal_size")?;
    let values = interp.collect_iterable(&sequence)?;
    if values.len() != 2 {
        return Err(pyrust_core::type_err!(
            "os.terminal_size() takes a 2-sequence ({}-sequence given)",
            values.len()
        ));
    }
    Ok(make_result_instance(
        class,
        values.clone(),
        [("columns", values[0].clone()), ("lines", values[1].clone())],
    ))
}

pub(super) fn stat_result_new(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    _fn_name: &str,
) -> Result<Value> {
    let (class, sequence, overrides) = struct_sequence_args(args, "stat_result")?;
    let values = interp.collect_iterable(&sequence)?;
    if values.len() < 10 {
        return Err(pyrust_core::type_err!(
            "os.stat_result() takes an at least 10-sequence ({}-sequence given)",
            values.len()
        ));
    }
    if values.len() > 19 {
        return Err(pyrust_core::type_err!(
            "os.stat_result() takes an at most 19-sequence ({}-sequence given)",
            values.len()
        ));
    }

    let overrides = overrides.as_ref();
    let supplemental = |name: &str, index: usize, fallback: Value| {
        dict_value(overrides, name)
            .or_else(|| values.get(index).cloned())
            .unwrap_or(fallback)
    };
    let atime = supplemental("st_atime", 10, values[7].clone());
    let mtime = supplemental("st_mtime", 11, values[8].clone());
    let ctime = supplemental("st_ctime", 12, values[9].clone());
    let optional = |name: &str, index: usize| {
        dict_value(overrides, name)
            .or_else(|| values.get(index).cloned())
            .unwrap_or_else(Value::none)
    };
    let sequence_values = values[..10].to_vec();
    Ok(make_result_instance(
        class,
        sequence_values,
        [
            ("st_mode", values[0].clone()),
            ("st_ino", values[1].clone()),
            ("st_dev", values[2].clone()),
            ("st_nlink", values[3].clone()),
            ("st_uid", values[4].clone()),
            ("st_gid", values[5].clone()),
            ("st_size", values[6].clone()),
            ("st_atime", atime),
            ("st_mtime", mtime),
            ("st_ctime", ctime),
            ("st_atime_ns", optional("st_atime_ns", 13)),
            ("st_mtime_ns", optional("st_mtime_ns", 14)),
            ("st_ctime_ns", optional("st_ctime_ns", 15)),
            ("st_blksize", optional("st_blksize", 16)),
            ("st_blocks", optional("st_blocks", 17)),
            ("st_rdev", optional("st_rdev", 18)),
        ],
    ))
}

pub(super) fn terminal_size_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    let instance = match args[0].value.kind() {
        ValueKind::PyInstance(instance) => Rc::clone(instance),
        _ => {
            return Err(PyError::runtime(
                "terminal_size_repr() expected a terminal_size instance".to_string(),
            ));
        }
    };
    let instance = instance.borrow();
    let attr_repr = |name: &str| match instance.attrs.get(name) {
        Some(value) => value.repr_raw(),
        None => "None".to_string(),
    };
    Ok(Value::string(format!(
        "os.terminal_size(columns={}, lines={})",
        attr_repr("columns"),
        attr_repr("lines"),
    )))
}

pub(super) fn stat_result_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    let instance = match args[0].value.kind() {
        ValueKind::PyInstance(instance) => Rc::clone(instance),
        _ => {
            return Err(PyError::runtime(
                "stat_result_repr() expected a stat_result instance".to_string(),
            ));
        }
    };
    let instance = instance.borrow();
    let sequence = instance
        .attrs
        .builtin_data()
        .and_then(|value| match value.kind() {
            ValueKind::Tuple(values) => Some(values),
            _ => None,
        });
    let repr = |name: &str, index: usize| {
        sequence
            .and_then(|values| values.get(index))
            .or_else(|| instance.attrs.get(name))
            .map_or_else(|| "None".to_string(), Value::repr_raw)
    };
    Ok(Value::string(format!(
        "os.stat_result(st_mode={}, st_ino={}, st_dev={}, st_nlink={}, \
         st_uid={}, st_gid={}, st_size={}, st_atime={}, st_mtime={}, st_ctime={})",
        repr("st_mode", 0),
        repr("st_ino", 1),
        repr("st_dev", 2),
        repr("st_nlink", 3),
        repr("st_uid", 4),
        repr("st_gid", 5),
        repr("st_size", 6),
        repr("st_atime", 7),
        repr("st_mtime", 8),
        repr("st_ctime", 9),
    )))
}

pub(super) fn make_terminal_size(columns: i64, lines: i64) -> Value {
    TERMINAL_SIZE_CLASS.with(|class| {
        let columns = Value::int(columns);
        let lines = Value::int(lines);
        make_result_instance(
            Rc::clone(class),
            vec![columns.clone(), lines.clone()],
            [("columns", columns), ("lines", lines)],
        )
    })
}

fn seconds_and_nanos(result: std::io::Result<std::time::SystemTime>) -> (f64, i64) {
    result
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| (duration.as_secs_f64(), duration.as_nanos() as i64))
        .unwrap_or((0.0, 0))
}

/// Query Windows `(st_ino, st_dev, st_nlink)` from an open OS `HANDLE`.
#[cfg(windows)]
pub(super) fn win_handle_file_ids(
    raw_handle: windows_sys::Win32::Foundation::HANDLE,
) -> Option<(u64, u32, u32)> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    // SAFETY: `info` is zero-initialized and `raw_handle` is a borrowed Win32 handle.
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(raw_handle, &mut info) != 0 {
            let ino = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);
            return Some((ino, info.dwVolumeSerialNumber, info.nNumberOfLinks));
        }
    }
    None
}

/// Query Windows `(st_ino, st_dev, st_nlink)` for a filesystem path without
/// requiring read/write permissions (`dwDesiredAccess = 0`).
#[cfg(windows)]
pub(super) fn win_path_file_ids(path: &Path, follow_symlinks: bool) -> Option<(u64, u32, u32)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut flags = FILE_FLAG_BACKUP_SEMANTICS;
    if !follow_symlinks {
        flags |= FILE_FLAG_OPEN_REPARSE_POINT;
    }
    // SAFETY: `wide` is NUL-terminated; any valid handle is closed before returning.
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        );
        if handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let ids = win_handle_file_ids(handle);
        CloseHandle(handle);
        ids
    }
}

fn build_stat_result_with_ids(
    metadata: &std::fs::Metadata,
    override_ids: Option<(u64, u32, u32)>,
) -> Value {
    let (modified, modified_ns) = seconds_and_nanos(metadata.modified());
    let (accessed, accessed_ns) = seconds_and_nanos(metadata.accessed());

    #[cfg(unix)]
    let (mode, inode_val, links, user, group, device, ctime_f, ctime_ns_val) = {
        use std::os::unix::fs::MetadataExt;
        let _ = override_ids;
        let ct_ns = metadata.ctime() * 1_000_000_000 + metadata.ctime_nsec();
        let ct_f = metadata.ctime() as f64 + (metadata.ctime_nsec() as f64) / 1_000_000_000.0;
        (
            metadata.mode() as i64,
            Value::int(metadata.ino() as i64),
            metadata.nlink() as i64,
            metadata.uid() as i64,
            metadata.gid() as i64,
            metadata.dev() as i64,
            ct_f,
            ct_ns,
        )
    };
    #[cfg(not(unix))]
    let (mode, inode_val, links, user, group, device, ctime_f, ctime_ns_val) = {
        let (created, created_ns) = seconds_and_nanos(metadata.created());
        let mode = if metadata.file_type().is_symlink() {
            0o120777_i64
        } else if metadata.is_dir() {
            0o040777_i64
        } else if metadata.permissions().readonly() {
            0o100444_i64
        } else {
            0o100666_i64
        };
        let (ino, dev, nlink) = override_ids.unwrap_or((0, 0, 1));
        let ino_val = if ino <= i64::MAX as u64 {
            Value::int(ino as i64)
        } else {
            Value::bigint(crate::value::PyBigInt::from(ino))
        };
        (
            mode,
            ino_val,
            nlink as i64,
            0_i64,
            0_i64,
            dev as i64,
            created,
            created_ns,
        )
    };

    STAT_RESULT_CLASS.with(|class| {
        let visible = vec![
            Value::int(mode),
            inode_val,
            Value::int(device),
            Value::int(links),
            Value::int(user),
            Value::int(group),
            Value::int(metadata.len() as i64),
            Value::float(accessed),
            Value::float(modified),
            Value::float(ctime_f),
        ];
        make_result_instance(
            Rc::clone(class),
            visible.clone(),
            [
                ("st_mode", visible[0].clone()),
                ("st_ino", visible[1].clone()),
                ("st_dev", visible[2].clone()),
                ("st_nlink", visible[3].clone()),
                ("st_uid", visible[4].clone()),
                ("st_gid", visible[5].clone()),
                ("st_size", visible[6].clone()),
                ("st_atime", visible[7].clone()),
                ("st_mtime", visible[8].clone()),
                ("st_ctime", visible[9].clone()),
                ("st_atime_ns", Value::int(accessed_ns)),
                ("st_mtime_ns", Value::int(modified_ns)),
                ("st_ctime_ns", Value::int(ctime_ns_val)),
                ("st_blksize", Value::none()),
                ("st_blocks", Value::none()),
                ("st_rdev", Value::none()),
            ],
        )
    })
}

pub(crate) fn make_stat_result(metadata: &std::fs::Metadata) -> Value {
    build_stat_result_with_ids(metadata, None)
}

pub(super) fn make_stat_result_for_path(
    path: &Path,
    metadata: &std::fs::Metadata,
    follow_symlinks: bool,
) -> Value {
    #[cfg(windows)]
    let ids = win_path_file_ids(path, follow_symlinks);
    #[cfg(not(windows))]
    let ids = {
        let _ = (path, follow_symlinks);
        None
    };
    build_stat_result_with_ids(metadata, ids)
}

#[cfg(windows)]
pub(super) fn make_stat_result_for_win_handle(
    raw_handle: windows_sys::Win32::Foundation::HANDLE,
    metadata: &std::fs::Metadata,
) -> Value {
    let ids = win_handle_file_ids(raw_handle);
    build_stat_result_with_ids(metadata, ids)
}

/// Build an `os.stat_result` directly from a Windows `HANDLE` via
/// `GetFileInformationByHandle` (used when we have a borrowed handle from
/// `open(path).fileno()` or stdio).
#[cfg(windows)]
pub(super) fn stat_result_from_borrowed_win_handle(
    raw_handle: windows_sys::Win32::Foundation::HANDLE,
) -> std::io::Result<Value> {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY,
        FILE_ATTRIBUTE_REPARSE_POINT, GetFileInformationByHandle,
    };
    fn filetime_to_unix(ft: windows_sys::Win32::Foundation::FILETIME) -> (f64, i64) {
        const EPOCH_DIFF_100NS: u64 = 116_444_736_000_000_000;
        let ticks = ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64);
        if ticks < EPOCH_DIFF_100NS {
            return (0.0, 0);
        }
        let rel = ticks - EPOCH_DIFF_100NS;
        let ns = (rel.saturating_mul(100)) as i64;
        let secs = (rel as f64) / 10_000_000.0;
        (secs, ns)
    }

    // SAFETY: `info` is zero-initialized and `raw_handle` is a live borrowed handle.
    let info = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(raw_handle, &mut info) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        info
    };

    let attrs = info.dwFileAttributes;
    let mode = if (attrs & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        0o120777_i64
    } else if (attrs & FILE_ATTRIBUTE_DIRECTORY) != 0 {
        0o040777_i64
    } else if (attrs & FILE_ATTRIBUTE_READONLY) != 0 {
        0o100444_i64
    } else {
        0o100666_i64
    };
    let ino = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);
    let ino_val = if ino <= i64::MAX as u64 {
        Value::int(ino as i64)
    } else {
        Value::bigint(crate::value::PyBigInt::from(ino))
    };
    let size = (((info.nFileSizeHigh as u64) << 32) | (info.nFileSizeLow as u64)) as i64;
    let (atime, atime_ns) = filetime_to_unix(info.ftLastAccessTime);
    let (mtime, mtime_ns) = filetime_to_unix(info.ftLastWriteTime);
    let (ctime, ctime_ns) = filetime_to_unix(info.ftCreationTime);

    Ok(STAT_RESULT_CLASS.with(|class| {
        let visible = vec![
            Value::int(mode),
            ino_val,
            Value::int(info.dwVolumeSerialNumber as i64),
            Value::int(info.nNumberOfLinks as i64),
            Value::int(0),
            Value::int(0),
            Value::int(size),
            Value::float(atime),
            Value::float(mtime),
            Value::float(ctime),
        ];
        make_result_instance(
            Rc::clone(class),
            visible.clone(),
            [
                ("st_mode", visible[0].clone()),
                ("st_ino", visible[1].clone()),
                ("st_dev", visible[2].clone()),
                ("st_nlink", visible[3].clone()),
                ("st_uid", visible[4].clone()),
                ("st_gid", visible[5].clone()),
                ("st_size", visible[6].clone()),
                ("st_atime", visible[7].clone()),
                ("st_mtime", visible[8].clone()),
                ("st_ctime", visible[9].clone()),
                ("st_atime_ns", Value::int(atime_ns)),
                ("st_mtime_ns", Value::int(mtime_ns)),
                ("st_ctime_ns", Value::int(ctime_ns)),
                ("st_blksize", Value::none()),
                ("st_blocks", Value::none()),
                ("st_rdev", Value::none()),
            ],
        )
    }))
}
