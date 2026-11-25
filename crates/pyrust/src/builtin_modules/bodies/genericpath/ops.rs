//! Shared filesystem stat queries, `commonprefix`, `splitext`, and path
//! argument helpers backing `genericpath`, `posixpath`, `ntpath`, and `os.path`.

#[cfg(unix)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, value_type_name_str};
use crate::value::{Value, ValueKind};

/// Resolved `str` or `bytes` path value (after `os.fspath()`).
#[derive(Debug, Clone)]
pub(crate) enum PathValue {
    Str(String),
    Bytes(Vec<u8>),
}

impl PathValue {
    pub(crate) fn is_bytes(&self) -> bool {
        matches!(self, Self::Bytes(_))
    }

    pub(crate) fn into_value(self) -> Value {
        match self {
            Self::Str(s) => Value::string(s),
            Self::Bytes(b) => Value::bytes(b),
        }
    }

    pub(crate) fn to_path_buf(&self) -> Result<PathBuf> {
        match self {
            Self::Str(s) => {
                if s.as_bytes().contains(&0) {
                    return Err(PyError::named(
                        "ValueError",
                        "embedded null byte".to_string(),
                    ));
                }
                Ok(PathBuf::from(s))
            }
            Self::Bytes(b) => {
                if b.contains(&0) {
                    return Err(PyError::named(
                        "ValueError",
                        "embedded null byte".to_string(),
                    ));
                }
                bytes_to_path_buf(b)
            }
        }
    }

    pub(crate) fn to_lossy_string(&self) -> String {
        match self {
            Self::Str(s) => s.clone(),
            Self::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        }
    }
}

pub(crate) fn bytes_to_path_buf(bytes: &[u8]) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        let s = std::str::from_utf8(bytes).map_err(|_| {
            PyError::named(
                "UnicodeDecodeError",
                "'utf-8' codec can't decode bytes in path".to_string(),
            )
        })?;
        Ok(PathBuf::from(s))
    }
}

pub(crate) fn path_buf_to_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().as_bytes().to_vec()
    }
}

/// Resolve a `str`, `bytes`, subclass thereof, or `os.PathLike` via `__fspath__`.
pub(crate) fn fspath_resolve(interp: &mut Interpreter, value: &Value) -> Result<PathValue> {
    if let Some(pv) = extract_str_or_bytes(value) {
        return Ok(pv);
    }
    if let Ok(method) = interp.get_attr(value, "__fspath__") {
        let res = interp.call_function_expanded(method, &[])?;
        if let Some(pv) = extract_str_or_bytes(&res) {
            return Ok(pv);
        }
        return Err(PyError::named(
            "TypeError",
            format!(
                "expected {}.__fspath__() to return str or bytes, not {}",
                value_type_name_str(value),
                value_type_name_str(&res),
            ),
        ));
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "expected str, bytes or os.PathLike object, not {}",
            value_type_name_str(value),
        ),
    ))
}

fn extract_str_or_bytes(value: &Value) -> Option<PathValue> {
    match value.kind() {
        ValueKind::Str(s) => Some(PathValue::Str(s.to_string())),
        ValueKind::Bytes(b) => Some(PathValue::Bytes(b.as_ref().clone())),
        _ => {
            if let Some(backing) = crate::interpreter::builtin_data_backing(value) {
                match backing.kind() {
                    ValueKind::Str(s) => return Some(PathValue::Str(s.to_string())),
                    ValueKind::Bytes(b) => return Some(PathValue::Bytes(b.as_ref().clone())),
                    _ => {}
                }
            }
            None
        }
    }
}

