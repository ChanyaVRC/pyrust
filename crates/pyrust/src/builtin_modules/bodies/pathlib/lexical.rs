//! POSIX and Windows lexical parsing, normalization, component access,
//! derived paths, `relative_to`, `match`, and `_PathParents` indexing/slicing.
//!
//! This module deliberately does not touch the host filesystem. Operations
//! preserve the receiver's concrete class through `make_path_result`.

use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, NativeIterFrame};
use crate::value::{Value, ValueKind};

use super::class_registry::{
    PathFlavor, expect_self, flavor_for_instance, get_path, is_path_instance,
    make_parents_instance, make_path_result,
};

/// Parsed lexical components of a path for either POSIX or Windows flavor.
pub(super) struct ParsedPath {
    pub drive: String,
    pub root: String,
    pub tail: Vec<String>,
}

impl ParsedPath {
    pub fn anchor(&self) -> String {
        format!("{}{}", self.drive, self.root)
    }

    /// `PurePath.is_absolute()`: a root, plus a drive for the Windows flavor.
    pub fn is_absolute(&self, flavor: PathFlavor) -> bool {
        match flavor {
            PathFlavor::Posix => !self.root.is_empty(),
            PathFlavor::Windows => !self.drive.is_empty() && !self.root.is_empty(),
        }
    }

    pub fn format(&self, flavor: PathFlavor) -> String {
        let sep = match flavor {
            PathFlavor::Posix => "/",
            PathFlavor::Windows => "\\",
        };
        if self.drive.is_empty() && self.root.is_empty() && self.tail.is_empty() {
            ".".to_string()
        } else {
            format!("{}{}{}", self.drive, self.root, self.tail.join(sep))
        }
    }
}

/// `(drive, root, tail)` of the separator-normalised Windows path `p`.
///
/// Normalises `/` to `\` and splits at `ntpath.splitroot`'s byte offsets, so
/// a lone surrogate in a path component is never decoded into an invalid
/// `char` (#3315).
fn win_splitroot(p: &str) -> (String, String, String) {
    let normp = p.replace('/', "\\");
    let (drive_end, root_end) = crate::builtin_modules::ntpath::ops::nt_splitroot_offsets(&normp);
    (
        normp[..drive_end].to_string(),
        normp[drive_end..root_end].to_string(),
        normp[root_end..].to_string(),
    )
}

pub(super) fn parse_path(path: &str, flavor: PathFlavor) -> ParsedPath {
    match flavor {
        PathFlavor::Posix => {
            let (root, rest) = if path.starts_with("//") && !path.starts_with("///") {
                ("//", &path[2..])
            } else if let Some(rest) = path.strip_prefix('/') {
                ("/", rest)
            } else {
                ("", path)
            };
            let tail = rest
                .split('/')
                .filter(|part| !part.is_empty() && *part != ".")
                .map(str::to_string)
                .collect();
            ParsedPath {
                drive: String::new(),
                root: root.to_string(),
                tail,
            }
        }
        PathFlavor::Windows => {
            let (drive, root, rest) = win_splitroot(path);
            let tail = rest
                .split('\\')
                .filter(|part| !part.is_empty() && *part != ".")
                .map(str::to_string)
                .collect();
            ParsedPath { drive, root, tail }
        }
    }
}

/// Normalize a POSIX path string the same way CPython's `pathlib` does.
pub(super) fn normalize_path(path: &str) -> String {
    parse_path(path, PathFlavor::Posix).format(PathFlavor::Posix)
}

pub(super) fn normalize_for_flavor(path: &str, flavor: PathFlavor) -> String {
    parse_path(path, flavor).format(flavor)
}

