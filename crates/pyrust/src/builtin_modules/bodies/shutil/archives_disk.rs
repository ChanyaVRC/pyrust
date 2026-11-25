//! Native Rust implementations of `shutil` archive and disk APIs (issue #3251):
//! - `usage` structseq & `disk_usage(path)`
//! - `chown(path, user=None, group=None)` (`#[cfg(unix)]`)
//! - `get_archive_formats`, `register_archive_format`, `unregister_archive_format`
//! - `get_unpack_formats`, `register_unpack_format`, `unregister_unpack_format`
//! - `make_archive`, `unpack_archive` (wiring `zipfile` and `tarfile`)

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use num_bigint::BigInt;

use crate::error::{PyError, Result};
#[cfg(unix)]
use crate::interpreter::value_type_name_str;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

pyrust_struct_seq! {
    pub(crate) usage in "shutil" {
        fields: [total, used, free],
    }
}

pub(super) fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

pub(super) fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

pub(super) fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

pub(super) fn call_method_kw(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_kw(interp, f, pos, kw)
}

fn u128_to_py_int(v: u128) -> Value {
    if v <= i64::MAX as u128 {
        Value::int(v as i64)
    } else {
        Value::bigint(BigInt::from(v))
    }
}

pub(super) fn disk_usage_impl(path: &Path) -> Result<Value> {
    let disp = path.to_string_lossy().into_owned();
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| PyError::named("ValueError", "embedded null byte in path".to_string()))?;
        // SAFETY: `st` is zero-initialized POD and `c_path` is NUL-terminated.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut st) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&err, Some(&disp)));
        }
        let frsize = if st.f_frsize != 0 {
            st.f_frsize as u128
        } else {
            st.f_bsize as u128
        };
        let free = (st.f_bavail as u128).saturating_mul(frsize);
        let total = (st.f_blocks as u128).saturating_mul(frsize);
        let used = (st.f_blocks.saturating_sub(st.f_bfree) as u128).saturating_mul(frsize);
        Ok(usage::make([
            u128_to_py_int(total),
            u128_to_py_int(used),
            u128_to_py_int(free),
        ]))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

        let meta = std::fs::metadata(path).map_err(|e| PyError::from_io_error(&e, Some(&disp)))?;
        let dir_path: PathBuf = if meta.is_file() {
            match path.parent() {
                Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => PathBuf::from("."),
            }
        } else {
            path.to_path_buf()
        };
        let wide: Vec<u16> = dir_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_avail: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;
        // SAFETY: `wide` is NUL-terminated UTF-16 and output pointers are valid aligned `u64`s.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_avail,
                &mut total_bytes,
                &mut total_free,
            )
        };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&err, Some(&disp)));
        }
        let total = total_bytes as u128;
        let free = free_avail as u128;
        let used = total.saturating_sub(free);
        Ok(usage::make([
            u128_to_py_int(total),
            u128_to_py_int(used),
            u128_to_py_int(free),
        ]))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = disp;
        Err(PyError::named(
            "NotImplementedError",
            "disk_usage is not supported on this platform".to_string(),
        ))
    }
}

