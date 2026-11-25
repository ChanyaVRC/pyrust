//! Host-filesystem and interpreter-facing `Path` operations.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, NativeIterFrame};
use crate::value::{PyClass, Value, ValueKind};

use super::class_registry::{
    PathFlavor, expect_self, flavor_for_instance, get_path, make_path_classmethod_result,
    make_path_instance_for_class, make_path_result,
};
use super::lexical::{
    extract_path_segment, glob_component_matches, glob_parts_match_refs, normalize_for_flavor,
};

fn clean_canonical(p: &Path) -> String {
    let s = p.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return stripped.to_string();
        }
    }
    s.into_owned()
}

pub(super) fn path_exists(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::bool_(Path::new(&path).exists()))
}

pub(super) fn path_is_file(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::bool_(Path::new(&path).is_file()))
}

pub(super) fn path_is_dir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::bool_(Path::new(&path).is_dir()))
}

pub(super) fn path_is_symlink(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    Ok(Value::bool_(
        std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()),
    ))
}

pub(super) fn path_readlink(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    let target =
        std::fs::read_link(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    let flavor = flavor_for_instance(&inst);
    Ok(make_path_result(
        &inst,
        &normalize_for_flavor(&target.to_string_lossy(), flavor),
    ))
}

pub(super) fn path_stat(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut follow_symlinks = true;
    let mut seen_follow = false;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("follow_symlinks") => {
                if seen_follow {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'follow_symlinks'"),
                    ));
                }
                follow_symlinks = interp.truthy_value(&arg.value)?;
                seen_follow = true;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() takes 0 positional arguments"),
                ));
            }
        }
    }
    let path = get_path(&inst, fn_name)?;
    let meta = if follow_symlinks {
        std::fs::metadata(&path)
    } else {
        std::fs::symlink_metadata(&path)
    }
    .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(crate::builtin_modules::os::make_stat_result(&meta))
}

pub(super) fn path_lstat(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(crate::builtin_modules::os::make_stat_result(&meta))
}

pub(super) fn path_samefile(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 || args[1].name.is_some() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 positional argument"),
        ));
    }
    let p1 = get_path(&inst, fn_name)?;
    let p2 = extract_path_segment(interp, &args[1].value, fn_name)?;
    let m1 = std::fs::metadata(&p1).map_err(|e| PyError::from_io_error(&e, Some(&p1)))?;
    let m2 = std::fs::metadata(&p2).map_err(|e| PyError::from_io_error(&e, Some(&p2)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Value::bool_(m1.dev() == m2.dev() && m1.ino() == m2.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = (m1, m2);
        let c1 = std::fs::canonicalize(&p1).map_err(|e| PyError::from_io_error(&e, Some(&p1)))?;
        let c2 = std::fs::canonicalize(&p2).map_err(|e| PyError::from_io_error(&e, Some(&p2)))?;
        Ok(Value::bool_(c1 == c2))
    }
}

pub(super) fn path_touch(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut mode: i64 = 0o666;
    let mut exist_ok = true;
    let mut pos_idx = 0usize;
    let mut seen_mode = false;
    let mut seen_exist_ok = false;

    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("mode") => {
                if seen_mode {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'mode'"),
                    ));
                }
                mode = extract_int_arg(&arg.value, fn_name, "mode")?;
                seen_mode = true;
            }
            Some("exist_ok") => {
                if seen_exist_ok {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'exist_ok'"),
                    ));
                }
                exist_ok = interp.truthy_value(&arg.value)?;
                seen_exist_ok = true;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                match pos_idx {
                    0 => {
                        mode = extract_int_arg(&arg.value, fn_name, "mode")?;
                        seen_mode = true;
                    }
                    1 => {
                        exist_ok = interp.truthy_value(&arg.value)?;
                        seen_exist_ok = true;
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{fn_name}() takes at most 2 arguments"),
                        ));
                    }
                }
                pos_idx += 1;
            }
        }
    }

    let path = get_path(&inst, fn_name)?;
    if exist_ok && let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) {
        let _ = file.set_modified(std::time::SystemTime::now());
        return Ok(Value::none());
    }

    let mut open_opts = std::fs::OpenOptions::new();
    open_opts.write(true);
    if exist_ok {
        open_opts.create(true);
    } else {
        open_opts.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode((mode & 0o7777) as u32);
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    open_opts
        .open(&path)
        .map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn path_rename(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 || args[1].name.is_some() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 positional argument"),
        ));
    }
    let src = get_path(&inst, fn_name)?;
    let dst = extract_path_segment(interp, &args[1].value, fn_name)?;
    #[cfg(windows)]
    {
        // CPython's `Path.rename` on Windows raises FileExistsError when the
        // target already exists (unlike `Path.replace`, which overwrites it).
        if std::fs::symlink_metadata(&dst).is_ok() {
            let err = std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Cannot create a file when that file already exists",
            );
            return Err(PyError::from_io_error2(&err, Some(&src), Some(&dst)));
        }
    }
    std::fs::rename(&src, &dst)
        .map_err(|error| PyError::from_io_error2(&error, Some(&src), Some(&dst)))?;
    let flavor = flavor_for_instance(&inst);
    Ok(make_path_result(&inst, &normalize_for_flavor(&dst, flavor)))
}