/// Join segments using flavor-aware path joining rules (matching CPython).
pub(super) fn join_segments(segments: &[String], flavor: PathFlavor) -> String {
    if segments.is_empty() {
        return ".".to_string();
    }
    match flavor {
        PathFlavor::Posix => {
            let mut acc = String::new();
            for seg in segments {
                if seg.starts_with('/') {
                    acc.clear();
                    acc.push_str(seg);
                } else if !seg.is_empty() {
                    if !acc.is_empty() && !acc.ends_with('/') {
                        acc.push('/');
                    }
                    acc.push_str(seg);
                }
            }
            if acc.is_empty() {
                ".".to_string()
            } else {
                normalize_path(&acc)
            }
        }
        PathFlavor::Windows => {
            let mut cur_drive = String::new();
            let mut cur_root = String::new();
            let mut cur_tail: Vec<String> = Vec::new();
            for seg in segments {
                if seg.is_empty() {
                    continue;
                }
                let p = parse_path(seg, PathFlavor::Windows);
                if !p.root.is_empty() {
                    if !p.drive.is_empty() || cur_drive.is_empty() {
                        cur_drive = p.drive;
                    }
                    cur_root = p.root;
                    cur_tail = p.tail;
                } else if !p.drive.is_empty() {
                    if p.drive.to_lowercase() != cur_drive.to_lowercase() {
                        cur_drive = p.drive;
                        cur_root.clear();
                        cur_tail = p.tail;
                    } else {
                        cur_drive = p.drive;
                        cur_tail.extend(p.tail);
                    }
                } else {
                    cur_tail.extend(p.tail);
                }
            }
            ParsedPath {
                drive: cur_drive,
                root: cur_root,
                tail: cur_tail,
            }
            .format(PathFlavor::Windows)
        }
    }
}

/// Extract a path segment string from `str`, `PurePath`/`Path`, or any `os.PathLike`.
pub(super) fn extract_path_segment(
    interp: &mut Interpreter,
    value: &Value,
    fn_name: &str,
) -> Result<String> {
    match value.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        ValueKind::PyInstance(other) if is_path_instance(other) => {
            get_path(&Rc::clone(other), fn_name)
        }
        _ => {
            if let Ok(method) = interp.get_attr(value, "__fspath__") {
                let res = interp.call_function_expanded(method, &[])?;
                if let ValueKind::Str(s) = res.kind() {
                    return Ok(s.to_string());
                }
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "expected {}.__fspath__() to return str, not {}",
                        pyrust_core::builtin_type_name(value),
                        pyrust_core::builtin_type_name(&res),
                    ),
                ));
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "argument should be a str or an os.PathLike object where __fspath__ returns a str, not '{}'",
                    pyrust_core::builtin_type_name(value),
                ),
            ))
        }
    }
}

pub(super) fn path_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let mut segments: Vec<String> = Vec::with_capacity(args.len().saturating_sub(1));
    for arg in args.iter().skip(1) {
        if arg.name.is_some() {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() takes no keyword arguments"),
            ));
        }
        segments.push(extract_path_segment(interp, &arg.value, fn_name)?);
    }
    let joined = join_segments(&segments, flavor);
    inst.borrow_mut()
        .attrs
        .insert("_path", Value::string(&joined));
    Ok(Value::none())
}

pub(super) fn path_truediv(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let lhs = get_path(&inst, fn_name)?;
    let rhs = match extract_path_segment_opt(interp, &args[1].value, fn_name)? {
        Some(s) => s,
        None => return Ok(Value::not_implemented()),
    };
    let joined = join_segments(&[lhs, rhs], flavor);
    Ok(make_path_result(&inst, &joined))
}

pub(super) fn path_rtruediv(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let rhs = get_path(&inst, fn_name)?;
    let lhs = match extract_path_segment_opt(interp, &args[1].value, fn_name)? {
        Some(s) => s,
        None => return Ok(Value::not_implemented()),
    };
    let joined = join_segments(&[lhs, rhs], flavor);
    Ok(make_path_result(&inst, &joined))
}