#[cfg(unix)]
pub(super) fn chown_impl(
    path: &Path,
    user: Option<&Value>,
    group: Option<&Value>,
) -> Result<Value> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let user = user.filter(|v| !v.is_none());
    let group = group.filter(|v| !v.is_none());
    if user.is_none() && group.is_none() {
        return Err(PyError::named(
            "ValueError",
            "user and/or group must be set".to_string(),
        ));
    }

    let uid: libc::uid_t = match user {
        None => (-1i32) as libc::uid_t,
        Some(v) => match v.kind() {
            ValueKind::Int(n) => n as libc::uid_t,
            ValueKind::Bool(b) => (b as i64) as libc::uid_t,
            ValueKind::Str(s) => {
                let c_name = CString::new(s)
                    .map_err(|_| PyError::named("LookupError", format!("no such user: {s:?}")))?;
                // SAFETY: `c_name` is a valid NUL-terminated C string.
                let pw = unsafe { libc::getpwnam(c_name.as_ptr()) };
                if pw.is_null() {
                    return Err(PyError::named(
                        "LookupError",
                        format!("no such user: '{s}'"),
                    ));
                }
                // SAFETY: `pw` is non-null from `getpwnam`.
                unsafe { (*pw).pw_uid }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "user must be str, int, or None, not {}",
                        value_type_name_str(v)
                    ),
                ));
            }
        },
    };

    let gid: libc::gid_t = match group {
        None => (-1i32) as libc::gid_t,
        Some(v) => match v.kind() {
            ValueKind::Int(n) => n as libc::gid_t,
            ValueKind::Bool(b) => (b as i64) as libc::gid_t,
            ValueKind::Str(s) => {
                let c_name = CString::new(s)
                    .map_err(|_| PyError::named("LookupError", format!("no such group: {s:?}")))?;
                // SAFETY: `c_name` is a valid NUL-terminated C string.
                let gr = unsafe { libc::getgrnam(c_name.as_ptr()) };
                if gr.is_null() {
                    return Err(PyError::named(
                        "LookupError",
                        format!("no such group: '{s}'"),
                    ));
                }
                // SAFETY: `gr` is non-null from `getgrnam`.
                unsafe { (*gr).gr_gid }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "group must be str, int, or None, not {}",
                        value_type_name_str(v)
                    ),
                ));
            }
        },
    };

    let disp = path.to_string_lossy().into_owned();
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| PyError::named("ValueError", "embedded null byte in path".to_string()))?;
    // SAFETY: `c_path` is NUL-terminated.
    let rc = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(PyError::from_io_error(&err, Some(&disp)));
    }
    Ok(Value::none())
}

// ── Archive & Unpack Format Registries ────────────────────────────────────────

pub(super) fn init_format_registries(
    module: &std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>,
) {
    let make_tarball_fn = Value::builtin_function("shutil._make_tarball");
    let make_zipfile_fn = Value::builtin_function("shutil._make_zipfile");
    let unpack_tarfile_fn = Value::builtin_function("shutil._unpack_tarfile");
    let unpack_zipfile_fn = Value::builtin_function("shutil._unpack_zipfile");

    let mut archive_formats = PyDict::default();
    type ArchiveEntry<'a> = (&'a str, Value, Option<(&'a str, Value)>, &'a str);
    let archive_entries: [ArchiveEntry<'_>; 5] = [
        (
            "gztar",
            make_tarball_fn.clone(),
            Some(("compress", Value::string("gzip"))),
            "gzip'ed tar-file",
        ),
        (
            "bztar",
            make_tarball_fn.clone(),
            Some(("compress", Value::string("bzip2"))),
            "bzip2'ed tar-file",
        ),
        (
            "xztar",
            make_tarball_fn.clone(),
            Some(("compress", Value::string("xz"))),
            "xz'ed tar-file",
        ),
        (
            "tar",
            make_tarball_fn,
            Some(("compress", Value::none())),
            "uncompressed tar file",
        ),
        ("zip", make_zipfile_fn, None, "ZIP file"),
    ];
    for (name, func, extra, desc) in archive_entries {
        let extra_list = match extra {
            Some((k, v)) => Value::list(vec![Value::tuple(vec![Value::string(k), v])]),
            None => Value::list(Vec::new()),
        };
        archive_formats.insert(
            PyKey::str_from(name),
            Value::tuple(vec![func, extra_list, Value::string(desc)]),
        );
    }

    let mut unpack_formats = PyDict::default();
    let unpack_entries: [(&str, &[&str], Value, &str); 5] = [
        (
            "tar",
            &[".tar"],
            unpack_tarfile_fn.clone(),
            "uncompressed tar file",
        ),
        (
            "gztar",
            &[".tar.gz", ".tgz"],
            unpack_tarfile_fn.clone(),
            "gzip'ed tar-file",
        ),
        (
            "bztar",
            &[".tar.bz2", ".tbz2"],
            unpack_tarfile_fn.clone(),
            "bzip2'ed tar-file",
        ),
        (
            "xztar",
            &[".tar.xz", ".txz"],
            unpack_tarfile_fn,
            "xz'ed tar-file",
        ),
        ("zip", &[".zip"], unpack_zipfile_fn, "ZIP file"),
    ];
    for (name, exts, func, desc) in unpack_entries {
        let ext_list = Value::list(exts.iter().map(|e| Value::string(*e)).collect());
        unpack_formats.insert(
            PyKey::str_from(name),
            Value::tuple(vec![
                ext_list,
                func,
                Value::list(Vec::new()),
                Value::string(desc),
            ]),
        );
    }

    let mut mod_ref = module.borrow_mut();
    mod_ref
        .attrs
        .insert("_ARCHIVE_FORMATS".to_string(), Value::dict(archive_formats));
    mod_ref
        .attrs
        .insert("_UNPACK_FORMATS".to_string(), Value::dict(unpack_formats));
}