pub(super) fn path_replace(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 || args[1].name.is_some() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 positional argument"),
        ));
    }
    let src = get_path(&inst, fn_name)?;
    let dst = extract_path_segment(interp, &args[1].value, fn_name)?;
    std::fs::rename(&src, &dst)
        .map_err(|error| PyError::from_io_error2(&error, Some(&src), Some(&dst)))?;
    let flavor = flavor_for_instance(&inst);
    Ok(make_path_result(&inst, &normalize_for_flavor(&dst, flavor)))
}

pub(super) fn path_rmdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    std::fs::remove_dir(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::none())
}

pub(super) fn path_read_text(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut encoding: Option<String> = None;
    let mut errors: Option<String> = None;
    let mut pos_idx = 0usize;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("encoding") => encoding = opt_str_arg(&arg.value, fn_name, "encoding")?,
            Some("errors") => errors = opt_str_arg(&arg.value, fn_name, "errors")?,
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                match pos_idx {
                    0 => encoding = opt_str_arg(&arg.value, fn_name, "encoding")?,
                    1 => errors = opt_str_arg(&arg.value, fn_name, "errors")?,
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{fn_name}() takes at most 2 arguments"),
                        ));
                    }
                }
                pos_idx += 1;
            }
        }
    }
    let path = get_path(&inst, fn_name)?;
    let raw = std::fs::read(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    let enc = encoding.as_deref().unwrap_or("utf-8");
    let errs = errors.as_deref().unwrap_or("strict");
    let decoded_val = pyrust_builtins::bytes::decode_bytes(&raw, enc, errs)?;
    if let ValueKind::Str(s) = decoded_val.kind() {
        // Universal newline translation (matching Path.read_text -> open(mode='r')).
        let mut out = Vec::with_capacity(s.len());
        let bytes = s.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\r' {
                out.push(b'\n');
                i += if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    2
                } else {
                    1
                };
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        // SAFETY: only ASCII \r/\n are rewritten.
        let normalized = unsafe { String::from_utf8_unchecked(out) };
        return Ok(Value::string(normalized));
    }
    Ok(decoded_val)
}

pub(super) fn path_write_text(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut data_arg: Option<&Value> = None;
    let mut encoding: Option<String> = None;
    let mut errors: Option<String> = None;
    let mut newline: Option<String> = None;
    let mut pos_idx = 0usize;

    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("data") => data_arg = Some(&arg.value),
            Some("encoding") => encoding = opt_str_arg(&arg.value, fn_name, "encoding")?,
            Some("errors") => errors = opt_str_arg(&arg.value, fn_name, "errors")?,
            Some("newline") => newline = opt_str_arg(&arg.value, fn_name, "newline")?,
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                match pos_idx {
                    0 => data_arg = Some(&arg.value),
                    1 => encoding = opt_str_arg(&arg.value, fn_name, "encoding")?,
                    2 => errors = opt_str_arg(&arg.value, fn_name, "errors")?,
                    3 => newline = opt_str_arg(&arg.value, fn_name, "newline")?,
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{fn_name}() takes at most 4 arguments"),
                        ));
                    }
                }
                pos_idx += 1;
            }
        }
    }
    let Some(data_val) = data_arg else {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing 1 required positional argument: 'data'"),
        ));
    };
    let data = match data_val.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): data must be str, not {}",
                    pyrust_core::builtin_type_name(data_val),
                ),
            ));
        }
    };
    let path = get_path(&inst, fn_name)?;
    let file_obj = pyrust_builtins::file::open_with_options(
        &path,
        pyrust_builtins::file::FileOpenOptions {
            mode: "w",
            buffering: -1,
            encoding: encoding.as_deref(),
            errors: errors.as_deref(),
            newline: newline.as_deref(),
            closefd: true,
        },
    )?;
    if let ValueKind::BuiltinObject { ops, state } = file_obj.kind() {
        let empty = indexmap::IndexMap::new();
        let written = ops.call_method(state, "write", vec![Value::string(data)], &empty)?;
        ops.call_method(state, "close", Vec::new(), &empty)?;
        return Ok(written);
    }
    unreachable!("file::open_with_options always returns a BuiltinObject")
}