fn extract_path_segment_opt(
    interp: &mut Interpreter,
    value: &Value,
    fn_name: &str,
) -> Result<Option<String>> {
    match value.kind() {
        ValueKind::Str(s) => Ok(Some(s.to_string())),
        ValueKind::PyInstance(other) if is_path_instance(other) => {
            Ok(Some(get_path(&Rc::clone(other), fn_name)?))
        }
        _ => {
            if let Ok(method) = interp.get_attr(value, "__fspath__") {
                let res = interp.call_function_expanded(method, &[])?;
                if let ValueKind::Str(s) = res.kind() {
                    return Ok(Some(s.to_string()));
                }
            }
            Ok(None)
        }
    }
}

pub(super) fn path_joinpath(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let mut segments = Vec::with_capacity(args.len());
    segments.push(get_path(&inst, fn_name)?);
    for arg in args.iter().skip(1) {
        segments.push(extract_path_segment(interp, &arg.value, fn_name)?);
    }
    let joined = join_segments(&segments, flavor);
    Ok(make_path_result(&inst, &joined))
}

pub(super) fn path_as_posix(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let path = get_path(&inst, fn_name)?;
    match flavor {
        PathFlavor::Posix => Ok(Value::string(path)),
        PathFlavor::Windows => Ok(Value::string(path.replace('\\', "/"))),
    }
}

pub(super) fn path_drive(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor_for_instance(&inst));
    Ok(Value::string(parsed.drive))
}

pub(super) fn path_root(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor_for_instance(&inst));
    Ok(Value::string(parsed.root))
}

pub(super) fn path_anchor(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor_for_instance(&inst));
    Ok(Value::string(parsed.anchor()))
}

pub(super) fn path_name(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor_for_instance(&inst));
    Ok(Value::string(
        parsed.tail.last().cloned().unwrap_or_default(),
    ))
}

pub(super) fn path_parent(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let mut parsed = parse_path(&get_path(&inst, fn_name)?, flavor);
    if !parsed.tail.is_empty() {
        parsed.tail.pop();
    }
    Ok(make_path_result(&inst, &parsed.format(flavor)))
}

pub(super) fn path_parents(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let mut parsed = parse_path(&get_path(&inst, fn_name)?, flavor);
    let mut parents = Vec::with_capacity(parsed.tail.len());
    while !parsed.tail.is_empty() {
        parsed.tail.pop();
        parents.push(make_path_result(&inst, &parsed.format(flavor)));
    }
    Ok(make_parents_instance(&inst, parents))
}

/// CPython 3.12 `PurePath.suffixes` helper.
fn compute_suffixes(name: &str) -> Vec<String> {
    if name.ends_with('.') {
        return Vec::new();
    }
    let stripped = name.trim_start_matches('.');
    stripped
        .split('.')
        .skip(1)
        .map(|part| format!(".{part}"))
        .collect()
}

fn compute_suffix(name: &str) -> &str {
    if name.ends_with('.') {
        return "";
    }
    let stripped_offset = name.len() - name.trim_start_matches('.').len();
    let rest = &name[stripped_offset..];
    match rest.rfind('.') {
        Some(idx) if idx > 0 => &rest[idx..],
        _ => "",
    }
}

fn compute_stem(name: &str) -> &str {
    let suf = compute_suffix(name);
    if suf.is_empty() {
        name
    } else {
        &name[..name.len() - suf.len()]
    }
}

pub(super) fn path_stem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor_for_instance(&inst));
    let name = parsed.tail.last().map(String::as_str).unwrap_or("");
    Ok(Value::string(compute_stem(name)))
}

pub(super) fn path_suffix(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor_for_instance(&inst));
    let name = parsed.tail.last().map(String::as_str).unwrap_or("");
    Ok(Value::string(compute_suffix(name)))
}

pub(super) fn path_suffixes(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor_for_instance(&inst));
    let name = parsed.tail.last().map(String::as_str).unwrap_or("");
    let suffixes = compute_suffixes(name)
        .into_iter()
        .map(Value::string)
        .collect();
    Ok(Value::list(suffixes))
}