fn shutil_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("shutil")?;
    interp.get_attr(&m, name)
}

fn raise_shutil_error(interp: &mut Interpreter, exc_name: &str, msg: &str) -> PyError {
    if let Ok(cls) = shutil_attr(interp, exc_name)
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named("OSError", msg.to_string())
}

fn check_callable(interp: &mut Interpreter, func: &Value) -> Result<()> {
    let is_call = call_pos(
        interp,
        Value::builtin_function("callable"),
        std::slice::from_ref(func),
    )?;
    if !interp.truthy_value(&is_call)? {
        let repr = crate::interpreter::render_value_repr(interp, func)?;
        return Err(PyError::named(
            "TypeError",
            format!("{repr} is not callable"),
        ));
    }
    Ok(())
}

fn validate_extra_args(interp: &mut Interpreter, extra_args: Option<&Value>) -> Result<Value> {
    let Some(val) = extra_args.filter(|v| !v.is_none()) else {
        return Ok(Value::list(Vec::new()));
    };
    if !matches!(val.kind(), ValueKind::List(_) | ValueKind::Tuple(_)) {
        return Err(PyError::named(
            "TypeError",
            "extra_args needs to be a sequence".to_string(),
        ));
    }
    let items = interp.collect_iterable(val)?;
    let mut normalized = Vec::with_capacity(items.len());
    for item in items {
        let pair = interp.collect_iterable(&item).map_err(|_| {
            PyError::named(
                "TypeError",
                "extra_args elements are : (arg_name, value)".to_string(),
            )
        })?;
        if pair.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                "extra_args elements are : (arg_name, value)".to_string(),
            ));
        }
        normalized.push(Value::tuple(pair));
    }
    Ok(Value::list(normalized))
}

pub(super) fn get_archive_formats_impl(interp: &mut Interpreter) -> Result<Value> {
    let reg = shutil_attr(interp, "_ARCHIVE_FORMATS")?;
    let mut entries: Vec<(String, Value)> = Vec::new();
    let items = call_method(interp, &reg, "items", &[])?;
    for pair in interp.collect_iterable(&items)? {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() == 2
            && let Some(k) = kv[0].as_str()
        {
            let info = interp.collect_iterable(&kv[1])?;
            let desc = info.get(2).cloned().unwrap_or_else(|| Value::string(""));
            entries.push((k.to_string(), desc));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::list(
        entries
            .into_iter()
            .map(|(k, d)| Value::tuple(vec![Value::string(k), d]))
            .collect(),
    ))
}

pub(super) fn register_archive_format_impl(
    interp: &mut Interpreter,
    name: &str,
    function: Value,
    extra_args: Option<&Value>,
    description: &str,
) -> Result<Value> {
    check_callable(interp, &function)?;
    let extra = validate_extra_args(interp, extra_args)?;
    let reg = shutil_attr(interp, "_ARCHIVE_FORMATS")?;
    let entry = Value::tuple(vec![function, extra, Value::string(description)]);
    call_method(interp, &reg, "__setitem__", &[Value::string(name), entry])?;
    Ok(Value::none())
}

pub(super) fn unregister_archive_format_impl(
    interp: &mut Interpreter,
    name: &str,
) -> Result<Value> {
    let reg = shutil_attr(interp, "_ARCHIVE_FORMATS")?;
    call_method(interp, &reg, "__delitem__", &[Value::string(name)])?;
    Ok(Value::none())
}

pub(super) fn get_unpack_formats_impl(interp: &mut Interpreter) -> Result<Value> {
    let reg = shutil_attr(interp, "_UNPACK_FORMATS")?;
    let mut entries: Vec<(String, Value, Value)> = Vec::new();
    let items = call_method(interp, &reg, "items", &[])?;
    for pair in interp.collect_iterable(&items)? {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() == 2
            && let Some(k) = kv[0].as_str()
        {
            let info = interp.collect_iterable(&kv[1])?;
            let exts = info
                .first()
                .cloned()
                .unwrap_or_else(|| Value::list(Vec::new()));
            let desc = info.get(3).cloned().unwrap_or_else(|| Value::string(""));
            entries.push((k.to_string(), exts, desc));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::list(
        entries
            .into_iter()
            .map(|(k, exts, d)| Value::tuple(vec![Value::string(k), exts, d]))
            .collect(),
    ))
}

pub(super) fn register_unpack_format_impl(
    interp: &mut Interpreter,
    name: &str,
    extensions: Value,
    function: Value,
    extra_args: Option<&Value>,
    description: &str,
) -> Result<Value> {
    let ext_items = interp.collect_iterable(&extensions)?;
    let mut ext_strs = Vec::with_capacity(ext_items.len());
    for e in &ext_items {
        let s = e
            .as_str()
            .ok_or_else(|| PyError::named("TypeError", "extensions must be strings".to_string()))?;
        ext_strs.push(s.to_string());
    }

    let reg = shutil_attr(interp, "_UNPACK_FORMATS")?;
    // Check extension collision against existing registered formats.
    let items = call_method(interp, &reg, "items", &[])?;
    for pair in interp.collect_iterable(&items)? {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() == 2
            && let Some(existing_name) = kv[0].as_str()
        {
            let info = interp.collect_iterable(&kv[1])?;
            if let Some(existing_exts_val) = info.first() {
                for existing_ext in interp.collect_iterable(existing_exts_val)? {
                    if let Some(es) = existing_ext.as_str()
                        && ext_strs.iter().any(|x| x == es)
                    {
                        let msg = format!("{es} is already registered for \"{existing_name}\"");
                        return Err(raise_shutil_error(interp, "RegistryError", &msg));
                    }
                }
            }
        }
    }

    check_callable(interp, &function)?;
    let extra = validate_extra_args(interp, extra_args)?;
    let ext_list = Value::list(ext_strs.into_iter().map(Value::string).collect());
    let entry = Value::tuple(vec![ext_list, function, extra, Value::string(description)]);
    call_method(interp, &reg, "__setitem__", &[Value::string(name), entry])?;
    Ok(Value::none())
}

pub(super) fn unregister_unpack_format_impl(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let reg = shutil_attr(interp, "_UNPACK_FORMATS")?;
    call_method(interp, &reg, "__delitem__", &[Value::string(name)])?;
    Ok(Value::none())
}

// ── Archive Creation & Extraction ────────────────────────────────────────────

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| PyError::from_io_error(&e, Some(&parent.to_string_lossy())))?;
    }
    Ok(())
}

