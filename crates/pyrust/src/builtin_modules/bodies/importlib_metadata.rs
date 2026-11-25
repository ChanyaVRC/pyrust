// `importlib.metadata` module — 100% native-Rust Python 3.12 implementation
// using PyRust's expanded macro toolkit (`pyrust_module!`, `exception`,
// `class`, `#[getter]`, `#[classmethod]`, `#[staticmethod]`, `#[native_state]`,
// `#[varkwargs]`, `PyPath`).
//
// Reads `.dist-info` and `.egg-info` package metadata (`METADATA`/`PKG-INFO`,
// `entry_points.txt`, `RECORD`, `top_level.txt`) on `sys.path` and exposes
// `PackageNotFoundError`, `PackageMetadata`, `EntryPoint`, `EntryPoints`,
// `PackagePath`, `FileHash`, `Distribution`, `PathDistribution`,
// `DistributionFinder`, `MetadataPathFinder`, `distribution`, `distributions`,
// `metadata`, `version`, `files`, `requires`, `entry_points`, and
// `packages_distributions`.
//
// Reference: <https://docs.python.org/3/library/importlib.metadata.html>

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyPath, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{PyDict, PyKey, PyModule, PySet, Value, ValueKind};

/// Keys that PEP 566 / CPython 3.12 `PackageMetadata.json` always emits as
/// lists (lowercased with `-` replaced by `_`).
const MULTIPLE_USE_KEYS: &[&str] = &[
    "classifier",
    "dynamic",
    "obsoletes_dist",
    "platform",
    "project_url",
    "provides_dist",
    "provides_extra",
    "requires_dist",
    "requires_external",
    "supported_platform",
];

struct MetadataState {
    /// Original header `(key, value)` pairs in file order, including duplicates.
    headers: Vec<(String, String)>,
    /// Message body after the blank line separating headers from payload.
    payload: String,
    /// True when an explicit `Description` header was present in `headers`.
    has_description_header: bool,
}

impl MetadataState {
    fn effective_entries(&self) -> Vec<(String, String)> {
        let mut entries = self.headers.clone();
        if !self.has_description_header && !self.payload.is_empty() {
            entries.push(("Description".to_string(), self.payload.clone()));
        }
        entries
    }

    fn get_first(&self, key: &str) -> Option<&str> {
        let lower = key.to_ascii_lowercase();
        for (k, v) in &self.headers {
            if k.to_ascii_lowercase() == lower {
                return Some(v.as_str());
            }
        }
        if lower == "description" && !self.has_description_header && !self.payload.is_empty() {
            return Some(self.payload.as_str());
        }
        None
    }

    fn get_all(&self, key: &str) -> Vec<String> {
        let lower = key.to_ascii_lowercase();
        let mut out = Vec::new();
        for (k, v) in &self.headers {
            if k.to_ascii_lowercase() == lower {
                out.push(v.clone());
            }
        }
        if out.is_empty()
            && lower == "description"
            && !self.has_description_header
            && !self.payload.is_empty()
        {
            out.push(self.payload.clone());
        }
        out
    }
}

fn py_eq(interp: &mut Interpreter, lhs: &Value, rhs: &Value) -> Result<bool> {
    let res = interp.eval_binary(lhs.clone(), BinaryOp::Eq, rhs.clone())?;
    interp.truthy_value(&res)
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_with_args_kwargs(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &IndexMap<String, Value>,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some(k.clone()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn meta_mod_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("importlib.metadata")?;
    interp.get_attr(&m, name)
}

fn raise_package_not_found(interp: &mut Interpreter, name: &str) -> PyError {
    match meta_mod_attr(interp, "PackageNotFoundError")
        .and_then(|cls| call_pos(interp, cls, &[Value::string(name)]))
    {
        Ok(exc) => PyError::raised(exc),
        Err(e) => e,
    }
}

/// PEP 503 distribution-name normalization: replace runs of `[-_.]+` with `-`
/// and lowercase ASCII.
fn normalize_dist_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_sep = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !prev_sep {
                out.push('-');
                prev_sep = true;
            }
        } else {
            out.push(ch.to_ascii_lowercase());
            prev_sep = false;
        }
    }
    out
}

/// Parse RFC 822 core metadata (`METADATA` / `PKG-INFO`) into ordered headers
/// and body payload.
fn parse_rfc822_metadata(raw: &str) -> MetadataState {
    let normalized = raw.replace("\r\n", "\n");
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut payload = String::new();
    let mut in_body = false;

    let mut lines = normalized.split('\n').peekable();
    while let Some(line) = lines.next() {
        if in_body {
            payload.push_str(line);
            if lines.peek().is_some() {
                payload.push('\n');
            }
            continue;
        }
        if line.is_empty() {
            in_body = true;
            continue;
        }
        if (line.starts_with(' ') || line.starts_with('\t')) && !headers.is_empty() {
            if let Some((_, val)) = headers.last_mut() {
                val.push('\n');
                val.push_str(line.trim_start());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    let has_description_header = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("description"));

    MetadataState {
        headers,
        payload,
        has_description_header,
    }
}

/// Parse an `EntryPoint.value` string (`module[:attr] [extra1, extra2]`) into
/// `(module, attr, extras)`.
fn parse_entry_point_value(value: &str) -> (String, Option<String>, Vec<String>) {
    let trimmed = value.trim();
    let (main_part, extras) = if let Some(bracket_idx) = trimmed.find('[') {
        let before = trimmed[..bracket_idx].trim();
        let after = &trimmed[bracket_idx + 1..];
        let inner = after.rsplit_once(']').map(|(i, _)| i).unwrap_or(after);
        let parsed_extras: Vec<String> = inner
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        (before, parsed_extras)
    } else {
        (trimmed, Vec::new())
    };

    if let Some((module, attr)) = main_part.split_once(':') {
        let m = module.trim().to_string();
        let a = attr.trim();
        let attr_opt = if a.is_empty() {
            None
        } else {
            Some(a.to_string())
        };
        (m, attr_opt, extras)
    } else {
        (main_part.trim().to_string(), None, extras)
    }
}

/// Parse `entry_points.txt` INI sections into `(name, value, group)` triples.
fn parse_entry_points_text(raw: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut current_group: Option<String> = None;
    for raw_line in raw.replace("\r\n", "\n").lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            current_group = Some(line[1..line.len() - 1].trim().to_string());
            continue;
        }
        if let Some(ref group) = current_group
            && let Some((name, val)) = line.split_once('=')
        {
            let k = name.trim();
            let v = val.trim();
            if !k.is_empty() {
                out.push((k.to_string(), v.to_string(), group.clone()));
            }
        }
    }
    out
}