pub(super) fn path_parts(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor_for_instance(&inst));
    let anchor = parsed.anchor();
    let mut components: Vec<Value> = Vec::with_capacity(parsed.tail.len() + 1);
    if !anchor.is_empty() {
        components.push(Value::string(anchor));
    }
    for part in parsed.tail {
        components.push(Value::string(part));
    }
    Ok(Value::tuple(components))
}

pub(super) fn path_is_absolute(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let parsed = parse_path(&get_path(&inst, fn_name)?, flavor);
    Ok(Value::bool_(parsed.is_absolute(flavor)))
}

fn is_invalid_name(name: &str, flavor: PathFlavor) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return true;
    }
    match flavor {
        PathFlavor::Posix => name.contains('/'),
        PathFlavor::Windows => name.contains('/') || name.contains('\\') || name.contains(':'),
    }
}

pub(super) fn path_with_name(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let name = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): name must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            ));
        }
    };
    let path = get_path(&inst, fn_name)?;
    let mut parsed = parse_path(&path, flavor);
    if parsed.tail.is_empty() {
        return Err(empty_name_error(&inst, &path));
    }
    if is_invalid_name(&name, flavor) {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid name '{name}'"),
        ));
    }
    *parsed.tail.last_mut().unwrap() = name;
    Ok(make_path_result(&inst, &parsed.format(flavor)))
}

pub(super) fn path_with_stem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let new_stem = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): stem must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            ));
        }
    };
    let path = get_path(&inst, fn_name)?;
    let mut parsed = parse_path(&path, flavor);
    let Some(cur_name) = parsed.tail.last() else {
        return Err(empty_name_error(&inst, &path));
    };
    let suf = compute_suffix(cur_name).to_string();
    let new_name = format!("{new_stem}{suf}");
    if is_invalid_name(&new_name, flavor) {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid name '{new_name}'"),
        ));
    }
    *parsed.tail.last_mut().unwrap() = new_name;
    Ok(make_path_result(&inst, &parsed.format(flavor)))
}

pub(super) fn path_with_suffix(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let new_suffix = match args[1].value.kind() {
        ValueKind::Str(value) => value.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}(): suffix must be str, not {}",
                    pyrust_core::builtin_type_name(&args[1].value),
                ),
            ));
        }
    };
    let has_bad_sep = match flavor {
        PathFlavor::Posix => new_suffix.contains('/'),
        PathFlavor::Windows => new_suffix.contains('/') || new_suffix.contains('\\'),
    };
    if !new_suffix.is_empty() && (!new_suffix.starts_with('.') || new_suffix == "." || has_bad_sep)
    {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid suffix '{new_suffix}'"),
        ));
    }
    let path = get_path(&inst, fn_name)?;
    let mut parsed = parse_path(&path, flavor);
    let Some(cur_name) = parsed.tail.last() else {
        return Err(empty_name_error(&inst, &path));
    };
    let new_name = format!("{}{new_suffix}", compute_stem(cur_name));
    *parsed.tail.last_mut().unwrap() = new_name;
    Ok(make_path_result(&inst, &parsed.format(flavor)))
}

fn parts_equal(a: &str, b: &str, flavor: PathFlavor) -> bool {
    match flavor {
        PathFlavor::Posix => a == b,
        PathFlavor::Windows => a.to_lowercase() == b.to_lowercase(),
    }
}