fn log_info(interp: &mut Interpreter, logger: Option<&Value>, msg: &str) {
    if let Some(lg) = logger.filter(|v| !v.is_none()) {
        let _ = call_method(interp, lg, "info", &[Value::string(msg)]);
    }
}

fn log_debug(interp: &mut Interpreter, logger: Option<&Value>, msg: &str) {
    if let Some(lg) = logger.filter(|v| !v.is_none()) {
        let _ = call_method(interp, lg, "debug", &[Value::string(msg)]);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn make_tarball_impl(
    interp: &mut Interpreter,
    base_name: &str,
    base_dir: &str,
    compress: Option<&str>,
    dry_run: bool,
    owner: Option<&Value>,
    group: Option<&Value>,
    logger: Option<&Value>,
) -> Result<Value> {
    let (tar_comp, ext) = match compress {
        None => ("", ""),
        Some("gzip") => ("gz", ".gz"),
        Some("bzip2") => ("bz2", ".bz2"),
        Some("xz") => ("xz", ".xz"),
        Some(other) => {
            return Err(PyError::named(
                "ValueError",
                format!(
                    "bad value for 'compress', or compression format not supported : {other:?}"
                ),
            ));
        }
    };

    let archive_name = format!("{base_name}.tar{ext}");
    if !dry_run {
        ensure_parent_dir(Path::new(&archive_name))?;
    }
    log_info(interp, logger, "Creating tar archive");
    if !dry_run {
        let tarfile_mod = interp.load_module("tarfile")?;
        let open_fn = interp.get_attr(&tarfile_mod, "open")?;
        let mode = format!("w:{tar_comp}");
        let tar = call_pos(
            interp,
            open_fn,
            &[Value::string(&archive_name), Value::string(&mode)],
        )?;
        let add_res = (|| -> Result<()> {
            let _ = (owner, group);
            call_method_kw(interp, &tar, "add", &[Value::string(base_dir)], &[])?;
            Ok(())
        })();
        let close_res = call_method(interp, &tar, "close", &[]);
        add_res?;
        close_res?;
    }
    Ok(Value::string(archive_name))
}

fn walk_for_zip(
    root: &Path,
    archive_canon: Option<&Path>,
    out: &mut Vec<(PathBuf, bool)>,
) -> Result<()> {
    let meta = std::fs::metadata(root)
        .map_err(|e| PyError::from_io_error(&e, Some(&root.to_string_lossy())))?;
    if !meta.is_dir() {
        out.push((root.to_path_buf(), false));
        return Ok(());
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut subdirs = Vec::new();
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| PyError::from_io_error(&e, Some(&dir.to_string_lossy())))?
        {
            let entry =
                entry.map_err(|e| PyError::from_io_error(&e, Some(&dir.to_string_lossy())))?;
            let path = entry.path();
            if let Some(ac) = archive_canon
                && let Ok(pc) = std::fs::canonicalize(&path)
                && pc == ac
            {
                continue;
            }
            let ft = entry
                .file_type()
                .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
            if ft.is_dir() {
                subdirs.push(path);
            } else {
                files.push(path);
            }
        }
        subdirs.sort();
        files.sort();
        for d in &subdirs {
            out.push((d.clone(), true));
        }
        for f in files {
            out.push((f, false));
        }
        for d in subdirs.into_iter().rev() {
            stack.push(d);
        }
    }
    Ok(())
}

fn normalize_zip_arcname(p: &Path) -> String {
    let mut parts = Vec::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            std::path::Component::ParentDir => parts.push("..".to_string()),
            _ => {}
        }
    }
    parts.join("/")
}