pub(super) fn path_mkdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut parents = false;
    let mut exist_ok = false;
    let mut pos_index: usize = 0;
    let mut seen_mode_kw = false;
    let mut seen_parents_kw = false;
    let mut seen_exist_ok_kw = false;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("mode") => {
                if seen_mode_kw {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'mode'"),
                    ));
                }
                seen_mode_kw = true;
            }
            Some("parents") => {
                if seen_parents_kw {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'parents'"),
                    ));
                }
                parents = is_truthy_flag(&arg.value);
                seen_parents_kw = true;
            }
            Some("exist_ok") => {
                if seen_exist_ok_kw {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'exist_ok'"),
                    ));
                }
                exist_ok = is_truthy_flag(&arg.value);
                seen_exist_ok_kw = true;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                match pos_index {
                    0 => {}
                    1 => parents = is_truthy_flag(&arg.value),
                    2 => exist_ok = is_truthy_flag(&arg.value),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{fn_name}() takes at most 3 arguments"),
                        ));
                    }
                }
                pos_index += 1;
            }
        }
    }
    let path = get_path(&inst, fn_name)?;
    let result = if parents {
        std::fs::create_dir_all(&path)
    } else {
        std::fs::create_dir(&path)
    };
    match result {
        Ok(()) => Ok(Value::none()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && exist_ok => {
            Ok(Value::none())
        }
        Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
    }
}

pub(super) fn path_cwd(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let cwd = std::env::current_dir().map_err(|error| PyError::from_io_error(&error, None))?;
    let path = cwd.to_string_lossy();
    make_path_classmethod_result(interp, args, path.into_owned())
}

pub(super) fn path_home(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| {
            PyError::named(
                "RuntimeError",
                "Could not determine home directory".to_string(),
            )
        })?;
    make_path_classmethod_result(interp, args, home)
}

pub(super) fn path_resolve(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut strict = false;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("strict") => strict = is_truthy_flag(&arg.value),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => strict = is_truthy_flag(&arg.value),
        }
    }
    let path = get_path(&inst, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    match std::fs::canonicalize(&path) {
        Ok(resolved) => Ok(make_path_result(
            &inst,
            &normalize_for_flavor(&clean_canonical(&resolved), flavor),
        )),
        Err(error) => {
            if strict {
                Err(PyError::from_io_error(&error, Some(&path)))
            } else {
                let raw = Path::new(&path);
                let absolute = if raw.is_absolute() {
                    raw.to_path_buf()
                } else {
                    std::env::current_dir()
                        .unwrap_or_else(|_| PathBuf::from("/"))
                        .join(raw)
                };
                // Try resolving deepest existing ancestor when strict=False.
                let mut cur = absolute.clone();
                let mut tail: Vec<std::ffi::OsString> = Vec::new();
                while let Some(name) = cur.file_name() {
                    tail.push(name.to_os_string());
                    if !cur.pop() {
                        break;
                    }
                    if let Ok(resolved_parent) = std::fs::canonicalize(&cur) {
                        let mut out = PathBuf::from(clean_canonical(&resolved_parent));
                        for part in tail.into_iter().rev() {
                            out.push(part);
                        }
                        return Ok(make_path_result(
                            &inst,
                            &normalize_for_flavor(&out.to_string_lossy(), flavor),
                        ));
                    }
                }
                Ok(make_path_result(
                    &inst,
                    &normalize_for_flavor(&absolute.to_string_lossy(), flavor),
                ))
            }
        }
    }
}

pub(super) fn path_read_bytes(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Value::bytes(bytes)),
        Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
    }
}

pub(super) fn path_write_bytes(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let data = match args[1].value.kind() {
        ValueKind::Bytes(bytes) => bytes.to_vec(),
        ValueKind::BuiltinObject { .. } => {
            match pyrust_builtins::bytearray::as_bytearray_snapshot(&args[1].value) {
                Some(bytes) => bytes,
                None => return Err(bytes_like_type_error(fn_name, &args[1].value)),
            }
        }
        _ => return Err(bytes_like_type_error(fn_name, &args[1].value)),
    };
    let path = get_path(&inst, fn_name)?;
    let len = data.len();
    std::fs::write(&path, &data).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    Ok(Value::int(len as i64))
}

