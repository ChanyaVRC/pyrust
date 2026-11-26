//! Host-filesystem and interpreter-facing `Path` operations.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyIndex};
use crate::interpreter::{ExpandedCallArg, Interpreter, NativeIterFrame};
use crate::value::{PyClass, Value, ValueKind};

use super::class_registry::{
    PathFlavor, expect_self, flavor_for_class, flavor_for_instance, get_path,
    make_path_classmethod_result, make_path_instance_for_class, make_path_result,
};
use super::lexical::{
    extract_path_segment, glob_component_matches, glob_parts_match_refs, join_segments,
    normalize_for_flavor, parse_path,
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

/// `Path.home()` is `cls("~").expanduser()` in CPython 3.12, so it follows the
/// flavor's `expanduser` rules (`HOME` on POSIX; `USERPROFILE` or
/// `HOMEDRIVE`+`HOMEPATH` on Windows, where `HOME` is ignored).
pub(super) fn path_home(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let flavor = match args.first().map(|arg| arg.value.kind()) {
        Some(ValueKind::PyClass(class)) => flavor_for_class(class),
        _ => PathFlavor::host(),
    };
    let home = expand_user_component("~", flavor)?;
    make_path_classmethod_result(interp, args, home)
}

/// Expand a leading `~` / `~user` path component with the flavor's
/// `os.path.expanduser`, raising CPython's `RuntimeError` when no home
/// directory can be determined.
fn expand_user_component(component: &str, flavor: PathFlavor) -> Result<String> {
    let homedir = match flavor {
        PathFlavor::Posix => {
            crate::builtin_modules::posixpath::ops::posix_expanduser_str(component)
        }
        PathFlavor::Windows => crate::builtin_modules::ntpath::ops::nt_expanduser_str(component),
    };
    if homedir.starts_with('~') {
        return Err(PyError::named(
            "RuntimeError",
            "Could not determine home directory.".to_string(),
        ));
    }
    Ok(homedir)
}

// ── Python-level signatures (CPython-exact argument errors) ─────────────────

/// A Python-level method signature `def m(self, <positional>, *, <keyword_only>)`.
pub(super) struct MethodSig<'a> {
    /// `Path.method` — the function `__qualname__` used in error messages.
    pub(super) qualname: &'a str,
    /// Positional-or-keyword parameters after `self`.
    pub(super) positional: &'a [&'a str],
    /// How many leading `positional` parameters have no default.
    pub(super) required: usize,
    /// Keyword-only parameters (all with defaults).
    pub(super) keyword_only: &'a [&'a str],
}

/// `Path.method` for a registry name such as `pathlib.Path.method`.
pub(super) fn method_qualname(fn_name: &str) -> &str {
    fn_name.strip_prefix("pathlib.").unwrap_or(fn_name)
}

fn type_error(message: String) -> PyError {
    PyError::named("TypeError", message)
}