/// Parse a single CSV line from `RECORD` (handles quoted fields with commas).
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(c);
            }
        } else if c == '"' && current.is_empty() {
            in_quotes = true;
        } else if c == ',' {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    fields.push(current);
    fields
}

/// Parse `RECORD` lines into `(path, optional_hash_spec, optional_size)`.
fn parse_record_text(raw: &str) -> Vec<(String, Option<String>, Option<i64>)> {
    let mut out = Vec::new();
    for line in raw.replace("\r\n", "\n").lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cols = parse_csv_line(trimmed);
        let Some(path) = cols.first().filter(|p| !p.is_empty()) else {
            continue;
        };
        let hash = cols
            .get(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let size = cols
            .get(2)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<i64>().ok());
        out.push((path.clone(), hash, size));
    }
    out
}

/// Extract distribution name stem from a `.dist-info` or `.egg-info` directory
/// or file name (e.g., `my_pkg-1.2.3.dist-info` -> `"my_pkg"`).
fn extract_dist_stem(file_name: &str) -> Option<String> {
    let base = file_name
        .strip_suffix(".dist-info")
        .or_else(|| file_name.strip_suffix(".egg-info"))?;
    // Split off `-<version>` if present.
    let stem = base.split_once('-').map(|(name, _)| name).unwrap_or(base);
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_string())
    }
}

fn make_pathlib_path(interp: &mut Interpreter, p: &Path) -> Result<Value> {
    let pathlib_mod = interp.load_module("pathlib")?;
    let path_cls = interp.get_attr(&pathlib_mod, "Path")?;
    call_pos(interp, path_cls, &[Value::string(p.to_string_lossy())])
}

fn make_package_metadata(interp: &mut Interpreter, raw: &str) -> Result<Value> {
    let cls = meta_mod_attr(interp, "PackageMetadata")?;
    call_pos(interp, cls, &[Value::string(raw)])
}

/// Discover `PathDistribution` objects across `search_paths`, optionally
/// filtered by PEP 503 normalized `target_name`.
fn discover_path_distributions(
    interp: &mut Interpreter,
    search_paths: &[PathBuf],
    target_name: Option<&str>,
) -> Result<Vec<Value>> {
    let target_norm = target_name.map(normalize_dist_name);
    let path_dist_cls = meta_mod_attr(interp, "PathDistribution")?;
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();

    for root in search_paths {
        let probe = if root.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            root.clone()
        };
        let Ok(entries) = std::fs::read_dir(&probe) else {
            continue;
        };
        let mut dir_entries: Vec<PathBuf> =
            entries.filter_map(|e| e.ok().map(|de| de.path())).collect();
        dir_entries.sort();

        for entry_path in dir_entries {
            let Some(fname) = entry_path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(stem) = extract_dist_stem(fname) else {
                continue;
            };
            let canonical = std::path::absolute(&entry_path).unwrap_or(entry_path.clone());
            if !seen_paths.insert(canonical.clone()) {
                continue;
            }
            if let Some(ref want) = target_norm
                && normalize_dist_name(&stem) != *want
            {
                continue;
            }
            let py_path = make_pathlib_path(interp, &canonical)?;
            let dist = call_pos(interp, path_dist_cls.clone(), &[py_path])?;
            out.push(dist);
        }
    }
    Ok(out)
}

fn collect_search_paths(
    interp: &mut Interpreter,
    path_arg: Option<&Value>,
) -> Result<Vec<PathBuf>> {
    let path_val = match path_arg {
        Some(v) if !v.is_none() => v.clone(),
        _ => {
            let sys_mod = interp.load_module("sys")?;
            interp.get_attr(&sys_mod, "path")?
        }
    };
    let items = interp.collect_iterable(&path_val)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if let Some(s) = item.as_str() {
            out.push(PathBuf::from(s));
        } else if let Ok(fspath) = interp.get_attr(&item, "__fspath__")
            && let Ok(res) = call_pos(interp, fspath, &[])
            && let Some(s) = res.as_str()
        {
            out.push(PathBuf::from(s));
        }
    }
    Ok(out)
}