pub(super) fn path_open(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    let mut mode = "r".to_string();
    let mut buffering: i64 = -1;
    let mut encoding: Option<String> = None;
    let mut errors: Option<String> = None;
    let mut newline: Option<String> = None;
    let mut pos_idx = 0usize;

    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("mode") => {
                if let ValueKind::Str(value) = arg.value.kind() {
                    mode = value.to_string();
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        "open() argument 'mode' must be str".to_string(),
                    ));
                }
            }
            Some("buffering") => buffering = extract_int_arg(&arg.value, fn_name, "buffering")?,
            Some("encoding") => encoding = opt_str_arg(&arg.value, fn_name, "encoding")?,
            Some("errors") => errors = opt_str_arg(&arg.value, fn_name, "errors")?,
            Some("newline") => newline = opt_str_arg(&arg.value, fn_name, "newline")?,
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => {
                match pos_idx {
                    0 => {
                        if let ValueKind::Str(value) = arg.value.kind() {
                            mode = value.to_string();
                        } else {
                            return Err(PyError::named(
                                "TypeError",
                                "open() argument 'mode' must be str".to_string(),
                            ));
                        }
                    }
                    1 => buffering = extract_int_arg(&arg.value, fn_name, "buffering")?,
                    2 => encoding = opt_str_arg(&arg.value, fn_name, "encoding")?,
                    3 => errors = opt_str_arg(&arg.value, fn_name, "errors")?,
                    4 => newline = opt_str_arg(&arg.value, fn_name, "newline")?,
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{fn_name}() takes at most 5 arguments"),
                        ));
                    }
                }
                pos_idx += 1;
            }
        }
    }
    pyrust_builtins::file::open_with_options(
        &path,
        pyrust_builtins::file::FileOpenOptions {
            mode: &mode,
            buffering,
            encoding: encoding.as_deref(),
            errors: errors.as_deref(),
            newline: newline.as_deref(),
            closefd: true,
        },
    )
}

pub(super) fn path_unlink(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut missing_ok = false;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            Some("missing_ok") => missing_ok = is_truthy_flag(&arg.value),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => missing_ok = is_truthy_flag(&arg.value),
        }
    }
    let path = get_path(&inst, fn_name)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(Value::none()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && missing_ok => {
            Ok(Value::none())
        }
        Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
    }
}

pub(super) fn path_iterdir(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let entries =
        std::fs::read_dir(&path).map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
    let mut items: Vec<Value> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| PyError::from_io_error(&error, Some(&path)))?;
        let child_path = normalize_for_flavor(&entry.path().to_string_lossy(), flavor);
        items.push(make_path_result(&inst, &child_path));
    }
    Ok(Value::generator(Box::new(NativeIterFrame::generator(
        items,
    ))))
}

pub(super) fn path_glob(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let (pattern, case_sensitive) = parse_glob_args(interp, args, fn_name)?;
    let base = get_path(&inst, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let cs = case_sensitive.unwrap_or(flavor == PathFlavor::Posix);
    let result_class = Rc::clone(&inst.borrow().class);
    let items = glob_collect(&base, &pattern, &result_class, flavor, cs)?;
    Ok(Value::generator(Box::new(NativeIterFrame::generator(
        items,
    ))))
}

pub(super) fn path_rglob(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let (pattern, case_sensitive) = parse_glob_args(interp, args, fn_name)?;
    let base = get_path(&inst, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let cs = case_sensitive.unwrap_or(flavor == PathFlavor::Posix);
    let result_class = Rc::clone(&inst.borrow().class);
    let norm_pat = pattern.replace('\\', "/");
    if norm_pat.starts_with('/') {
        return Err(PyError::named(
            "NotImplementedError",
            "Non-relative patterns are unsupported".to_string(),
        ));
    }
    let recursive_pattern = if norm_pat.is_empty() || norm_pat == "." {
        "**".to_string()
    } else {
        format!("**/{norm_pat}")
    };
    let items = glob_collect(&base, &recursive_pattern, &result_class, flavor, cs)?;
    Ok(Value::generator(Box::new(NativeIterFrame::generator(
        items,
    ))))
}

fn parse_glob_args(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<(String, Option<bool>)> {
    let mut pattern_arg: Option<&Value> = None;
    let mut case_sensitive: Option<bool> = None;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            None => {
                if pattern_arg.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() takes 1 positional argument"),
                    ));
                }
                pattern_arg = Some(&arg.value);
            }
            Some("pattern") => pattern_arg = Some(&arg.value),
            Some("case_sensitive") => {
                case_sensitive = if arg.value.is_none() {
                    None
                } else {
                    Some(interp.truthy_value(&arg.value)?)
                };
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
        }
    }
    let Some(pat_val) = pattern_arg else {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing 1 required positional argument: 'pattern'"),
        ));
    };
    let pattern = match pat_val.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): pattern must be str, not {}",
                    pyrust_core::builtin_type_name(pat_val),
                ),
            ));
        }
    };
    Ok((pattern, case_sensitive))
}