/// Bind the arguments after `self` to `sig` the way CPython binds a Python
/// function call: positional arguments first, then keywords, then the
/// too-many-positional and missing-argument checks. Returns one slot per
/// `positional` parameter followed by one per `keyword_only` parameter.
pub(super) fn bind_method_args<'v>(
    args: &'v [ExpandedCallArg],
    sig: &MethodSig<'_>,
) -> Result<Vec<Option<&'v Value>>> {
    let n_params = sig.positional.len() + sig.keyword_only.len();
    let mut slots: Vec<Option<&Value>> = vec![None; n_params];
    let mut given = 0usize;
    for arg in args.iter().skip(1).filter(|arg| arg.name.is_none()) {
        if given < sig.positional.len() {
            slots[given] = Some(&arg.value);
        }
        given += 1;
    }
    for arg in args.iter().skip(1) {
        let Some(name) = arg.name.as_deref() else {
            continue;
        };
        let Some(index) = sig
            .positional
            .iter()
            .chain(sig.keyword_only)
            .position(|param| *param == name)
        else {
            return Err(type_error(format!(
                "{}() got an unexpected keyword argument '{name}'",
                sig.qualname
            )));
        };
        if slots[index].is_some() {
            return Err(type_error(format!(
                "{}() got multiple values for argument '{name}'",
                sig.qualname
            )));
        }
        slots[index] = Some(&arg.value);
    }
    if given > sig.positional.len() {
        // Counts include `self`, exactly like CPython's `too_many_positional`.
        let argcount = sig.positional.len() + 1;
        let defaults = sig.positional.len() - sig.required;
        let (takes, plural) = if defaults > 0 {
            (format!("from {} to {argcount}", argcount - defaults), true)
        } else {
            (argcount.to_string(), argcount != 1)
        };
        let kwonly_given = slots[sig.positional.len()..]
            .iter()
            .filter(|slot| slot.is_some())
            .count();
        let given_total = given + 1;
        let kwonly_note = if kwonly_given > 0 {
            format!(
                " positional argument{} (and {kwonly_given} keyword-only argument{})",
                if given_total != 1 { "s" } else { "" },
                if kwonly_given != 1 { "s" } else { "" },
            )
        } else {
            String::new()
        };
        return Err(type_error(format!(
            "{}() takes {takes} positional argument{} but {given_total}{kwonly_note} {} given",
            sig.qualname,
            if plural { "s" } else { "" },
            if given_total == 1 && kwonly_given == 0 {
                "was"
            } else {
                "were"
            },
        )));
    }
    let missing: Vec<&str> = (0..sig.required)
        .filter(|&index| slots[index].is_none())
        .map(|index| sig.positional[index])
        .collect();
    if !missing.is_empty() {
        let quoted: Vec<String> = missing.iter().map(|name| format!("'{name}'")).collect();
        let names = match quoted.as_slice() {
            [only] => only.clone(),
            [first, second] => format!("{first} and {second}"),
            [init @ .., last] => format!("{}, and {last}", init.join(", ")),
            [] => String::new(),
        };
        return Err(type_error(format!(
            "{}() missing {} required positional argument{}: {names}",
            sig.qualname,
            missing.len(),
            if missing.len() == 1 { "" } else { "s" },
        )));
    }
    Ok(slots)
}

/// Reject any argument after `self` for a `def m(self)` method.
fn bind_no_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<()> {
    bind_method_args(
        args,
        &MethodSig {
            qualname: method_qualname(fn_name),
            positional: &[],
            required: 0,
            keyword_only: &[],
        },
    )
    .map(|_| ())
}

/// Convert a path-like call argument for an `os` function the way CPython's
/// `path_converter` does, with its exact `TypeError` wording.
fn os_path_arg(interp: &mut Interpreter, value: &Value, func: &str, arg: &str) -> Result<String> {
    match crate::builtin_modules::genericpath::ops::fspath_resolve(interp, value) {
        Ok(resolved) => Ok(resolved.to_lossy_string()),
        // Not path-like at all (a bad `__fspath__` result keeps its own error).
        Err(error)
            if error.class_name_is("TypeError")
                && interp.get_attr(value, "__fspath__").is_err() =>
        {
            Err(type_error(format!(
                "{func}: {arg} should be string, bytes or os.PathLike, not {}",
                pyrust_core::builtin_type_name(value),
            )))
        }
        Err(error) => Err(error),
    }
}

// ── Path.absolute / Path.expanduser ──────────────────────────────────────────