pub(super) fn make_zipfile_impl(
    interp: &mut Interpreter,
    base_name: &str,
    base_dir: &str,
    dry_run: bool,
    logger: Option<&Value>,
) -> Result<Value> {
    let zip_filename = format!("{base_name}.zip");
    if !dry_run {
        ensure_parent_dir(Path::new(&zip_filename))?;
    }
    log_info(
        interp,
        logger,
        &format!("creating '{zip_filename}' and adding '{base_dir}' to it"),
    );

    if !dry_run {
        let zipfile_mod = interp.load_module("zipfile")?;
        let zip_cls = interp.get_attr(&zipfile_mod, "ZipFile")?;
        let zip_deflated = interp.get_attr(&zipfile_mod, "ZIP_DEFLATED")?;
        let zf = call_kw(
            interp,
            zip_cls,
            &[Value::string(&zip_filename), Value::string("w")],
            &[("compression", zip_deflated)],
        )?;
        let archive_canon = std::fs::canonicalize(&zip_filename).ok();
        let write_res = (|| -> Result<()> {
            let base_path = Path::new(base_dir);
            let mut entries = Vec::new();
            let base_norm = normalize_zip_arcname(base_path);
            if base_path.is_dir() && !base_norm.is_empty() {
                entries.push((base_path.to_path_buf(), true));
            }
            walk_for_zip(base_path, archive_canon.as_deref(), &mut entries)?;
            for (path, is_dir) in entries {
                let mut arcname = normalize_zip_arcname(&path);
                if arcname.is_empty() {
                    continue;
                }
                if is_dir && !arcname.ends_with('/') {
                    arcname.push('/');
                }
                let path_str = path.to_string_lossy().into_owned();
                if is_dir {
                    call_method(
                        interp,
                        &zf,
                        "writestr",
                        &[Value::string(&arcname), Value::bytes(Vec::new())],
                    )?;
                } else {
                    call_method(
                        interp,
                        &zf,
                        "write",
                        &[Value::string(&path_str), Value::string(&arcname)],
                    )?;
                }
            }
            Ok(())
        })();
        let close_res = call_method(interp, &zf, "close", &[]);
        write_res?;
        close_res?;
    }

    Ok(Value::string(zip_filename))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn make_archive_impl(
    interp: &mut Interpreter,
    base_name: &Path,
    format: &str,
    root_dir: Option<&Path>,
    base_dir: Option<&Path>,
    dry_run: bool,
    owner: Option<&Value>,
    group: Option<&Value>,
    logger: Option<&Value>,
) -> Result<Value> {
    let reg = shutil_attr(interp, "_ARCHIVE_FORMATS")?;
    let format_info = call_method(interp, &reg, "get", &[Value::string(format), Value::none()])?;
    if format_info.is_none() {
        return Err(PyError::named(
            "ValueError",
            format!("unknown archive format '{format}'"),
        ));
    }
    let parts = interp.collect_iterable(&format_info)?;
    let func = parts
        .first()
        .cloned()
        .ok_or_else(|| PyError::runtime("corrupt archive format entry".to_string()))?;
    let extra_args_val = parts
        .get(1)
        .cloned()
        .unwrap_or_else(|| Value::list(Vec::new()));

    let mut save_cwd: Option<PathBuf> = None;
    let effective_base_name: String = if let Some(rd) = root_dir {
        log_debug(interp, logger, &format!("changing into '{}'", rd.display()));
        let abs = if base_name.is_absolute() {
            base_name.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| PyError::from_io_error(&e, None))?
                .join(base_name)
        };
        if !dry_run {
            let cwd = std::env::current_dir().map_err(|e| PyError::from_io_error(&e, None))?;
            std::env::set_current_dir(rd)
                .map_err(|e| PyError::from_io_error(&e, Some(&rd.to_string_lossy())))?;
            save_cwd = Some(cwd);
        }
        abs.to_string_lossy().into_owned()
    } else {
        base_name.to_string_lossy().into_owned()
    };

    let effective_base_dir = match base_dir {
        Some(bd) => bd.to_string_lossy().into_owned(),
        None => ".".to_string(),
    };

    let mut kwargs: IndexMap<String, Value> = IndexMap::new();
    kwargs.insert("dry_run".to_string(), Value::bool_(dry_run));
    kwargs.insert(
        "logger".to_string(),
        logger.cloned().unwrap_or_else(Value::none),
    );
    for pair in interp.collect_iterable(&extra_args_val)? {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() == 2
            && let Some(k) = kv[0].as_str()
        {
            kwargs.insert(k.to_string(), kv[1].clone());
        }
    }
    if format != "zip" {
        kwargs.insert(
            "owner".to_string(),
            owner.cloned().unwrap_or_else(Value::none),
        );
        kwargs.insert(
            "group".to_string(),
            group.cloned().unwrap_or_else(Value::none),
        );
    }

    let mut call_args = vec![
        ExpandedCallArg {
            name: None,
            value: Value::string(&effective_base_name),
        },
        ExpandedCallArg {
            name: None,
            value: Value::string(&effective_base_dir),
        },
    ];
    for (k, v) in kwargs {
        call_args.push(ExpandedCallArg {
            name: Some(k),
            value: v,
        });
    }

    let result = interp.call_function_expanded(func, &call_args);
    if let Some(cwd) = save_cwd {
        log_debug(
            interp,
            logger,
            &format!("changing back to '{}'", cwd.display()),
        );
        let _ = std::env::set_current_dir(cwd);
    }
    result
}