fn opt_str_arg(val: &Value, fn_name: &str, arg_name: &str) -> Result<Option<String>> {
    match val.kind() {
        ValueKind::None => Ok(None),
        ValueKind::Str(s) => Ok(Some(s.to_string())),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() argument '{arg_name}' must be str or None, not {}",
                pyrust_core::builtin_type_name(val)
            ),
        )),
    }
}

fn extract_int_arg(val: &Value, fn_name: &str, arg_name: &str) -> Result<i64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() argument '{arg_name}' must be int, not {}",
                pyrust_core::builtin_type_name(val)
            ),
        )),
    }
}

fn is_truthy_flag(value: &Value) -> bool {
    matches!(value.kind(), ValueKind::Bool(true))
        || matches!(value.kind(), ValueKind::Int(number) if number != 0)
}

fn bytes_like_type_error(fn_name: &str, value: &Value) -> PyError {
    PyError::named(
        "TypeError",
        format!(
            "{fn_name}(): data must be bytes-like, not {}",
            pyrust_core::builtin_type_name(value),
        ),
    )
}

/// Collect paths under `base` matching `pattern` as `result_class` instances.
fn glob_collect(
    base: &str,
    pattern: &str,
    result_class: &Rc<RefCell<PyClass>>,
    flavor: PathFlavor,
    case_sensitive: bool,
) -> Result<Vec<Value>> {
    let norm_pattern = pattern.replace('\\', "/");
    if norm_pattern.starts_with('/') {
        return Err(PyError::named(
            "NotImplementedError",
            "Non-relative patterns are unsupported".to_string(),
        ));
    }
    let dir_only = norm_pattern.ends_with('/');
    let parts: Vec<&str> = norm_pattern
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let base_path = Path::new(base);
    let has_recursive = parts.contains(&"**");
    let mut results: Vec<PathBuf> = Vec::new();

    if has_recursive {
        let all = collect_all_descendants(base_path);
        // Include base_path itself when pattern is purely "**"
        if parts.iter().all(|p| *p == "**") && base_path.is_dir() {
            results.push(base_path.to_path_buf());
        }
        for candidate in &all {
            if dir_only && !candidate.is_dir() {
                continue;
            }
            let relative = candidate.strip_prefix(base_path).unwrap_or(candidate);
            let relative_norm = relative.to_string_lossy().replace('\\', "/");
            let rel_parts: Vec<&str> = relative_norm.split('/').filter(|p| !p.is_empty()).collect();
            if glob_parts_match_refs(&parts, &rel_parts, case_sensitive) {
                results.push(candidate.clone());
            }
        }
    } else if parts.is_empty() {
        if base_path.is_dir() {
            results.push(base_path.to_path_buf());
        }
    } else {
        let mut current_dirs: Vec<PathBuf> = vec![base_path.to_path_buf()];
        for (index, part) in parts.iter().enumerate() {
            let is_last = index == parts.len() - 1;
            let mut next: Vec<PathBuf> = Vec::new();
            for dir in &current_dirs {
                let entries = match std::fs::read_dir(dir) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_string = name.to_string_lossy();
                    if glob_component_matches(part, &name_string, case_sensitive) {
                        let child = entry.path();
                        if is_last {
                            if !dir_only || child.is_dir() {
                                results.push(child);
                            }
                        } else if child.is_dir() {
                            next.push(child);
                        }
                    }
                }
            }
            if !is_last {
                current_dirs = next;
            }
        }
    }

    Ok(results
        .into_iter()
        .map(|path| {
            let formatted = normalize_for_flavor(&path.to_string_lossy(), flavor);
            make_path_instance_for_class(Rc::clone(result_class), &formatted)
        })
        .collect())
}

fn collect_all_descendants(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let sub = collect_all_descendants(&path);
            out.push(path);
            out.extend(sub);
        } else {
            out.push(path);
        }
    }
    out
}