/// `Path.absolute()`: prepend the current directory without normalising or
/// resolving symlinks (unlike `resolve()`); absolute paths return `self`.
pub(super) fn path_absolute(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bind_no_args(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let parsed = parse_path(&path, flavor);
    if parsed.is_absolute(flavor) {
        return Ok(args[0].value.clone());
    }
    let cwd = if parsed.drive.is_empty() {
        let cwd = std::env::current_dir().map_err(|error| PyError::from_io_error(&error, None))?;
        let cwd = cwd.to_string_lossy().into_owned();
        // `Path()`, `Path('.')` and `Path('')` become the cwd string itself.
        if parsed.root.is_empty() && parsed.tail.is_empty() {
            return Ok(make_path_result(&inst, &cwd));
        }
        cwd
    } else {
        // There is a current directory on each Windows drive (`C:foo`).
        drive_cwd(&parsed.drive)?
    };
    Ok(make_path_result(
        &inst,
        &join_segments(&[cwd, path], flavor),
    ))
}

/// `ntpath.abspath(drive)`: the per-drive current directory.
fn drive_cwd(drive: &str) -> Result<String> {
    #[cfg(windows)]
    {
        match std::path::absolute(drive) {
            Ok(resolved) => Ok(resolved.to_string_lossy().into_owned()),
            Err(_) => crate::builtin_modules::ntpath::ops::nt_abspath_str(drive),
        }
    }
    #[cfg(not(windows))]
    {
        crate::builtin_modules::ntpath::ops::nt_abspath_str(drive)
    }
}

/// `Path.expanduser()`: expand a leading `~` / `~user` component; other
/// paths return `self`.
pub(super) fn path_expanduser(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bind_no_args(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let parsed = parse_path(&path, flavor);
    let Some(first) = parsed.tail.first() else {
        return Ok(args[0].value.clone());
    };
    if !parsed.drive.is_empty() || !parsed.root.is_empty() || !first.starts_with('~') {
        return Ok(args[0].value.clone());
    }
    let homedir = expand_user_component(first, flavor)?;
    let mut expanded = parse_path(&homedir, flavor);
    expanded.tail.extend(parsed.tail.into_iter().skip(1));
    Ok(make_path_result(&inst, &expanded.format(flavor)))
}

// ── Path.chmod / Path.lchmod ─────────────────────────────────────────────────

/// `mode` as CPython's `os.chmod` parses it (an `__index__`-able C `int`).
fn chmod_mode_arg(interp: &mut Interpreter, value: &Value, qualname: &str) -> Result<i64> {
    let index = PyIndex::try_from_value_with_interp(interp, value, qualname, "mode")?;
    index
        .as_i64()
        .and_then(|mode| i32::try_from(mode).ok())
        .map(i64::from)
        .ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        })
}

/// `Path.chmod(mode, *, follow_symlinks=True)` → `os.chmod`.
pub(super) fn path_chmod(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let qualname = method_qualname(fn_name);
    let bound = bind_method_args(
        args,
        &MethodSig {
            qualname,
            positional: &["mode"],
            required: 1,
            keyword_only: &["follow_symlinks"],
        },
    )?;
    let path = get_path(&inst, fn_name)?;
    let mode_value = bound[0].expect("required argument is bound");
    let mode = chmod_mode_arg(interp, mode_value, qualname)?;
    let follow_symlinks = match bound[1] {
        Some(value) => interp.truthy_value(value)?,
        None => true,
    };
    crate::builtin_modules::os::chmod_path(&path, mode, follow_symlinks)?;
    Ok(Value::none())
}

/// `Path.lchmod(mode)` → `self.chmod(mode, follow_symlinks=False)`.
pub(super) fn path_lchmod(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let qualname = method_qualname(fn_name);
    let bound = bind_method_args(
        args,
        &MethodSig {
            qualname,
            positional: &["mode"],
            required: 1,
            keyword_only: &[],
        },
    )?;
    let path = get_path(&inst, fn_name)?;
    let mode_value = bound[0].expect("required argument is bound");
    let mode = chmod_mode_arg(interp, mode_value, qualname)?;
    crate::builtin_modules::os::chmod_path(&path, mode, false)?;
    Ok(Value::none())
}

// ── Path.symlink_to / Path.hardlink_to ───────────────────────────────────────

/// `Path.symlink_to(target, target_is_directory=False)` →
/// `os.symlink(target, self, target_is_directory)`.
pub(super) fn path_symlink_to(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let bound = bind_method_args(
        args,
        &MethodSig {
            qualname: method_qualname(fn_name),
            positional: &["target", "target_is_directory"],
            required: 1,
            keyword_only: &[],
        },
    )?;
    let target_value = bound[0].expect("required argument is bound");
    let target = os_path_arg(interp, target_value, "symlink", "src")?;
    let link = get_path(&inst, fn_name)?;
    let target_is_directory = match bound[1] {
        Some(value) => interp.truthy_value(value)?,
        None => false,
    };
    crate::builtin_modules::os::symlink_path(&target, &link, target_is_directory)?;
    Ok(Value::none())
}

/// `Path.hardlink_to(target)` → `os.link(target, self)`.
pub(super) fn path_hardlink_to(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let bound = bind_method_args(
        args,
        &MethodSig {
            qualname: method_qualname(fn_name),
            positional: &["target"],
            required: 1,
            keyword_only: &[],
        },
    )?;
    let target_value = bound[0].expect("required argument is bound");
    let target = os_path_arg(interp, target_value, "link", "src")?;
    let link = get_path(&inst, fn_name)?;
    crate::builtin_modules::os::hard_link_path(Path::new(&target), Path::new(&link))?;
    Ok(Value::none())
}