pub(super) fn path_relative_to(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let self_path = get_path(&inst, fn_name)?;

    let mut other_segments: Vec<String> = Vec::new();
    let mut walk_up = false;
    let mut seen_walk_up = false;
    for arg in args.iter().skip(1) {
        match arg.name.as_deref() {
            None => other_segments.push(extract_path_segment(interp, &arg.value, fn_name)?),
            Some("walk_up") => {
                if seen_walk_up {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'walk_up'"),
                    ));
                }
                walk_up = interp.truthy_value(&arg.value)?;
                seen_walk_up = true;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
        }
    }
    if other_segments.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing 1 required positional argument: 'other'"),
        ));
    }

    let other_path = join_segments(&other_segments, flavor);
    let p_self = parse_path(&self_path, flavor);
    let p_other = parse_path(&other_path, flavor);

    let same_anchor = parts_equal(&p_self.drive, &p_other.drive, flavor)
        && parts_equal(&p_self.root, &p_other.root, flavor);
    if !same_anchor {
        if walk_up {
            return Err(PyError::named(
                "ValueError",
                format!("'{self_path}' and '{other_path}' have different anchors"),
            ));
        }
        return Err(PyError::named(
            "ValueError",
            format!("'{self_path}' is not in the subpath of '{other_path}'"),
        ));
    }

    if !walk_up {
        if p_self.tail.len() < p_other.tail.len()
            || !p_self
                .tail
                .iter()
                .zip(p_other.tail.iter())
                .all(|(a, b)| parts_equal(a, b, flavor))
        {
            return Err(PyError::named(
                "ValueError",
                format!("'{self_path}' is not in the subpath of '{other_path}'"),
            ));
        }
        let rel = ParsedPath {
            drive: String::new(),
            root: String::new(),
            tail: p_self.tail[p_other.tail.len()..].to_vec(),
        };
        return Ok(make_path_result(&inst, &rel.format(flavor)));
    }

    // walk_up=True: walk up via `..` from common prefix.
    let common = p_self
        .tail
        .iter()
        .zip(p_other.tail.iter())
        .take_while(|(a, b)| parts_equal(a, b, flavor))
        .count();
    if p_other.tail[common..].iter().any(|part| part == "..") {
        return Err(PyError::named(
            "ValueError",
            format!("'..' segment in '{other_path}' cannot be walked"),
        ));
    }
    let mut rel_tail: Vec<String> = vec!["..".to_string(); p_other.tail.len() - common];
    rel_tail.extend_from_slice(&p_self.tail[common..]);
    let rel = ParsedPath {
        drive: String::new(),
        root: String::new(),
        tail: rel_tail,
    };
    Ok(make_path_result(&inst, &rel.format(flavor)))
}

pub(super) fn path_is_relative_to(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let self_path = get_path(&inst, fn_name)?;

    let mut other_segments: Vec<String> = Vec::new();
    for arg in args.iter().skip(1) {
        if arg.name.is_some() {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() takes no keyword arguments"),
            ));
        }
        other_segments.push(extract_path_segment(interp, &arg.value, fn_name)?);
    }
    if other_segments.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing 1 required positional argument: 'other'"),
        ));
    }

    let other_path = join_segments(&other_segments, flavor);
    let p_self = parse_path(&self_path, flavor);
    let p_other = parse_path(&other_path, flavor);

    let matched = parts_equal(&p_self.drive, &p_other.drive, flavor)
        && parts_equal(&p_self.root, &p_other.root, flavor)
        && p_self.tail.len() >= p_other.tail.len()
        && p_self
            .tail
            .iter()
            .zip(p_other.tail.iter())
            .all(|(a, b)| parts_equal(a, b, flavor));
    Ok(Value::bool_(matched))
}

pub(super) fn path_match(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let self_path = get_path(&inst, fn_name)?;

    let mut pattern_arg: Option<&Value> = None;
    let mut case_sensitive: Option<bool> = None;
    let mut seen_cs = false;
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
            Some("path_pattern") => {
                if pattern_arg.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'path_pattern'"),
                    ));
                }
                pattern_arg = Some(&arg.value);
            }
            Some("case_sensitive") => {
                if seen_cs {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'case_sensitive'"),
                    ));
                }
                case_sensitive = if arg.value.is_none() {
                    None
                } else {
                    Some(interp.truthy_value(&arg.value)?)
                };
                seen_cs = true;
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
            format!("{fn_name}() missing 1 required positional argument: 'path_pattern'"),
        ));
    };
    let pat_str = extract_path_segment(interp, pat_val, fn_name)?;
    if pat_str.is_empty() {
        return Err(PyError::named("ValueError", "empty pattern".to_string()));
    }

    let cs = case_sensitive.unwrap_or(flavor == PathFlavor::Posix);
    let p_self = parse_path(&self_path, flavor);
    let p_pat = parse_path(&pat_str, flavor);

    let pat_has_anchor = !p_pat.drive.is_empty() || !p_pat.root.is_empty();
    let matched = if pat_has_anchor {
        let drive_ok = glob_component_matches(&p_pat.drive, &p_self.drive, cs);
        let root_ok = glob_component_matches(&p_pat.root, &p_self.root, cs);
        drive_ok && root_ok && glob_parts_match_slices(&p_pat.tail, &p_self.tail, cs)
    } else {
        // Relative pattern matches right-aligned against any trailing suffix.
        let mut ok = false;
        for start in 0..=p_self.tail.len() {
            if glob_parts_match_slices(&p_pat.tail, &p_self.tail[start..], cs) {
                ok = true;
                break;
            }
        }
        ok
    };
    Ok(Value::bool_(matched))
}