fn discover_all_distributions(
    interp: &mut Interpreter,
    context_arg: Option<&Value>,
    kwargs: &IndexMap<String, Value>,
) -> Result<Vec<Value>> {
    let mut name_filter: Option<String> = None;
    let mut path_override: Option<Value> = None;

    if let Some(ctx) = context_arg
        && !ctx.is_none()
    {
        if !kwargs.is_empty() {
            return Err(PyError::named(
                "ValueError",
                "cannot accept context and kwargs",
            ));
        }
        if let Ok(n) = interp.get_attr(ctx, "name")
            && let Some(s) = n.as_str()
        {
            name_filter = Some(s.to_string());
        }
        if let Ok(p) = interp.get_attr(ctx, "path")
            && !p.is_none()
        {
            path_override = Some(p);
        }
    } else {
        if let Some(n) = kwargs.get("name")
            && let Some(s) = n.as_str()
        {
            name_filter = Some(s.to_string());
        }
        if let Some(p) = kwargs.get("path")
            && !p.is_none()
        {
            path_override = Some(p.clone());
        }
    }

    let mut results = Vec::new();

    // Consult any custom `find_distributions` finders on `sys.meta_path`.
    let sys_mod = interp.load_module("sys")?;
    if let Ok(meta_path) = interp.get_attr(&sys_mod, "meta_path")
        && let Ok(finders) = interp.collect_iterable(&meta_path)
    {
        let ctx_obj = if let Some(ctx) = context_arg
            && !ctx.is_none()
        {
            ctx.clone()
        } else {
            let df_cls = meta_mod_attr(interp, "DistributionFinder")?;
            let ctx_cls = interp.get_attr(&df_cls, "Context")?;
            call_with_args_kwargs(interp, ctx_cls, &[], kwargs)?
        };
        for finder in finders {
            if let Ok(find_dists) = interp.get_attr(&finder, "find_distributions") {
                let found = call_pos(interp, find_dists, std::slice::from_ref(&ctx_obj))?;
                let items = interp.collect_iterable(&found)?;
                results.extend(items);
            }
        }
    }

    // Default filesystem `.dist-info` / `.egg-info` discovery on `sys.path`.
    let search_paths = collect_search_paths(interp, path_override.as_ref())?;
    let fs_dists = discover_path_distributions(interp, &search_paths, name_filter.as_deref())?;
    results.extend(fs_dists);

    Ok(results)
}