// ── Path.is_mount / Path.is_junction ─────────────────────────────────────────

/// `Path.is_mount()` → the flavor's `os.path.ismount(self)`.
pub(super) fn path_is_mount(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bind_no_args(args, fn_name)?;
    let path = Value::string(get_path(&inst, fn_name)?);
    match flavor_for_instance(&inst) {
        PathFlavor::Posix => crate::builtin_modules::posixpath::ops::ismount(interp, &path),
        PathFlavor::Windows => crate::builtin_modules::ntpath::ops::ismount(interp, &path),
    }
}

/// `Path.is_junction()` → the flavor's `os.path.isjunction(self)`; always
/// `False` for POSIX paths.
pub(super) fn path_is_junction(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bind_no_args(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    match flavor_for_instance(&inst) {
        PathFlavor::Posix => Ok(Value::bool_(false)),
        PathFlavor::Windows => {
            crate::builtin_modules::ntpath::ops::isjunction(interp, &Value::string(path))
        }
    }
}

// ── Path.is_fifo / is_socket / is_block_device / is_char_device ──────────────

/// The `S_IFMT` file kinds probed by the `Path.is_*` stat predicates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SpecialFileKind {
    Fifo,
    Socket,
    BlockDevice,
    CharDevice,
}

/// CPython's `pathlib._ignore_error`: missing paths, non-directories, bad
/// descriptors, and symlink loops (plus Windows not-ready / invalid-name /
/// unresolvable-link codes) make a stat predicate `False`; any other
/// `OSError` propagates.
fn is_ignored_stat_error(error: &std::io::Error) -> bool {
    let Some(code) = error.raw_os_error() else {
        return false;
    };
    #[cfg(unix)]
    {
        matches!(
            code,
            libc::ENOENT | libc::ENOTDIR | libc::EBADF | libc::ELOOP
        )
    }
    #[cfg(windows)]
    {
        // Windows codes that `winerror_to_errno` maps to ENOENT, EBADF or
        // ENOTDIR, plus ERROR_NOT_READY, ERROR_INVALID_NAME and
        // ERROR_CANT_RESOLVE_FILENAME.
        matches!(
            code,
            2 | 3 | 15 | 18 | 53 | 67 | 161 | 206 | 6 | 114 | 130 | 267 | 21 | 123 | 1921
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = code;
        false
    }
}

/// `S_IS*(self.stat().st_mode)` with `_ignore_error` / `ValueError` → `False`.
pub(super) fn path_is_special_kind(
    args: &[ExpandedCallArg],
    fn_name: &str,
    kind: SpecialFileKind,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bind_no_args(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    if path.contains('\0') {
        // os.stat raises ValueError for an embedded NUL; pathlib swallows it.
        return Ok(Value::bool_(false));
    }
    match stat_special_kind(&path) {
        Ok(found) => Ok(Value::bool_(found == Some(kind))),
        Err(error) if is_ignored_stat_error(&error) => Ok(Value::bool_(false)),
        Err(error) => Err(PyError::from_io_error(&error, Some(&path))),
    }
}

/// Follow symlinks and report which special file kind `path` is, if any.
#[cfg(unix)]
fn stat_special_kind(path: &str) -> std::io::Result<Option<SpecialFileKind>> {
    use std::os::unix::fs::FileTypeExt;
    let file_type = std::fs::metadata(path)?.file_type();
    Ok(if file_type.is_fifo() {
        Some(SpecialFileKind::Fifo)
    } else if file_type.is_socket() {
        Some(SpecialFileKind::Socket)
    } else if file_type.is_block_device() {
        Some(SpecialFileKind::BlockDevice)
    } else if file_type.is_char_device() {
        Some(SpecialFileKind::CharDevice)
    } else {
        None
    })
}

/// Windows `os.stat` reports `S_IFCHR` / `S_IFIFO` for non-disk handles
/// (`NUL`, `CON`, `\\.\pipe\name`), mirroring CPython's
/// `win32_xstat_slow_impl`; on-disk files are never special.
#[cfg(windows)]
fn stat_special_kind(path: &str) -> std::io::Result<Option<SpecialFileKind>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE, FILE_TYPE_UNKNOWN, GetFileAttributesW,
        GetFileType, INVALID_FILE_ATTRIBUTES, OPEN_EXISTING,
    };
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const ERROR_CANT_ACCESS_FILE: i32 = 1920;

    let wide: Vec<u16> = std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let open = |access: u32, share: u32, flags: u32| {
        // SAFETY: `wide` is NUL-terminated and outlives the call; null
        // security attributes and template handle are permitted.
        unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                share,
                std::ptr::null(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            )
        }
    };
    let mut handle = open(FILE_READ_ATTRIBUTES, 0, FILE_FLAG_BACKUP_SEMANTICS);
    if handle == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            // CPython then reads the parent directory's entry: an on-disk file.
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION) => {
                return std::fs::metadata(path).map(|_| None);
            }
            // `\\.\con` needs read or write access.
            Some(ERROR_INVALID_PARAMETER) => {
                handle = open(
                    FILE_READ_ATTRIBUTES | GENERIC_READ,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    FILE_FLAG_BACKUP_SEMANTICS,
                );
            }
            // An unhandled reparse point is opened as itself.
            Some(ERROR_CANT_ACCESS_FILE) => {
                handle = open(
                    FILE_READ_ATTRIBUTES,
                    0,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                );
            }
            _ => return Err(error),
        }
        if handle == INVALID_HANDLE_VALUE {
            return Err(error);
        }
    }
    // SAFETY: `handle` is a valid open handle, closed exactly once below.
    let file_type = unsafe { GetFileType(handle) };
    let query_error = std::io::Error::last_os_error();
    // SAFETY: as above.
    unsafe { CloseHandle(handle) };
    if file_type == FILE_TYPE_DISK {
        return Ok(None);
    }
    if file_type == FILE_TYPE_UNKNOWN && query_error.raw_os_error().is_some_and(|code| code != 0) {
        return Err(query_error);
    }
    // SAFETY: `wide` is NUL-terminated.
    let attrs = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attrs != INVALID_FILE_ATTRIBUTES && attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
        // `\\.\pipe\` and `\\.\mailslot\` are directories.
        return Ok(None);
    }
    Ok(match file_type {
        FILE_TYPE_CHAR => Some(SpecialFileKind::CharDevice),
        FILE_TYPE_PIPE => Some(SpecialFileKind::Fifo),
        _ => None,
    })
}