pub(super) fn glob_parts_match_slices(
    pattern: &[String],
    path: &[String],
    case_sensitive: bool,
) -> bool {
    let p_refs: Vec<&str> = pattern.iter().map(String::as_str).collect();
    let v_refs: Vec<&str> = path.iter().map(String::as_str).collect();
    glob_parts_match_refs(&p_refs, &v_refs, case_sensitive)
}

pub(super) fn glob_parts_match_refs(pattern: &[&str], path: &[&str], case_sensitive: bool) -> bool {
    match (pattern.first(), path.first()) {
        (None, None) => true,
        (None, _) => false,
        (Some(&"**"), _) => {
            let rest = &pattern[1..];
            for skip in 0..=path.len() {
                if glob_parts_match_refs(rest, &path[skip..], case_sensitive) {
                    return true;
                }
            }
            false
        }
        (Some(_), None) => pattern.iter().all(|part| *part == "**"),
        (Some(component), Some(value)) => {
            glob_component_matches(component, value, case_sensitive)
                && glob_parts_match_refs(&pattern[1..], &path[1..], case_sensitive)
        }
    }
}

pub(super) fn glob_component_matches(pattern: &str, name: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        glob_match(pattern.as_bytes(), name.as_bytes())
    } else {
        let p = pattern.to_lowercase();
        let n = name.to_lowercase();
        glob_match(p.as_bytes(), n.as_bytes())
    }
}

fn glob_match(pattern: &[u8], value: &[u8]) -> bool {
    match (pattern.first(), value.first()) {
        (None, None) => true,
        (None, _) => false,
        (Some(b'*'), _) => {
            for skip in 0..=value.len() {
                if glob_match(&pattern[1..], &value[skip..]) {
                    return true;
                }
            }
            false
        }
        (Some(b'?'), Some(_)) => glob_match(&pattern[1..], &value[1..]),
        (Some(b'?'), None) => false,
        (Some(b'['), _) => {
            let close = pattern[1..].iter().position(|&byte| byte == b']');
            let close = match close {
                Some(index) => index + 1,
                None => {
                    if value.first() == Some(&b'[') {
                        return glob_match(&pattern[1..], &value[1..]);
                    }
                    return false;
                }
            };
            let class = &pattern[1..close];
            let rest = &pattern[close + 1..];
            let character = match value.first() {
                Some(&character) => character,
                None => return false,
            };
            char_class_matches(class, character) && glob_match(rest, &value[1..])
        }
        (Some(&pattern_byte), Some(&value_byte)) => {
            pattern_byte == value_byte && glob_match(&pattern[1..], &value[1..])
        }
        (Some(_), None) => false,
    }
}

fn char_class_matches(class: &[u8], character: u8) -> bool {
    let (negated, class) = if class.len() > 1 && class[0] == b'!' {
        (true, &class[1..])
    } else {
        (false, class)
    };

    let mut index = 0;
    let mut matched = false;
    while index < class.len() {
        if index + 2 < class.len() && class[index + 1] == b'-' {
            if character >= class[index] && character <= class[index + 2] {
                matched = true;
            }
            index += 3;
        } else {
            if class[index] == character {
                matched = true;
            }
            index += 1;
        }
    }

    if negated { !matched } else { matched }
}