fn dist_top_level_packages(interp: &mut Interpreter, dist: &Value) -> Result<Vec<String>> {
    let read_text_fn = interp.get_attr(dist, "read_text")?;
    let top_level = call_pos(interp, read_text_fn, &[Value::string("top_level.txt")])?;
    if let Some(text) = top_level.as_str() {
        let names: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !names.is_empty() {
            return Ok(names);
        }
    }
    // Fallback: infer top-level packages/modules from `dist.files`.
    let mut pkgs: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if let Ok(files_val) = interp.get_attr(dist, "files")
        && !files_val.is_none()
        && let Ok(items) = interp.collect_iterable(&files_val)
    {
        for item in items {
            let s_val = call_pos(interp, Value::builtin_function("str"), &[item])?;
            let Some(p_str) = s_val.as_str() else {
                continue;
            };
            let first = p_str.split('/').next().unwrap_or("");
            if first.is_empty()
                || first.ends_with(".dist-info")
                || first.ends_with(".egg-info")
                || first.starts_with("..")
            {
                continue;
            }
            let pkg_name = if p_str.contains('/') {
                first.to_string()
            } else if let Some(stem) = first.strip_suffix(".py") {
                stem.to_string()
            } else {
                continue;
            };
            if !pkg_name.is_empty() && seen.insert(pkg_name.clone()) {
                pkgs.push(pkg_name);
            }
        }
    }
    Ok(pkgs)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // Wire `PackageNotFoundError.__str__` and `.name` property to match CPython 3.12.
    let pnfe_opt = module.borrow().attrs.get("PackageNotFoundError").cloned();
    if let Some(pnfe) = pnfe_opt
        && let ValueKind::PyClass(cls_rc) = pnfe.kind()
    {
        cls_rc.borrow_mut().attrs.insert(
            "__str__".to_string(),
            Value::builtin_function("importlib.metadata.PackageNotFoundError.__str__"),
        );
        let prop = pyrust_builtins::property::property(
            Value::builtin_function("importlib.metadata.PackageNotFoundError.name"),
            Value::none(),
            Value::none(),
        );
        cls_rc.borrow_mut().attrs.insert("name".to_string(), prop);
    }

    // Wire `PackagePath` to inherit from `pathlib.PurePosixPath`.
    let pp_opt = module.borrow().attrs.get("PackagePath").cloned();
    if let Ok(pathlib_mod) = interp.load_module("pathlib")
        && let Ok(pure_posix) = interp.get_attr(&pathlib_mod, "PurePosixPath")
        && let Some(pp) = pp_opt
        && let (ValueKind::PyClass(pp_rc), ValueKind::PyClass(base_rc)) =
            (pp.kind(), pure_posix.kind())
    {
        crate::builtin_modules::macros::link_class_bases(pp_rc, vec![Rc::clone(base_rc)]);
    }

    // Wire `DistributionFinder` to inherit `importlib.abc.MetaPathFinder` and
    // attach nested `Context` class.
    let df_opt = module.borrow().attrs.get("DistributionFinder").cloned();
    let ctx_opt = module.borrow().attrs.get("_DistributionContext").cloned();
    if let Ok(imp_abc) = interp.load_module("importlib.abc")
        && let Ok(mpf) = interp.get_attr(&imp_abc, "MetaPathFinder")
        && let Some(df) = df_opt
        && let (ValueKind::PyClass(df_rc), ValueKind::PyClass(mpf_rc)) = (df.kind(), mpf.kind())
    {
        crate::builtin_modules::macros::link_class_bases(df_rc, vec![Rc::clone(mpf_rc)]);
        if let Some(ctx_cls) = ctx_opt {
            df_rc
                .borrow_mut()
                .attrs
                .insert("Context".to_string(), ctx_cls);
        }
        module
            .borrow_mut()
            .insert_attr("MetaPathFinder".to_string(), mpf.clone());
    }

    // Remove private helper bindings from the public module dict.
    {
        let mut m = module.borrow_mut();
        m.attrs.shift_remove("_DistributionContext");
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("Distribution"),
            Value::string("DistributionFinder"),
            Value::string("PackageMetadata"),
            Value::string("PackageNotFoundError"),
            Value::string("distribution"),
            Value::string("distributions"),
            Value::string("entry_points"),
            Value::string("files"),
            Value::string("metadata"),
            Value::string("packages_distributions"),
            Value::string("requires"),
            Value::string("version"),
        ]),
    }

    exception PackageNotFoundError(ModuleNotFoundError);

    #[py_name = "PackageNotFoundError.name"]
    fn _pnfe_name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
        let args = _interp.get_attr(self_val.0, "args")?;
        let items = _interp.collect_iterable(&args)?;
        Ok(items.into_iter().next().unwrap_or_else(Value::none))
    }

    #[py_name = "PackageNotFoundError.__str__"]
    fn _pnfe_str(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
        let name = _interp.get_attr(self_val.0, "name")?;
        let name_str = call_pos(_interp, Value::builtin_function("str"), &[name])?;
        Ok(Value::string(format!(
            "No package metadata was found for {}",
            name_str.as_str().unwrap_or_default()
        )))
    }

    #[native_state(MetadataState)]
    class PackageMetadata {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] raw: OptValueRef<'_>,
        ) -> Result<Value> {
            let text = raw
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("");
            set_state(self_val.0, parse_rfc822_metadata(text))?;
            Ok(Value::none())
        }

        fn __getitem__(#[state] st: &MetadataState, item: PyStr<'_>) -> Result<Value> {
            Ok(st
                .get_first(&item.0)
                .map(Value::string)
                .unwrap_or_else(Value::none))
        }

        fn get(
            #[state] st: &MetadataState,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] failobj: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(v) = st.get_first(&name.0) {
                return Ok(Value::string(v));
            }
            Ok(failobj.as_value().cloned().unwrap_or_else(Value::none))
        }

        fn get_all(
            #[state] st: &MetadataState,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] failobj: OptValueRef<'_>,
        ) -> Result<Value> {
            let vals = st.get_all(&name.0);
            if vals.is_empty() {
                return Ok(failobj.as_value().cloned().unwrap_or_else(Value::none));
            }
            Ok(Value::list(vals.into_iter().map(Value::string).collect()))
        }

        fn __contains__(#[state] st: &MetadataState, item: PyValue) -> Result<Value> {
            let Some(k) = item.0.as_str() else {
                return Ok(Value::bool_(false));
            };
            Ok(Value::bool_(st.get_first(k).is_some()))
        }

        fn __len__(#[state] st: &MetadataState) -> Result<Value> {
            Ok(Value::int(st.effective_entries().len() as i64))
        }

        fn __iter__(#[state] st: &MetadataState) -> Result<Value> {
            let keys: Vec<Value> = st
                .effective_entries()
                .into_iter()
                .map(|(k, _)| Value::string(k))
                .collect();
            call_pos(_interp, Value::builtin_function("iter"), &[Value::list(keys)])
        }

        fn keys(#[state] st: &MetadataState) -> Result<Value> {
            Ok(Value::list(
                st.effective_entries()
                    .into_iter()
                    .map(|(k, _)| Value::string(k))
                    .collect(),
            ))
        }

        fn values(#[state] st: &MetadataState) -> Result<Value> {
            Ok(Value::list(
                st.effective_entries()
                    .into_iter()
                    .map(|(_, v)| Value::string(v))
                    .collect(),
            ))
        }

        fn items(#[state] st: &MetadataState) -> Result<Value> {
            Ok(Value::list(
                st.effective_entries()
                    .into_iter()
                    .map(|(k, v)| Value::tuple(vec![Value::string(k), Value::string(v)]))
                    .collect(),
            ))
        }

        fn get_payload(#[state] st: &MetadataState) -> Result<Value> {
            Ok(Value::string(&st.payload))
        }

        #[getter]
        fn json(#[state] st: &MetadataState) -> Result<Value> {
            let mut out = PyDict::default();
            let mut seen: HashSet<String> = HashSet::new();
            for (k, _) in st.effective_entries() {
                let norm = k.to_ascii_lowercase().replace('-', "_");
                if !seen.insert(norm.clone()) {
                    continue;
                }
                if MULTIPLE_USE_KEYS.contains(&norm.as_str()) {
                    let vals = st.get_all(&k);
                    out.insert(
                        PyKey::str_from(&norm),
                        Value::list(vals.into_iter().map(Value::string).collect()),
                    );
                } else if let Some(val) = st.get_first(&k) {
                    out.insert(PyKey::str_from(&norm), Value::string(val));
                }
            }
            Ok(Value::dict(out))
        }
    }

    class FileHash {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, spec: PyStr<'_>) -> Result<Value> {
            let (mode, value) = spec
                .0
                .split_once('=')
                .ok_or_else(|| PyError::named("ValueError", "invalid FileHash spec"))?;
            _interp.assign_attr(self_val.0.clone(), "mode", Value::string(mode))?;
            _interp.assign_attr(self_val.0.clone(), "value", Value::string(value))?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mode = _interp.get_attr(self_val.0, "mode")?;
            let value = _interp.get_attr(self_val.0, "value")?;
            Ok(Value::string(format!(
                "<FileHash mode: {} value: {}>",
                mode.as_str().unwrap_or_default(),
                value.as_str().unwrap_or_default()
            )))
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let (Ok(om), Ok(ov)) = (
                _interp.get_attr(&other.0, "mode"),
                _interp.get_attr(&other.0, "value"),
            ) else {
                return Ok(Value::bool_(false));
            };
            let sm = _interp.get_attr(self_val.0, "mode")?;
            let sv = _interp.get_attr(self_val.0, "value")?;
            Ok(Value::bool_(
                py_eq(_interp, &sm, &om)? && py_eq(_interp, &sv, &ov)?,
            ))
        }
    }

    class PackagePath {
        fn locate(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dist = _interp.get_attr(self_val.0, "dist")?;
            let locate_fn = _interp.get_attr(&dist, "locate_file")?;
            call_pos(_interp, locate_fn, std::slice::from_ref(self_val.0))
        }

        fn read_text(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
        ) -> Result<Value> {
            let loc_fn = _interp.get_attr(self_val.0, "locate")?;
            let loc = call_pos(_interp, loc_fn, &[])?;
            let enc = encoding
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::string("utf-8"));
            let mut kw = IndexMap::new();
            kw.insert("encoding".to_string(), enc);
            let rt = _interp.get_attr(&loc, "read_text")?;
            call_with_args_kwargs(_interp, rt, &[], &kw)
        }

        fn read_binary(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let loc_fn = _interp.get_attr(self_val.0, "locate")?;
            let loc = call_pos(_interp, loc_fn, &[])?;
            let rb = _interp.get_attr(&loc, "read_bytes")?;
            call_pos(_interp, rb, &[])
        }

        fn read_bytes(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let loc_fn = _interp.get_attr(self_val.0, "locate")?;
            let loc = call_pos(_interp, loc_fn, &[])?;
            let rb = _interp.get_attr(&loc, "read_bytes")?;
            call_pos(_interp, rb, &[])
        }
    }

    class EntryPoint {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            value: PyStr<'_>,
            group: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "name", Value::string(name.0))?;
            _interp.assign_attr(self_val.0.clone(), "value", Value::string(value.0))?;
            _interp.assign_attr(self_val.0.clone(), "group", Value::string(group.0))?;
            _interp.assign_attr(self_val.0.clone(), "dist", Value::none())?;
            Ok(Value::none())
        }

        #[getter]
        fn module(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let val = _interp.get_attr(self_val.0, "value")?;
            let (m, _, _) = parse_entry_point_value(val.as_str().unwrap_or_default());
            Ok(Value::string(m))
        }

        #[getter]
        fn attr(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let val = _interp.get_attr(self_val.0, "value")?;
            let (_, a, _) = parse_entry_point_value(val.as_str().unwrap_or_default());
            Ok(a.map(Value::string).unwrap_or_else(Value::none))
        }

        #[getter]
        fn extras(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let val = _interp.get_attr(self_val.0, "value")?;
            let (_, _, extras) = parse_entry_point_value(val.as_str().unwrap_or_default());
            Ok(Value::list(extras.into_iter().map(Value::string).collect()))
        }

        fn load(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let val = _interp.get_attr(self_val.0, "value")?;
            let (module_name, attr_opt, _) =
                parse_entry_point_value(val.as_str().unwrap_or_default());
            let mut obj = _interp.load_module(&module_name)?;
            if let Some(attr_path) = attr_opt {
                for part in attr_path.split('.') {
                    if !part.is_empty() {
                        obj = _interp.get_attr(&obj, part)?;
                    }
                }
            }
            Ok(obj)
        }

        fn matches(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varkwargs] params: IndexMap<String, Value>,
        ) -> Result<Value> {
            for (k, expected) in &params {
                let actual = _interp.get_attr(self_val.0, k)?;
                if !py_eq(_interp, &actual, expected)? {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let nv = _interp.get_attr(self_val.0, "name")?;
            let vv = _interp.get_attr(self_val.0, "value")?;
            let gv = _interp.get_attr(self_val.0, "group")?;
            let n = render_value_repr(_interp, &nv)?;
            let v = render_value_repr(_interp, &vv)?;
            let g = render_value_repr(_interp, &gv)?;
            Ok(Value::string(format!(
                "EntryPoint(name={n}, value={v}, group={g})"
            )))
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let (Ok(on), Ok(ov), Ok(og)) = (
                _interp.get_attr(&other.0, "name"),
                _interp.get_attr(&other.0, "value"),
                _interp.get_attr(&other.0, "group"),
            ) else {
                return Ok(Value::bool_(false));
            };
            let sn = _interp.get_attr(self_val.0, "name")?;
            let sv = _interp.get_attr(self_val.0, "value")?;
            let sg = _interp.get_attr(self_val.0, "group")?;
            Ok(Value::bool_(
                py_eq(_interp, &sn, &on)?
                    && py_eq(_interp, &sv, &ov)?
                    && py_eq(_interp, &sg, &og)?,
            ))
        }

        fn __hash__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let tup = Value::tuple(vec![
                _interp.get_attr(self_val.0, "name")?,
                _interp.get_attr(self_val.0, "value")?,
                _interp.get_attr(self_val.0, "group")?,
            ]);
            call_pos(_interp, Value::builtin_function("hash"), &[tup])
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let tup = Value::tuple(vec![
                _interp.get_attr(self_val.0, "name")?,
                _interp.get_attr(self_val.0, "value")?,
                _interp.get_attr(self_val.0, "group")?,
            ]);
            call_pos(_interp, Value::builtin_function("iter"), &[tup])
        }

        fn __len__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::int(3))
        }
    }

    class EntryPoints("tuple") {
        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(self_val.0)?;
            for ep in items {
                if let Ok(ep_name) = _interp.get_attr(&ep, "name")
                    && py_eq(_interp, &ep_name, &name.0)?
                {
                    return Ok(ep);
                }
            }
            let r = render_value_repr(_interp, &name.0)?;
            Err(PyError::named("KeyError", r))
        }

        fn select(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varkwargs] params: IndexMap<String, Value>,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(self_val.0)?;
            let mut matched = Vec::new();
            for ep in items {
                if params.is_empty() {
                    matched.push(ep);
                    continue;
                }
                let matches_fn = _interp.get_attr(&ep, "matches")?;
                let ok_val = call_with_args_kwargs(_interp, matches_fn, &[], &params)?;
                if _interp.truthy_value(&ok_val)? {
                    matched.push(ep);
                }
            }
            let eps_cls = meta_mod_attr(_interp, "EntryPoints")?;
            call_pos(_interp, eps_cls, &[Value::list(matched)])
        }

        #[getter]
        fn names(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let items = _interp.collect_iterable(self_val.0)?;
            let mut set = PySet::default();
            for ep in items {
                if let Ok(n) = _interp.get_attr(&ep, "name")
                    && let Some(s) = n.as_str()
                {
                    set.insert(PyKey::str_from(s));
                }
            }
            Ok(Value::set(set))
        }

        #[getter]
        fn groups(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let items = _interp.collect_iterable(self_val.0)?;
            let mut set = PySet::default();
            for ep in items {
                if let Ok(g) = _interp.get_attr(&ep, "group")
                    && let Some(s) = g.as_str()
                {
                    set.insert(PyKey::str_from(s));
                }
            }
            Ok(Value::set(set))
        }
    }

    class _DistributionContext {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] path: OptValueRef<'_>,
            #[varkwargs] extra: IndexMap<String, Value>,
        ) -> Result<Value> {
            let name_val = name.as_value().cloned().unwrap_or_else(Value::none);
            let path_val = match path.as_value() {
                Some(p) if !p.is_none() => p.clone(),
                _ => {
                    let sys_mod = _interp.load_module("sys")?;
                    _interp.get_attr(&sys_mod, "path")?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "name", name_val)?;
            _interp.assign_attr(self_val.0.clone(), "path", path_val)?;
            for (k, v) in extra {
                _interp.assign_attr(self_val.0.clone(), &k, v)?;
            }
            Ok(Value::none())
        }
    }

    class Distribution {
        fn read_text(
            #[positional_only] _self: PyValueRef<'_>,
            _filename: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "read_text is abstract"))
        }

        fn locate_file(
            #[positional_only] _self: PyValueRef<'_>,
            _path: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "locate_file is abstract"))
        }

        #[classmethod]
        fn from_name(
            #[positional_only] cls: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let name_str: &str = &name.0;
            if name_str.is_empty() {
                return Err(PyError::named(
                    "ValueError",
                    "A distribution name is required.",
                ));
            }
            let discover_fn = _interp.get_attr(cls.0, "discover")?;
            let mut kw = IndexMap::new();
            kw.insert("name".to_string(), Value::string(name_str));
            let found = call_with_args_kwargs(_interp, discover_fn, &[], &kw)?;
            let items = _interp.collect_iterable(&found)?;
            if let Some(first) = items.into_iter().next() {
                return Ok(first);
            }
            Err(raise_package_not_found(_interp, name_str))
        }

        #[classmethod]
        fn discover(
            #[positional_only] _cls: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let dists = discover_all_distributions(_interp, context.as_value(), &kwargs)?;
            Ok(Value::list(dists))
        }

        #[staticmethod]
        fn at(path: PyPath) -> Result<Value> {
            let pd_cls = meta_mod_attr(_interp, "PathDistribution")?;
            let py_path = make_pathlib_path(_interp, path.as_path())?;
            call_pos(_interp, pd_cls, &[py_path])
        }

        #[getter]
        fn metadata(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rt = _interp.get_attr(self_val.0, "read_text")?;
            let mut raw = call_pos(_interp, rt.clone(), &[Value::string("METADATA")])?;
            if raw.is_none() {
                raw = call_pos(_interp, rt.clone(), &[Value::string("PKG-INFO")])?;
            }
            if raw.is_none() {
                raw = call_pos(_interp, rt, &[Value::string("")])?;
            }
            let text = raw.as_str().unwrap_or("");
            make_package_metadata(_interp, text)
        }

        #[getter]
        fn name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let meta = _interp.get_attr(self_val.0, "metadata")?;
            let get_fn = _interp.get_attr(&meta, "get")?;
            call_pos(_interp, get_fn, &[Value::string("Name")])
        }

        #[getter]
        fn version(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let meta = _interp.get_attr(self_val.0, "metadata")?;
            let get_fn = _interp.get_attr(&meta, "get")?;
            call_pos(_interp, get_fn, &[Value::string("Version")])
        }

        #[getter]
        fn entry_points(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rt = _interp.get_attr(self_val.0, "read_text")?;
            let raw = call_pos(_interp, rt, &[Value::string("entry_points.txt")])?;
            let ep_cls = meta_mod_attr(_interp, "EntryPoint")?;
            let eps_cls = meta_mod_attr(_interp, "EntryPoints")?;
            let mut eps = Vec::new();
            if let Some(text) = raw.as_str() {
                for (name, value, group) in parse_entry_points_text(text) {
                    let ep = call_pos(
                        _interp,
                        ep_cls.clone(),
                        &[Value::string(name), Value::string(value), Value::string(group)],
                    )?;
                    _interp.assign_attr(ep.clone(), "dist", self_val.0.clone())?;
                    eps.push(ep);
                }
            }
            call_pos(_interp, eps_cls, &[Value::list(eps)])
        }

        #[getter]
        fn files(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rt = _interp.get_attr(self_val.0, "read_text")?;
            let pp_cls = meta_mod_attr(_interp, "PackagePath")?;
            let fh_cls = meta_mod_attr(_interp, "FileHash")?;

            let record_raw = call_pos(_interp, rt.clone(), &[Value::string("RECORD")])?;
            if let Some(text) = record_raw.as_str() {
                let mut out = Vec::new();
                for (rel_path, hash_opt, size_opt) in parse_record_text(text) {
                    let pp = call_pos(_interp, pp_cls.clone(), &[Value::string(rel_path)])?;
                    let hash_val = match hash_opt {
                        Some(spec) if spec.contains('=') => {
                            call_pos(_interp, fh_cls.clone(), &[Value::string(spec)])?
                        }
                        _ => Value::none(),
                    };
                    let size_val = size_opt.map(Value::int).unwrap_or_else(Value::none);
                    _interp.assign_attr(pp.clone(), "hash", hash_val)?;
                    _interp.assign_attr(pp.clone(), "size", size_val)?;
                    _interp.assign_attr(pp.clone(), "dist", self_val.0.clone())?;
                    out.push(pp);
                }
                return Ok(Value::list(out));
            }

            // Fallback for `.egg-info` (`SOURCES.txt` / `installed-files.txt`).
            for alt in ["SOURCES.txt", "installed-files.txt"] {
                let raw = call_pos(_interp, rt.clone(), &[Value::string(alt)])?;
                if let Some(text) = raw.as_str() {
                    let mut out = Vec::new();
                    for line in text.replace("\r\n", "\n").lines() {
                        let p = line.trim();
                        if p.is_empty() {
                            continue;
                        }
                        let pp = call_pos(_interp, pp_cls.clone(), &[Value::string(p)])?;
                        _interp.assign_attr(pp.clone(), "hash", Value::none())?;
                        _interp.assign_attr(pp.clone(), "size", Value::none())?;
                        _interp.assign_attr(pp.clone(), "dist", self_val.0.clone())?;
                        out.push(pp);
                    }
                    return Ok(Value::list(out));
                }
            }

            Ok(Value::none())
        }

        #[getter]
        fn requires(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let meta = _interp.get_attr(self_val.0, "metadata")?;
            let get_all = _interp.get_attr(&meta, "get_all")?;
            let reqs = call_pos(_interp, get_all, &[Value::string("Requires-Dist")])?;
            if !reqs.is_none() {
                return Ok(reqs);
            }
            // Fallback for `.egg-info/requires.txt`.
            let rt = _interp.get_attr(self_val.0, "read_text")?;
            let raw = call_pos(_interp, rt, &[Value::string("requires.txt")])?;
            if let Some(text) = raw.as_str() {
                let mut out = Vec::new();
                let mut current_extra: Option<String> = None;
                for raw_line in text.replace("\r\n", "\n").lines() {
                    let line = raw_line.trim();
                    if line.is_empty() || line.starts_with('#') {
                        continue;
                    }
                    if line.starts_with('[') && line.ends_with(']') {
                        let inner = line[1..line.len() - 1].trim();
                        current_extra = if inner.is_empty() {
                            None
                        } else {
                            Some(inner.to_string())
                        };
                        continue;
                    }
                    let req = if let Some(ref extra) = current_extra {
                        if let Some((ex, cond)) = extra.split_once(':') {
                            if ex.is_empty() {
                                format!("{line} ; {cond}")
                            } else {
                                format!("{line} ; ({cond}) and extra == \"{ex}\"")
                            }
                        } else {
                            format!("{line} ; extra == \"{extra}\"")
                        }
                    } else {
                        line.to_string()
                    };
                    out.push(Value::string(req));
                }
                if !out.is_empty() {
                    return Ok(Value::list(out));
                }
            }
            Ok(Value::none())
        }
    }

    class PathDistribution("Distribution") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyValue,
        ) -> Result<Value> {
            let p_obj = if path.0.as_str().is_some() {
                let pathlib_mod = _interp.load_module("pathlib")?;
                let path_cls = _interp.get_attr(&pathlib_mod, "Path")?;
                call_pos(_interp, path_cls, &[path.0])?
            } else {
                path.0
            };
            _interp.assign_attr(self_val.0.clone(), "_path", p_obj)?;
            Ok(Value::none())
        }

        fn read_text(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let base = _interp.get_attr(self_val.0, "_path")?;
            let fname: &str = &filename.0;
            let base_str = call_pos(
                _interp,
                Value::builtin_function("str"),
                std::slice::from_ref(&base),
            )?;
            let base_path = Path::new(base_str.as_str().unwrap_or_default());
            if base_path.is_file() {
                if fname.is_empty() || fname == "METADATA" || fname == "PKG-INFO" {
                    return match std::fs::read_to_string(base_path) {
                        Ok(s) => Ok(Value::string(s)),
                        Err(_) => Ok(Value::none()),
                    };
                }
                return Ok(Value::none());
            }
            let target = base_path.join(fname);
            match std::fs::read_to_string(&target) {
                Ok(s) => Ok(Value::string(s)),
                Err(_) => Ok(Value::none()),
            }
        }

        fn locate_file(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyValue,
        ) -> Result<Value> {
            let base = _interp.get_attr(self_val.0, "_path")?;
            let parent = _interp.get_attr(&base, "parent")?;
            let jp = _interp.get_attr(&parent, "joinpath")?;
            call_pos(_interp, jp, &[path.0])
        }

        #[getter]
        fn _normalized_name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let name = _interp.get_attr(self_val.0, "name")?;
            let s = name.as_str().unwrap_or_default();
            Ok(Value::string(normalize_dist_name(s)))
        }
    }

    class DistributionFinder {
        fn find_distributions(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _context: OptValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::list(Vec::new()))
        }
    }

    class MetadataPathFinder("DistributionFinder") {
        #[classmethod]
        fn find_distributions(
            #[positional_only] _cls: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let mut name_filter: Option<String> = None;
            let mut path_override: Option<Value> = None;
            if let Some(ctx) = context.as_value()
                && !ctx.is_none()
            {
                if let Ok(n) = _interp.get_attr(ctx, "name")
                    && let Some(s) = n.as_str()
                {
                    name_filter = Some(s.to_string());
                }
                if let Ok(p) = _interp.get_attr(ctx, "path")
                    && !p.is_none()
                {
                    path_override = Some(p);
                }
            }
            let search_paths = collect_search_paths(_interp, path_override.as_ref())?;
            let dists =
                discover_path_distributions(_interp, &search_paths, name_filter.as_deref())?;
            Ok(Value::list(dists))
        }

        #[classmethod]
        fn invalidate_caches(#[positional_only] _cls: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }

    fn distribution(distribution_name: PyStr<'_>) -> Result<Value> {
        let dist_cls = meta_mod_attr(_interp, "Distribution")?;
        let from_name = _interp.get_attr(&dist_cls, "from_name")?;
        call_pos(_interp, from_name, &[Value::string(distribution_name.0)])
    }

    fn distributions(#[varkwargs] kwargs: IndexMap<String, Value>) -> Result<Value> {
        let dist_cls = meta_mod_attr(_interp, "Distribution")?;
        let discover = _interp.get_attr(&dist_cls, "discover")?;
        call_with_args_kwargs(_interp, discover, &[], &kwargs)
    }

    fn metadata(distribution_name: PyStr<'_>) -> Result<Value> {
        let dist_cls = meta_mod_attr(_interp, "Distribution")?;
        let from_name = _interp.get_attr(&dist_cls, "from_name")?;
        let dist = call_pos(_interp, from_name, &[Value::string(distribution_name.0)])?;
        _interp.get_attr(&dist, "metadata")
    }

    fn version(distribution_name: PyStr<'_>) -> Result<Value> {
        let dist_cls = meta_mod_attr(_interp, "Distribution")?;
        let from_name = _interp.get_attr(&dist_cls, "from_name")?;
        let dist = call_pos(_interp, from_name, &[Value::string(distribution_name.0)])?;
        _interp.get_attr(&dist, "version")
    }

    fn files(distribution_name: PyStr<'_>) -> Result<Value> {
        let dist_cls = meta_mod_attr(_interp, "Distribution")?;
        let from_name = _interp.get_attr(&dist_cls, "from_name")?;
        let dist = call_pos(_interp, from_name, &[Value::string(distribution_name.0)])?;
        _interp.get_attr(&dist, "files")
    }

    fn requires(distribution_name: PyStr<'_>) -> Result<Value> {
        let dist_cls = meta_mod_attr(_interp, "Distribution")?;
        let from_name = _interp.get_attr(&dist_cls, "from_name")?;
        let dist = call_pos(_interp, from_name, &[Value::string(distribution_name.0)])?;
        _interp.get_attr(&dist, "requires")
    }

    fn entry_points(#[varkwargs] params: IndexMap<String, Value>) -> Result<Value> {
        let dists = discover_all_distributions(_interp, None, &IndexMap::new())?;
        let mut all_eps: Vec<Value> = Vec::new();
        let mut seen_dists: HashSet<String> = HashSet::new();
        for dist in dists {
            if let Ok(n) = _interp.get_attr(&dist, "name")
                && let Some(s) = n.as_str()
                && !seen_dists.insert(normalize_dist_name(s))
            {
                continue;
            }
            if let Ok(dist_eps) = _interp.get_attr(&dist, "entry_points")
                && let Ok(items) = _interp.collect_iterable(&dist_eps)
            {
                all_eps.extend(items);
            }
        }
        let eps_cls = meta_mod_attr(_interp, "EntryPoints")?;
        let eps = call_pos(_interp, eps_cls, &[Value::list(all_eps)])?;
        if params.is_empty() {
            return Ok(eps);
        }
        let select_fn = _interp.get_attr(&eps, "select")?;
        call_with_args_kwargs(_interp, select_fn, &[], &params)
    }

    fn packages_distributions() -> Result<Value> {
        let dists = discover_all_distributions(_interp, None, &IndexMap::new())?;
        let mut pkg_to_dists: IndexMap<String, Vec<String>> = IndexMap::new();
        for dist in dists {
            let dist_name = _interp
                .get_attr(&dist, "name")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if dist_name.is_empty() {
                continue;
            }
            for pkg in dist_top_level_packages(_interp, &dist)? {
                let entry = pkg_to_dists.entry(pkg).or_default();
                if !entry.contains(&dist_name) {
                    entry.push(dist_name.clone());
                }
            }
        }
        let mut out = PyDict::default();
        for (pkg, names) in pkg_to_dists {
            out.insert(
                PyKey::str_from(&pkg),
                Value::list(names.into_iter().map(Value::string).collect()),
            );
        }
        Ok(Value::dict(out))
    }
}