/// Strip Windows extended-length verbatim prefixes (`\\?\` / `\\?\UNC\`)
/// returned by `std::fs::canonicalize` so canonical paths match CPython.
pub(crate) fn clean_canonical_path(p: &Path) -> String {
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

fn system_time_to_epoch_f64(res: std::io::Result<std::time::SystemTime>) -> f64 {
    res.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Resolve a stat target that may be a path (`str`/`bytes`/`PathLike`) or an
/// open integer fd (matching CPython's `os.stat` / `genericpath.exists`).
enum StatTarget {
    Path(PathBuf),
    Fd(i64),
}

fn resolve_stat_target(interp: &mut Interpreter, value: &Value) -> Result<StatTarget> {
    match value.kind() {
        ValueKind::Int(fd) => return Ok(StatTarget::Fd(fd)),
        ValueKind::Bool(b) => return Ok(StatTarget::Fd(i64::from(b))),
        _ => {}
    }
    let pv = fspath_resolve(interp, value)?;
    Ok(StatTarget::Path(pv.to_path_buf()?))
}

fn stat_metadata_for_target(target: &StatTarget) -> std::io::Result<std::fs::Metadata> {
    match target {
        StatTarget::Path(p) => std::fs::metadata(p),
        StatTarget::Fd(fd) => crate::builtin_modules::os::metadata_for_fd(*fd),
    }
}

pub(crate) fn exists(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let target = match resolve_stat_target(interp, path) {
        Ok(t) => t,
        Err(e) if e.class_name_is("ValueError") => return Ok(Value::bool_(false)),
        Err(e) => return Err(e),
    };
    Ok(Value::bool_(stat_metadata_for_target(&target).is_ok()))
}

pub(crate) fn lexists(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let pv = fspath_resolve(interp, path)?;
    let Ok(pb) = pv.to_path_buf() else {
        return Ok(Value::bool_(false));
    };
    Ok(Value::bool_(std::fs::symlink_metadata(&pb).is_ok()))
}

pub(crate) fn isfile(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let target = match resolve_stat_target(interp, path) {
        Ok(t) => t,
        Err(e) if e.class_name_is("ValueError") => return Ok(Value::bool_(false)),
        Err(e) => return Err(e),
    };
    Ok(Value::bool_(
        stat_metadata_for_target(&target).is_ok_and(|m| m.is_file()),
    ))
}

pub(crate) fn isdir(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let target = match resolve_stat_target(interp, path) {
        Ok(t) => t,
        Err(e) if e.class_name_is("ValueError") => return Ok(Value::bool_(false)),
        Err(e) => return Err(e),
    };
    Ok(Value::bool_(
        stat_metadata_for_target(&target).is_ok_and(|m| m.is_dir()),
    ))
}

pub(crate) fn islink(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let pv = fspath_resolve(interp, path)?;
    let Ok(pb) = pv.to_path_buf() else {
        return Ok(Value::bool_(false));
    };
    Ok(Value::bool_(
        std::fs::symlink_metadata(&pb).is_ok_and(|m| m.file_type().is_symlink()),
    ))
}

/// Generic `isjunction`: validates `os.fspath` and returns `False`.
pub(crate) fn generic_isjunction(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let _ = fspath_resolve(interp, path)?;
    Ok(Value::bool_(false))
}

/// Generic `isdevdrive`: validates `os.fspath` and returns `False`.
pub(crate) fn generic_isdevdrive(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let _ = fspath_resolve(interp, path)?;
    Ok(Value::bool_(false))
}

pub(crate) fn getsize(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let target = resolve_stat_target(interp, path)?;
    let meta = stat_metadata_for_target(&target).map_err(|e| match &target {
        StatTarget::Path(p) => PyError::from_io_error(&e, Some(&p.to_string_lossy())),
        StatTarget::Fd(_) => PyError::from_io_error(&e, None),
    })?;
    Ok(Value::int(meta.len() as i64))
}

pub(crate) fn getmtime(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let target = resolve_stat_target(interp, path)?;
    let meta = stat_metadata_for_target(&target).map_err(|e| match &target {
        StatTarget::Path(p) => PyError::from_io_error(&e, Some(&p.to_string_lossy())),
        StatTarget::Fd(_) => PyError::from_io_error(&e, None),
    })?;
    Ok(Value::float(system_time_to_epoch_f64(meta.modified())))
}

pub(crate) fn getatime(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let target = resolve_stat_target(interp, path)?;
    let meta = stat_metadata_for_target(&target).map_err(|e| match &target {
        StatTarget::Path(p) => PyError::from_io_error(&e, Some(&p.to_string_lossy())),
        StatTarget::Fd(_) => PyError::from_io_error(&e, None),
    })?;
    Ok(Value::float(system_time_to_epoch_f64(meta.accessed())))
}

pub(crate) fn getctime(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let target = resolve_stat_target(interp, path)?;
    let meta = stat_metadata_for_target(&target).map_err(|e| match &target {
        StatTarget::Path(p) => PyError::from_io_error(&e, Some(&p.to_string_lossy())),
        StatTarget::Fd(_) => PyError::from_io_error(&e, None),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let secs = meta.ctime() as f64 + (meta.ctime_nsec() as f64) / 1_000_000_000.0;
        Ok(Value::float(secs))
    }
    #[cfg(not(unix))]
    {
        Ok(Value::float(system_time_to_epoch_f64(meta.created())))
    }
}

pub(crate) fn samestat(interp: &mut Interpreter, s1: &Value, s2: &Value) -> Result<Value> {
    let ino1 = interp.get_attr(s1, "st_ino")?;
    let ino2 = interp.get_attr(s2, "st_ino")?;
    let dev1 = interp.get_attr(s1, "st_dev")?;
    let dev2 = interp.get_attr(s2, "st_dev")?;
    Ok(Value::bool_(ino1 == ino2 && dev1 == dev2))
}

pub(crate) fn samefile(interp: &mut Interpreter, f1: &Value, f2: &Value) -> Result<Value> {
    let t1 = resolve_stat_target(interp, f1)?;
    let t2 = resolve_stat_target(interp, f2)?;
    let s1 = match &t1 {
        StatTarget::Path(p) => crate::builtin_modules::os::stat_path_result(p)?,
        StatTarget::Fd(fd) => crate::builtin_modules::os::fstat_fd_result(*fd)?,
    };
    let s2 = match &t2 {
        StatTarget::Path(p) => crate::builtin_modules::os::stat_path_result(p)?,
        StatTarget::Fd(fd) => crate::builtin_modules::os::fstat_fd_result(*fd)?,
    };
    samestat(interp, &s1, &s2)
}

pub(crate) fn sameopenfile(interp: &mut Interpreter, fp1: i64, fp2: i64) -> Result<Value> {
    let s1 = crate::builtin_modules::os::fstat_fd_result(fp1)?;
    let s2 = crate::builtin_modules::os::fstat_fd_result(fp2)?;
    samestat(interp, &s1, &s2)
}

pub(crate) fn commonprefix(interp: &mut Interpreter, seq: &Value) -> Result<Value> {
    let items = interp.collect_iterable(seq)?;
    if items.is_empty() {
        return Ok(Value::string(String::new()));
    }
    // CPython genericpath.commonprefix also accepts sequences of lists or tuples.
    if matches!(items[0].kind(), ValueKind::List(_) | ValueKind::Tuple(_)) {
        let is_tuple = matches!(items[0].kind(), ValueKind::Tuple(_));
        let mut seqs: Vec<Vec<Value>> = Vec::with_capacity(items.len());
        for it in &items {
            seqs.push(interp.collect_iterable(it)?);
        }
        // Compute min and max lexicographically via Python comparison.
        let mut s1 = seqs[0].clone();
        let mut s2 = seqs[0].clone();
        for cand in &seqs[1..] {
            let cand_val = if is_tuple {
                Value::tuple(cand.clone())
            } else {
                Value::list(cand.clone())
            };
            let s1_val = if is_tuple {
                Value::tuple(s1.clone())
            } else {
                Value::list(s1.clone())
            };
            let s2_val = if is_tuple {
                Value::tuple(s2.clone())
            } else {
                Value::list(s2.clone())
            };
            if crate::interpreter::compare_values(&cand_val, &s1_val)? == std::cmp::Ordering::Less {
                s1 = cand.clone();
            }
            if crate::interpreter::compare_values(&cand_val, &s2_val)?
                == std::cmp::Ordering::Greater
            {
                s2 = cand.clone();
            }
        }
        let mut prefix_len = 0;
        for (a, b) in s1.iter().zip(s2.iter()) {
            if a != b {
                break;
            }
            prefix_len += 1;
        }
        let out = s1[..prefix_len].to_vec();
        return Ok(if is_tuple {
            Value::tuple(out)
        } else {
            Value::list(out)
        });
    }

    let first = fspath_resolve(interp, &items[0])?;
    match first {
        PathValue::Bytes(_) => {
            let mut all_bytes: Vec<Vec<u8>> = Vec::with_capacity(items.len());
            for it in &items {
                match fspath_resolve(interp, it)? {
                    PathValue::Bytes(b) => all_bytes.push(b),
                    PathValue::Str(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                }
            }
            let min = all_bytes.iter().min().unwrap();
            let max = all_bytes.iter().max().unwrap();
            let prefix_len = min
                .iter()
                .zip(max.iter())
                .take_while(|(a, b)| a == b)
                .count();
            Ok(Value::bytes(min[..prefix_len].to_vec()))
        }
        PathValue::Str(_) => {
            let mut strs: Vec<String> = Vec::with_capacity(items.len());
            for it in &items {
                match fspath_resolve(interp, it)? {
                    PathValue::Str(s) => strs.push(s),
                    PathValue::Bytes(_) => {
                        return Err(PyError::named(
                            "TypeError",
                            "Can't mix strings and bytes in path components".to_string(),
                        ));
                    }
                }
            }
            let min = strs.iter().min().unwrap();
            let max = strs.iter().max().unwrap();
            let prefix_len = min
                .chars()
                .zip(max.chars())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a.len_utf8())
                .sum::<usize>();
            Ok(Value::string(&min[..prefix_len]))
        }
    }
}

/// Shared `splitext` over `str` with separator `sep`, optional `altsep`, and `extsep`.
pub(crate) fn splitext_str(
    path: &str,
    sep: char,
    altsep: Option<char>,
    extsep: char,
) -> (String, String) {
    let sep_idx = path.rfind(sep).map(|i| i as isize).unwrap_or(-1);
    let sep_idx = match altsep {
        Some(alt) => sep_idx.max(path.rfind(alt).map(|i| i as isize).unwrap_or(-1)),
        None => sep_idx,
    };
    let dot_idx = path.rfind(extsep).map(|i| i as isize).unwrap_or(-1);
    if dot_idx > sep_idx {
        let filename_start = (sep_idx + 1) as usize;
        let dot_u = dot_idx as usize;
        let mut cursor = filename_start;
        let ext_len = extsep.len_utf8();
        while cursor < dot_u {
            if !path[cursor..].starts_with(extsep) {
                return (path[..dot_u].to_string(), path[dot_u..].to_string());
            }
            cursor += ext_len;
        }
    }
    (path.to_string(), String::new())
}

/// Shared `splitext` over `bytes` with separator `sep`, optional `altsep`, and `extsep`.
pub(crate) fn splitext_bytes(
    path: &[u8],
    sep: u8,
    altsep: Option<u8>,
    extsep: u8,
) -> (Vec<u8>, Vec<u8>) {
    let sep_idx = path
        .iter()
        .rposition(|&b| b == sep)
        .map(|i| i as isize)
        .unwrap_or(-1);
    let sep_idx = match altsep {
        Some(alt) => sep_idx.max(
            path.iter()
                .rposition(|&b| b == alt)
                .map(|i| i as isize)
                .unwrap_or(-1),
        ),
        None => sep_idx,
    };
    let dot_idx = path
        .iter()
        .rposition(|&b| b == extsep)
        .map(|i| i as isize)
        .unwrap_or(-1);
    if dot_idx > sep_idx {
        let filename_start = (sep_idx + 1) as usize;
        let dot_u = dot_idx as usize;
        let mut cursor = filename_start;
        while cursor < dot_u {
            if path[cursor] != extsep {
                return (path[..dot_u].to_vec(), path[dot_u..].to_vec());
            }
            cursor += 1;
        }
    }
    (path.to_vec(), Vec::new())
}

pub(crate) fn splitext_value(pv: PathValue, sep: u8, altsep: Option<u8>, extsep: u8) -> Value {
    match pv {
        PathValue::Str(s) => {
            let (root, ext) =
                splitext_str(&s, sep as char, altsep.map(|b| b as char), extsep as char);
            Value::tuple(vec![Value::string(root), Value::string(ext)])
        }
        PathValue::Bytes(b) => {
            let (root, ext) = splitext_bytes(&b, sep, altsep, extsep);
            Value::tuple(vec![Value::bytes(root), Value::bytes(ext)])
        }
    }
}

/// CPython `genericpath._splitext(p, sep, altsep, extsep)` supporting `str` and `bytes`.
pub(crate) fn generic_splitext_py(
    p: &Value,
    sep: &Value,
    altsep: &Value,
    extsep: &Value,
) -> Result<Value> {
    match (p.kind(), sep.kind(), extsep.kind()) {
        (ValueKind::Str(ps), ValueKind::Str(ss), ValueKind::Str(es)) => {
            let sep_ch = ss
                .chars()
                .next()
                .ok_or_else(|| PyError::named("ValueError", "empty separator".to_string()))?;
            let ext_ch = es.chars().next().ok_or_else(|| {
                PyError::named("ValueError", "empty extension separator".to_string())
            })?;
            let alt_ch = match altsep.kind() {
                ValueKind::None => None,
                ValueKind::Str(a) => a.chars().next(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "altsep must be str or None".to_string(),
                    ));
                }
            };
            let (root, ext) = splitext_str(ps, sep_ch, alt_ch, ext_ch);
            Ok(Value::tuple(vec![Value::string(root), Value::string(ext)]))
        }
        (ValueKind::Bytes(pb), ValueKind::Bytes(sb), ValueKind::Bytes(eb)) => {
            let sep_b = *sb
                .first()
                .ok_or_else(|| PyError::named("ValueError", "empty separator".to_string()))?;
            let ext_b = *eb.first().ok_or_else(|| {
                PyError::named("ValueError", "empty extension separator".to_string())
            })?;
            let alt_b = match altsep.kind() {
                ValueKind::None => None,
                ValueKind::Bytes(a) => a.first().copied(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "altsep must be bytes or None".to_string(),
                    ));
                }
            };
            let (root, ext) = splitext_bytes(pb, sep_b, alt_b, ext_b);
            Ok(Value::tuple(vec![Value::bytes(root), Value::bytes(ext)]))
        }
        _ => Err(PyError::named(
            "TypeError",
            "_splitext() arguments must be all str or all bytes".to_string(),
        )),
    }
}

/// CPython `genericpath._check_arg_types(funcname, *args)`.
pub(crate) fn check_arg_types(funcname: &str, args: &[Value]) -> Result<Value> {
    let mut has_str = false;
    let mut has_bytes = false;
    for s in args {
        match s.kind() {
            ValueKind::Str(_) => has_str = true,
            ValueKind::Bytes(_) => has_bytes = true,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{funcname}() argument must be str, bytes, or os.PathLike object, not '{}'",
                        value_type_name_str(s),
                    ),
                ));
            }
        }
    }
    if has_str && has_bytes {
        return Err(PyError::named(
            "TypeError",
            "Can't mix strings and bytes in path components".to_string(),
        ));
    }
    Ok(Value::none())
}