fn empty_name_error(inst: &Rc<RefCell<crate::value::PyInstance>>, path: &str) -> PyError {
    let class_name = inst.borrow().class.borrow().name.clone();
    PyError::named(
        "ValueError",
        format!("{class_name}('{path}') has an empty name"),
    )
}

// ---------------------------------------------------------------------------
// `_PathParents` sequence methods
// ---------------------------------------------------------------------------

fn get_parents_items(inst: &Rc<RefCell<crate::value::PyInstance>>) -> Result<Vec<Value>> {
    match inst.borrow().attrs.get("_items").map(|v| v.kind()) {
        Some(ValueKind::Tuple(items)) => Ok(items.to_vec()),
        _ => Err(PyError::runtime(
            "internal: _PathParents missing _items".to_string(),
        )),
    }
}

pub(super) fn parents_len(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let items = get_parents_items(&inst)?;
    Ok(Value::int(items.len() as i64))
}

pub(super) fn parents_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let owner = match inst.borrow().attrs.get("_owner_name").map(|v| v.kind()) {
        Some(ValueKind::Str(s)) => s.to_string(),
        _ => "Path".to_string(),
    };
    Ok(Value::string(format!("<{owner}.parents>")))
}

pub(super) fn parents_iter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let items = get_parents_items(&inst)?;
    Ok(Value::generator(Box::new(NativeIterFrame::generator(
        items,
    ))))
}

pub(super) fn parents_contains(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let items = get_parents_items(&inst)?;
    let target = &args[1].value;
    for item in &items {
        if interp.values_user_eq(item, target)? {
            return Ok(Value::bool_(true));
        }
    }
    Ok(Value::bool_(false))
}

pub(super) fn parents_getitem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let inst = expect_self(args, fn_name)?;
    let items = get_parents_items(&inst)?;
    let len = items.len() as i64;
    let idx_val = &args[1].value;

    if let Some((start_val, stop_val, step_val)) = pyrust_builtins::slice::slice_fields(idx_val) {
        let step: i64 = match step_val.kind() {
            ValueKind::None => 1,
            ValueKind::Int(n) => n,
            ValueKind::Bool(b) => i64::from(b),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "slice indices must be integers or None".to_string(),
                ));
            }
        };
        if step == 0 {
            return Err(PyError::named(
                "ValueError",
                "slice step cannot be zero".to_string(),
            ));
        }
        let clamp_bound = |v: &Value, default: i64| -> Result<i64> {
            let raw = match v.kind() {
                ValueKind::None => return Ok(default),
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "slice indices must be integers or None".to_string(),
                    ));
                }
            };
            let adjusted = if raw < 0 { raw + len } else { raw };
            if step > 0 {
                Ok(adjusted.clamp(0, len))
            } else {
                Ok(adjusted.clamp(-1, len - 1))
            }
        };
        let start = clamp_bound(&start_val, if step > 0 { 0 } else { len - 1 })?;
        let stop = clamp_bound(&stop_val, if step > 0 { len } else { -1 })?;
        let mut sliced = Vec::new();
        let mut cur = start;
        if step > 0 {
            while cur < stop {
                sliced.push(items[cur as usize].clone());
                cur += step;
            }
        } else {
            while cur > stop {
                sliced.push(items[cur as usize].clone());
                cur += step;
            }
        }
        return Ok(Value::tuple(sliced));
    }

    let raw_idx = match idx_val.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "indices must be integers or slices, not {}",
                    pyrust_core::builtin_type_name(idx_val)
                ),
            ));
        }
    };
    let idx = if raw_idx < 0 { raw_idx + len } else { raw_idx };
    if idx < 0 || idx >= len {
        return Err(PyError::named("IndexError", format!("{raw_idx}")));
    }
    Ok(items[idx as usize].clone())
}

use std::cell::RefCell;