pub(super) fn unpack_zipfile_impl(
    interp: &mut Interpreter,
    filename: &str,
    extract_dir: &str,
) -> Result<Value> {
    let zipfile_mod = interp.load_module("zipfile")?;
    let is_zip_fn = interp.get_attr(&zipfile_mod, "is_zipfile")?;
    let is_zip = call_pos(interp, is_zip_fn, &[Value::string(filename)])?;
    if !interp.truthy_value(&is_zip)? {
        return Err(raise_shutil_error(
            interp,
            "ReadError",
            &format!("{filename} is not a zip file"),
        ));
    }
    std::fs::create_dir_all(extract_dir)
        .map_err(|e| PyError::from_io_error(&e, Some(extract_dir)))?;
    let zip_cls = interp.get_attr(&zipfile_mod, "ZipFile")?;
    let zf = call_pos(interp, zip_cls, &[Value::string(filename)])?;
    let res = call_method(interp, &zf, "extractall", &[Value::string(extract_dir)]);
    let _ = call_method(interp, &zf, "close", &[]);
    res?;
    Ok(Value::none())
}

pub(super) fn unpack_tarfile_impl(
    interp: &mut Interpreter,
    filename: &str,
    extract_dir: &str,
    filter: Option<&Value>,
) -> Result<Value> {
    let tarfile_mod = interp.load_module("tarfile")?;
    let open_fn = interp.get_attr(&tarfile_mod, "open")?;
    let tar = match call_pos(
        interp,
        open_fn,
        &[Value::string(filename), Value::string("r:*")],
    ) {
        Ok(t) => t,
        Err(err) => {
            if let Ok(tar_err_cls) = interp.get_attr(&tarfile_mod, "TarError")
                && let Ok(exc) = interp.materialize_pyerror(err.clone())
                && let Ok(is_tar_err) = call_pos(
                    interp,
                    Value::builtin_function("isinstance"),
                    &[exc, tar_err_cls],
                )
                && interp.truthy_value(&is_tar_err).unwrap_or(false)
            {
                return Err(raise_shutil_error(
                    interp,
                    "ReadError",
                    &format!("{filename} is not a compressed or uncompressed tar file"),
                ));
            }
            return Err(err);
        }
    };
    std::fs::create_dir_all(extract_dir)
        .map_err(|e| PyError::from_io_error(&e, Some(extract_dir)))?;
    let res = if let Some(flt) = filter.filter(|v| !v.is_none()) {
        call_method_kw(
            interp,
            &tar,
            "extractall",
            &[Value::string(extract_dir)],
            &[("filter", flt.clone())],
        )
    } else {
        call_method(interp, &tar, "extractall", &[Value::string(extract_dir)])
    };
    let _ = call_method(interp, &tar, "close", &[]);
    res?;
    Ok(Value::none())
}