#[cfg(not(any(unix, windows)))]
fn stat_special_kind(path: &str) -> std::io::Result<Option<SpecialFileKind>> {
    std::fs::metadata(path).map(|_| None)
}

// ── Path.owner / Path.group ──────────────────────────────────────────────────

/// `self.stat()` with `os.stat`'s embedded-NUL `ValueError`.
#[cfg(unix)]
fn stat_following(path: &str) -> Result<std::fs::Metadata> {
    if path.contains('\0') {
        return Err(PyError::named(
            "ValueError",
            "stat: embedded null character in path".to_string(),
        ));
    }
    std::fs::metadata(path).map_err(|error| PyError::from_io_error(&error, Some(path)))
}

/// `Path.owner()` → `pwd.getpwuid(self.stat().st_uid).pw_name`; without
/// `pwd` (Windows) CPython 3.12 raises `NotImplementedError`.
pub(super) fn path_owner(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bind_no_args(args, fn_name)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let path = get_path(&inst, fn_name)?;
        let uid = stat_following(&path)?.uid();
        crate::builtin_modules::pwd::user_name_for_uid(uid)
    }
    #[cfg(not(unix))]
    {
        let _ = inst;
        Err(PyError::named(
            "NotImplementedError",
            "Path.owner() is unsupported on this system".to_string(),
        ))
    }
}

/// `Path.group()` → `grp.getgrgid(self.stat().st_gid).gr_name`; without
/// `grp` (Windows) CPython 3.12 raises `NotImplementedError`.
pub(super) fn path_group(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    bind_no_args(args, fn_name)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let path = get_path(&inst, fn_name)?;
        let gid = stat_following(&path)?.gid();
        crate::builtin_modules::grp::group_name_for_gid(gid)
    }
    #[cfg(not(unix))]
    {
        let _ = inst;
        Err(PyError::named(
            "NotImplementedError",
            "Path.group() is unsupported on this system".to_string(),
        ))
    }
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