pub(super) fn unpack_archive_impl(
    interp: &mut Interpreter,
    filename: &Path,
    extract_dir: Option<&Path>,
    format: Option<&str>,
    filter: Option<&Value>,
) -> Result<Value> {
    let filename_str = filename.to_string_lossy().into_owned();
    let extract_str = match extract_dir {
        Some(d) => d.to_string_lossy().into_owned(),
        None => std::env::current_dir()
            .map_err(|e| PyError::from_io_error(&e, None))?
            .to_string_lossy()
            .into_owned(),
    };

    let reg = shutil_attr(interp, "_UNPACK_FORMATS")?;
    let (func, extra_args_val, matched_name) = if let Some(fmt) = format {
        let info = call_method(interp, &reg, "get", &[Value::string(fmt), Value::none()])?;
        if info.is_none() {
            return Err(PyError::named(
                "ValueError",
                format!("Unknown unpack format '{fmt}'"),
            ));
        }
        let parts = interp.collect_iterable(&info)?;
        (
            parts
                .get(1)
                .cloned()
                .ok_or_else(|| PyError::runtime("corrupt unpack format entry".to_string()))?,
            parts
                .get(2)
                .cloned()
                .unwrap_or_else(|| Value::list(Vec::new())),
            fmt.to_string(),
        )
    } else {
        let items = call_method(interp, &reg, "items", &[])?;
        let mut found = None;
        for pair in interp.collect_iterable(&items)? {
            let kv = interp.collect_iterable(&pair)?;
            if kv.len() == 2
                && let Some(fmt_name) = kv[0].as_str()
            {
                let parts = interp.collect_iterable(&kv[1])?;
                if let Some(exts_val) = parts.first() {
                    for ext in interp.collect_iterable(exts_val)? {
                        if let Some(es) = ext.as_str()
                            && filename_str.ends_with(es)
                        {
                            found = Some((
                                parts.get(1).cloned().ok_or_else(|| {
                                    PyError::runtime("corrupt unpack format entry".to_string())
                                })?,
                                parts
                                    .get(2)
                                    .cloned()
                                    .unwrap_or_else(|| Value::list(Vec::new())),
                                fmt_name.to_string(),
                            ));
                            break;
                        }
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        match found {
            Some(f) => f,
            None => {
                return Err(raise_shutil_error(
                    interp,
                    "ReadError",
                    &format!("Unknown archive format '{filename_str}'"),
                ));
            }
        }
    };

    let mut call_args = vec![
        ExpandedCallArg {
            name: None,
            value: Value::string(&filename_str),
        },
        ExpandedCallArg {
            name: None,
            value: Value::string(&extract_str),
        },
    ];
    for pair in interp.collect_iterable(&extra_args_val)? {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() == 2
            && let Some(k) = kv[0].as_str()
        {
            call_args.push(ExpandedCallArg {
                name: Some(k.to_string()),
                value: kv[1].clone(),
            });
        }
    }
    if let Some(flt) = filter.filter(|v| !v.is_none()) {
        // Pass `filter` when unpacking tar formats or custom unpackers that accept it.
        if matches!(matched_name.as_str(), "tar" | "gztar" | "bztar" | "xztar") {
            call_args.push(ExpandedCallArg {
                name: Some("filter".to_string()),
                value: flt.clone(),
            });
        }
    }

    interp.call_function_expanded(func, &call_args)?;
    Ok(Value::none())
}
