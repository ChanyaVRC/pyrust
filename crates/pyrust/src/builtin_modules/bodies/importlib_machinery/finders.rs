//! The finders of `_bootstrap_external`: `WindowsRegistryFinder`,
//! `_NamespacePath`, `PathFinder`, `FileFinder` and the closure
//! `FileFinder.path_hook` returns.

use super::*;
use crate::value::PySet;

/// The state of a `FileFinder.path_hook(*loader_details)` closure.
pub(super) struct PathHookState {
    pub(super) cls: Value,
    pub(super) loader_details: Vec<Value>,
}

fn getcwd(interp: &mut Interpreter) -> Result<Value> {
    let os = interp.load_module("os")?;
    call_method(interp, &os, "getcwd", &[])
}

/// `_path_stat(path)` (`os.stat`).
fn path_stat(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let os = interp.load_module("os")?;
    call_method(interp, &os, "stat", std::slice::from_ref(path))
}

const S_IFMT: i64 = 0o170000;
const S_IFDIR: i64 = 0o040000;

/// `_path_is_mode_type(path, mode)`, with `_path_isdir`'s empty-path rule.
fn path_is_mode_type(interp: &mut Interpreter, path: &Value, mode: i64) -> Result<bool> {
    if let Some(text) = path.as_str() {
        if text.is_empty() && mode == S_IFDIR {
            let cwd = getcwd(interp)?;
            return path_is_mode_type(interp, &cwd, mode);
        }
        return Ok(std::fs::metadata(text).is_ok_and(|metadata| {
            if mode == S_IFDIR {
                metadata.is_dir()
            } else {
                metadata.is_file()
            }
        }));
    }
    let path = if mode == S_IFDIR && !truthy(interp, path)? {
        getcwd(interp)?
    } else {
        path.clone()
    };
    let Some(stat_info) = attempt(interp, "OSError", |interp| path_stat(interp, &path))? else {
        return Ok(false);
    };
    let st_mode = get(interp, &stat_info, "st_mode")?;
    Ok(st_mode
        .as_int()
        .is_some_and(|st_mode| st_mode & S_IFMT == mode))
}

fn path_isdir(interp: &mut Interpreter, path: &Value) -> Result<bool> {
    path_is_mode_type(interp, path, S_IFDIR)
}

fn path_isfile(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file())
}

/// `isinstance(entry, str)`.
pub(super) fn is_str(interp: &mut Interpreter, entry: &Value) -> Result<bool> {
    if entry.as_str().is_some() {
        return Ok(true);
    }
    let Some(str_class) = crate::interpreter::primitive_class_by_name("str") else {
        return Ok(false);
    };
    let result = call(
        interp,
        &builtin("isinstance"),
        &[entry.clone(), Value::py_class(str_class)],
    )?;
    interp.truthy_value(&result)
}

/// `_get_supported_file_loaders()`: extension, source, bytecode.
fn supported_file_loaders(interp: &mut Interpreter) -> Result<Vec<Value>> {
    let extension = machinery_attr(interp, "ExtensionFileLoader")?;
    let source = machinery_attr(interp, "SourceFileLoader")?;
    let sourceless = machinery_attr(interp, "SourcelessFileLoader")?;
    let source_suffixes = machinery_attr(interp, "SOURCE_SUFFIXES")?;
    let bytecode_suffixes = machinery_attr(interp, "BYTECODE_SUFFIXES")?;
    Ok(vec![
        Value::tuple(vec![extension, extension_suffixes()]),
        Value::tuple(vec![source, source_suffixes]),
        Value::tuple(vec![sourceless, bytecode_suffixes]),
    ])
}

/// `(loader, suffixes)` of one `loader_details` entry.
fn unpack_pair(interp: &mut Interpreter, pair: &Value) -> Result<(Value, Value)> {
    let items = interp.collect_iterable(pair)?;
    match <[Value; 2]>::try_from(items) {
        Ok([first, second]) => Ok((first, second)),
        Err(items) if items.len() > 2 => Err(PyError::named(
            "ValueError",
            "too many values to unpack (expected 2)",
        )),
        Err(items) => Err(PyError::named(
            "ValueError",
            format!(
                "not enough values to unpack (expected 2, got {})",
                items.len()
            ),
        )),
    }
}

fn str_set(interp: &mut Interpreter, names: impl IntoIterator<Item = String>) -> Result<Value> {
    let mut set = PySet::default();
    for name in names {
        interp.set_insert(&mut set, PyKey::str_from(&name))?;
    }
    Ok(Value::set(set))
}

// ── WindowsRegistryFinder ────────────────────────────────────────────────────

#[cfg(windows)]
pub(super) fn registry_open(interp: &mut Interpreter, key: &Value) -> Result<Value> {
    let winreg = interp.load_module("winreg")?;
    let open = get(interp, &winreg, "OpenKey")?;
    let current_user = get(interp, &winreg, "HKEY_CURRENT_USER")?;
    let opened = attempt(interp, "OSError", |interp| {
        call(interp, &open, &[current_user, key.clone()])
    })?;
    if let Some(handle) = opened {
        return Ok(handle);
    }
    let local_machine = get(interp, &winreg, "HKEY_LOCAL_MACHINE")?;
    call(interp, &open, &[local_machine, key.clone()])
}

/// `winreg` is only imported by `_bootstrap_external` on Windows.
#[cfg(not(windows))]
pub(super) fn registry_open(_interp: &mut Interpreter, _key: &Value) -> Result<Value> {
    Err(PyError::name_error(
        "NameError",
        "name 'winreg' is not defined",
        Some("winreg".to_string()),
    ))
}

/// `with cls._open_registry(key) as hkey: filepath = winreg.QueryValue(hkey, '')`.
fn registry_query(interp: &mut Interpreter, cls: &Value, key: Value) -> Result<Value> {
    let handle = call_method(interp, cls, "_open_registry", &[key])?;
    let hkey = call_method(interp, &handle, "__enter__", &[])?;
    let filepath = registry_query_value(interp, hkey);
    let exit = call_method(
        interp,
        &handle,
        "__exit__",
        &[Value::none(), Value::none(), Value::none()],
    );
    let filepath = filepath?;
    exit?;
    Ok(filepath)
}

#[cfg(windows)]
fn registry_query_value(interp: &mut Interpreter, hkey: Value) -> Result<Value> {
    let winreg = interp.load_module("winreg")?;
    call_method(interp, &winreg, "QueryValue", &[hkey, s("")])
}

#[cfg(not(windows))]
fn registry_query_value(interp: &mut Interpreter, hkey: Value) -> Result<Value> {
    let _ = (interp, hkey);
    Err(PyError::name_error(
        "NameError",
        "name 'winreg' is not defined",
        Some("winreg".to_string()),
    ))
}

pub(super) fn registry_search(
    interp: &mut Interpreter,
    cls: &Value,
    fullname: &Value,
) -> Result<Value> {
    let debug_build = get(interp, cls, "DEBUG_BUILD")?;
    let registry_key = if truthy(interp, &debug_build)? {
        get(interp, cls, "REGISTRY_KEY_DEBUG")?
    } else {
        get(interp, cls, "REGISTRY_KEY")?
    };
    let version_info = sys_attr(interp, "version_info")?;
    let version = interp.collect_iterable(&version_info)?;
    let major = version.first().and_then(Value::as_int).unwrap_or(3);
    let minor = version.get(1).and_then(Value::as_int).unwrap_or(12);
    let format = get(interp, &registry_key, "format")?;
    let key = call_kw(
        interp,
        &format,
        &[],
        &[
            ("fullname", fullname.clone()),
            ("sys_version", Value::string(format!("{major}.{minor}"))),
        ],
    )?;
    let filepath = attempt(interp, "OSError", |interp| registry_query(interp, cls, key))?;
    Ok(filepath.unwrap_or_else(Value::none))
}

pub(super) fn registry_find_spec(
    interp: &mut Interpreter,
    cls: &Value,
    fullname: &Value,
) -> Result<Value> {
    let filepath = call_method(
        interp,
        cls,
        "_search_registry",
        std::slice::from_ref(fullname),
    )?;
    if filepath.is_none() {
        return Ok(Value::none());
    }
    if attempt(interp, "OSError", |interp| path_stat(interp, &filepath))?.is_none() {
        return Ok(Value::none());
    }
    for pair in supported_file_loaders(interp)? {
        let (loader, suffixes) = unpack_pair(interp, &pair)?;
        if ends_with_any(interp, &filepath, &suffixes)? {
            let loader = call(interp, &loader, &[fullname.clone(), filepath.clone()])?;
            return util::spec_from_loader_impl(
                interp,
                fullname,
                &loader,
                filepath.clone(),
                Value::none(),
            );
        }
    }
    Ok(Value::none())
}

// ── _NamespacePath ───────────────────────────────────────────────────────────

pub(super) fn namespace_path_init(
    interp: &mut Interpreter,
    this: &Value,
    name: Value,
    path: Value,
    path_finder: Value,
) -> Result<()> {
    set(interp, this, "_name", name)?;
    set(interp, this, "_path", path)?;
    let parent_path = namespace_parent_path(interp, this)?;
    let parent_path = call(interp, &builtin("tuple"), &[parent_path])?;
    set(interp, this, "_last_parent_path", parent_path)?;
    let epoch = get(interp, this, "_epoch")?;
    set(interp, this, "_last_epoch", epoch)?;
    set(interp, this, "_path_finder", path_finder)
}

/// `(parent-module-name, parent-path-attr-name)`.
pub(super) fn namespace_parent_path_names(
    interp: &mut Interpreter,
    this: &Value,
) -> Result<(Value, Value)> {
    let name = get(interp, this, "_name")?;
    let parent = if let Some(text) = name.as_str() {
        text.rsplit_once('.').map(|(parent, _)| s(parent))
    } else {
        let parts = call_method(interp, &name, "rpartition", &[s(".")])?;
        let parts = interp.collect_iterable(&parts)?;
        match parts.as_slice() {
            [parent, dot, _] if truthy(interp, dot)? => Some(parent.clone()),
            _ => None,
        }
    };
    Ok(match parent {
        // This is a top-level module. sys.path contains the parent path.
        None => (s("sys"), s("path")),
        // Not a top-level module. parent-module.__path__ contains the
        // parent path.
        Some(parent) => (parent, s("__path__")),
    })
}

pub(super) fn namespace_parent_path(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let (module_name, attr_name) = namespace_parent_path_names(interp, this)?;
    let modules = sys_attr(interp, "modules")?;
    let module = call_method(interp, &modules, "__getitem__", &[module_name])?;
    let attr_name = str_of(interp, &attr_name)?;
    get(interp, &module, &attr_name)
}

/// If the parent's path has changed, recalculate `_path`.
pub(super) fn namespace_recalculate(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let parent_path = namespace_parent_path(interp, this)?;
    // Make a copy
    let parent_path = call(interp, &builtin("tuple"), &[parent_path])?;
    let last_parent_path = get(interp, this, "_last_parent_path")?;
    let mut changed = ne(interp, &parent_path, &last_parent_path)?;
    let epoch = get(interp, this, "_epoch")?;
    if !changed {
        let last_epoch = get(interp, this, "_last_epoch")?;
        changed = ne(interp, &epoch, &last_epoch)?;
    }
    if changed {
        let path_finder = get(interp, this, "_path_finder")?;
        let name = get(interp, this, "_name")?;
        let spec = call(interp, &path_finder, &[name, parent_path.clone()])?;
        // Note that no changes are made if a loader is returned, but we
        // do remember the new parent path
        if !spec.is_none() && get(interp, &spec, "loader")?.is_none() {
            let locations = get(interp, &spec, "submodule_search_locations")?;
            if truthy(interp, &locations)? {
                set(interp, this, "_path", locations)?;
            }
        }
        // Save the copy
        set(interp, this, "_last_parent_path", parent_path)?;
        set(interp, this, "_last_epoch", epoch)?;
    }
    get(interp, this, "_path")
}

// ── PathFinder ───────────────────────────────────────────────────────────────

pub(super) fn path_finder_invalidate_caches(interp: &mut Interpreter) -> Result<()> {
    let cache = sys_attr(interp, "path_importer_cache")?;
    let items = call_method(interp, &cache, "items", &[])?;
    for item in interp.collect_iterable(&items)? {
        let (name, finder) = unpack_pair(interp, &item)?;
        // Drop entry if finder name is a relative path. The current
        // working directory may have changed.
        let is_absolute = name.as_str().is_some_and(util::path_isabs);
        if finder.is_none() || !is_absolute {
            call_method(interp, &cache, "__delitem__", &[name])?;
        } else if has_attr(interp, &finder, "invalidate_caches")? {
            call_method(interp, &finder, "invalidate_caches", &[])?;
        }
    }
    // Also invalidate the caches of _NamespacePaths
    // https://bugs.python.org/issue45703
    let namespace_path = hidden_class(interp, "_NamespacePath")?;
    let epoch = get(interp, &namespace_path, "_epoch")?;
    let epoch = interp.eval_binary(epoch, BinaryOp::Add, Value::int(1))?;
    set(interp, &namespace_path, "_epoch", epoch)?;

    let metadata_finder = module_attr(interp, "importlib.metadata", "MetadataPathFinder")?;
    call_method(interp, &metadata_finder, "invalidate_caches", &[])?;
    Ok(())
}

pub(super) fn path_hooks(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let hooks = sys_attr(interp, "path_hooks")?;
    if !hooks.is_none() && !truthy(interp, &hooks)? {
        let warnings = interp.load_module("warnings")?;
        let category = exc_class("ImportWarning")?;
        call_method(
            interp,
            &warnings,
            "warn",
            &[s("sys.path_hooks is empty"), category],
        )?;
    }
    for hook in interp.collect_iterable(&hooks)? {
        let found = attempt(interp, "ImportError", |interp| {
            call(interp, &hook, std::slice::from_ref(path))
        })?;
        if let Some(finder) = found {
            return Ok(finder);
        }
    }
    Ok(Value::none())
}

/// The `FileFinder` CPython's `sys.path_hooks` entry would build for `path`.
fn fallback_file_finder(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let file_finder = machinery_attr(interp, "FileFinder")?;
    let mut args = vec![path.clone()];
    args.extend(supported_file_loaders(interp)?);
    call(interp, &file_finder, &args)
}

pub(super) fn path_importer_cache(
    interp: &mut Interpreter,
    cls: &Value,
    path: Value,
) -> Result<Value> {
    let mut path = path;
    if path.as_str() == Some("") {
        match attempt(interp, "FileNotFoundError", getcwd)? {
            Some(cwd) => path = cwd,
            // Don't cache the failure as the cwd can easily change to
            // a valid directory later on.
            None => return Ok(Value::none()),
        }
    }
    let cache = sys_attr(interp, "path_importer_cache")?;
    let cached = attempt(interp, "KeyError", |interp| {
        call_method(interp, &cache, "__getitem__", std::slice::from_ref(&path))
    })?;
    if let Some(finder) = cached {
        return Ok(finder);
    }
    let finder = call_method(interp, cls, "_path_hooks", std::slice::from_ref(&path))?;
    if !finder.is_none() {
        call_method(interp, &cache, "__setitem__", &[path, finder.clone()])?;
        return Ok(finder);
    }
    // PyRust's `sys.path_hooks` has no `FileFinder` hook: a directory no hook
    // claims gets an uncached `FileFinder` (caching `None` would hide the
    // directory from the runtime importer).
    if path_isdir(interp, &path)? {
        return fallback_file_finder(interp, &path);
    }
    call_method(interp, &cache, "__setitem__", &[path, Value::none()])?;
    Ok(Value::none())
}

pub(super) fn path_finder_get_spec(
    interp: &mut Interpreter,
    cls: &Value,
    fullname: &Value,
    path: &Value,
    target: &Value,
) -> Result<Value> {
    // If this ends up being a namespace package, namespace_path is
    //  the list of paths that will become its __path__
    let mut namespace_path = Vec::new();
    for entry in interp.collect_iterable(path)? {
        if !is_str(interp, &entry)? {
            continue;
        }
        let finder = call_method(interp, cls, "_path_importer_cache", &[entry])?;
        if finder.is_none() {
            continue;
        }
        let spec = call_method(
            interp,
            &finder,
            "find_spec",
            &[fullname.clone(), target.clone()],
        )?;
        if spec.is_none() {
            continue;
        }
        if !get(interp, &spec, "loader")?.is_none() {
            return Ok(spec);
        }
        let portions = get(interp, &spec, "submodule_search_locations")?;
        if portions.is_none() {
            return Err(import_error(interp, "spec missing loader", None, None));
        }
        // This is possibly part of a namespace package.
        //  Remember these path entries (if any) for when we
        //  create a namespace package, and continue iterating
        //  on path.
        namespace_path.extend(interp.collect_iterable(&portions)?);
    }
    let module_spec = machinery_attr(interp, "ModuleSpec")?;
    let spec = call(interp, &module_spec, &[fullname.clone(), Value::none()])?;
    set(
        interp,
        &spec,
        "submodule_search_locations",
        Value::list(namespace_path),
    )?;
    Ok(spec)
}

pub(super) fn path_finder_find_spec(
    interp: &mut Interpreter,
    cls: &Value,
    fullname: &Value,
    path: Value,
    target: &Value,
) -> Result<Value> {
    let path = if path.is_none() {
        sys_attr(interp, "path")?
    } else {
        path
    };
    let spec = call_method(
        interp,
        cls,
        "_get_spec",
        &[fullname.clone(), path, target.clone()],
    )?;
    if spec.is_none() {
        return Ok(Value::none());
    }
    if !get(interp, &spec, "loader")?.is_none() {
        return Ok(spec);
    }
    let namespace_path = get(interp, &spec, "submodule_search_locations")?;
    if !truthy(interp, &namespace_path)? {
        return Ok(Value::none());
    }
    // We found at least one namespace path.  Return a spec which
    // can create the namespace package.
    set(interp, &spec, "origin", Value::none())?;
    let get_spec = get(interp, cls, "_get_spec")?;
    let class = hidden_class(interp, "_NamespacePath")?;
    let locations = call(
        interp,
        &class,
        &[fullname.clone(), namespace_path, get_spec],
    )?;
    set(interp, &spec, "submodule_search_locations", locations)?;
    Ok(spec)
}

// ── FileFinder ───────────────────────────────────────────────────────────────

pub(super) fn file_finder_init(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<()> {
    let (positional, keywords) = split_args(args);
    let mut positional = positional.into_iter();
    let this = positional.next().unwrap_or_else(Value::none);
    let mut path = positional.next();
    for (keyword, value) in keywords {
        if keyword != "path" {
            return Err(type_error(format!(
                "FileFinder.__init__() got an unexpected keyword argument '{keyword}'"
            )));
        }
        if path.is_some() {
            return Err(type_error(
                "FileFinder.__init__() got multiple values for argument 'path'",
            ));
        }
        path = Some(value);
    }
    let Some(path) = path else {
        return Err(type_error(
            "FileFinder.__init__() missing 1 required positional argument: 'path'",
        ));
    };
    let mut loaders = Vec::new();
    for detail in positional {
        let (loader, suffixes) = unpack_pair(interp, &detail)?;
        for suffix in interp.collect_iterable(&suffixes)? {
            loaders.push(Value::tuple(vec![suffix, loader.clone()]));
        }
    }
    set(interp, &this, "_loaders", Value::list(loaders))?;
    // Base (directory) path
    let is_dot = path.as_str() == Some(".");
    let directory = if !truthy(interp, &path)? || is_dot {
        getcwd(interp)?
    } else {
        let text = util::fspath_str(interp, &path)?;
        Value::string(util::path_abspath(interp, &text)?)
    };
    set(interp, &this, "path", directory)?;
    set(interp, &this, "_path_mtime", Value::int(-1))?;
    let path_cache = str_set(interp, Vec::new())?;
    set(interp, &this, "_path_cache", path_cache)?;
    let relaxed_path_cache = str_set(interp, Vec::new())?;
    set(interp, &this, "_relaxed_path_cache", relaxed_path_cache)
}

pub(super) fn file_finder_get_spec(
    interp: &mut Interpreter,
    loader_class: &Value,
    fullname: &Value,
    path: &Value,
    smsl: Value,
) -> Result<Value> {
    let loader = call(interp, loader_class, &[fullname.clone(), path.clone()])?;
    util::spec_from_file_location_impl(interp, fullname, Some(path.clone()), loader, Some(smsl))
}

/// `_path_stat(path).st_mtime`, or `-1` on `OSError`.
fn directory_mtime(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    if let Some(text) = path.as_str() {
        let mtime = std::fs::metadata(text)
            .and_then(|metadata| metadata.modified())
            .ok()
            .map(
                |modified| match modified.duration_since(std::time::UNIX_EPOCH) {
                    Ok(after) => after.as_secs_f64(),
                    Err(before) => -before.duration().as_secs_f64(),
                },
            );
        return Ok(mtime.map_or_else(|| Value::int(-1), Value::float));
    }
    match attempt(interp, "OSError", |interp| path_stat(interp, path))? {
        Some(stat_info) => get(interp, &stat_info, "st_mtime"),
        None => Ok(Value::int(-1)),
    }
}

/// `_relax_case()`: `PYTHONCASEOK` on case-insensitive platforms.
#[cfg(any(windows, target_os = "macos"))]
fn relax_case(interp: &mut Interpreter) -> Result<bool> {
    let flags = sys_attr(interp, "flags")?;
    let ignore_environment = get(interp, &flags, "ignore_environment")?;
    if truthy(interp, &ignore_environment)? {
        return Ok(false);
    }
    Ok(std::env::var_os("PYTHONCASEOK").is_some())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn relax_case(interp: &mut Interpreter) -> Result<bool> {
    let _ = interp;
    Ok(false)
}

pub(super) fn file_finder_find_spec(
    interp: &mut Interpreter,
    this: &Value,
    fullname: &Value,
    target: &Value,
) -> Result<Value> {
    let tail_module = rpartition_dot(interp, fullname)?.1;
    let tail_module = str_of(interp, &tail_module)?;
    let path = get(interp, this, "path")?;
    let stat_path = if truthy(interp, &path)? {
        path.clone()
    } else {
        getcwd(interp)?
    };
    let mtime = directory_mtime(interp, &stat_path)?;
    let path_mtime = get(interp, this, "_path_mtime")?;
    if ne(interp, &mtime, &path_mtime)? {
        call_method(interp, this, "_fill_cache", &[])?;
        set(interp, this, "_path_mtime", mtime)?;
    }
    // tail_module keeps the original casing, for __file__ and friends
    let (cache, cache_module) = if relax_case(interp)? {
        (
            get(interp, this, "_relaxed_path_cache")?,
            tail_module.to_lowercase(),
        )
    } else {
        (get(interp, this, "_path_cache")?, tail_module.clone())
    };
    let directory = str_of(interp, &path)?;
    let loaders = get(interp, this, "_loaders")?;
    let mut entries = Vec::new();
    for entry in interp.collect_iterable(&loaders)? {
        let (suffix, loader_class) = unpack_pair(interp, &entry)?;
        let suffix = str_of(interp, &suffix)?;
        entries.push((suffix, loader_class));
    }
    // Check if the module is the name of a directory (and thus a package).
    let mut namespace_base = None;
    if contains(interp, &cache, &s(&cache_module))? {
        let base_path = util::path_join(&[&directory, &tail_module]);
        for (suffix, loader_class) in &entries {
            let full_path = util::path_join(&[&base_path, &format!("__init__{suffix}")]);
            if path_isfile(&full_path) {
                return call_method(
                    interp,
                    this,
                    "_get_spec",
                    &[
                        loader_class.clone(),
                        fullname.clone(),
                        Value::string(full_path),
                        Value::list(vec![Value::string(base_path)]),
                        target.clone(),
                    ],
                );
            }
        }
        // If a namespace package, return the path if we don't
        //  find a module in the next section.
        if std::path::Path::new(&base_path).is_dir() {
            namespace_base = Some(base_path);
        }
    }
    // Check for a file w/ a proper suffix exists.
    for (suffix, loader_class) in &entries {
        let full_path = util::path_join(&[&directory, &format!("{tail_module}{suffix}")]);
        if contains(interp, &cache, &s(&format!("{cache_module}{suffix}")))?
            && path_isfile(&full_path)
        {
            return call_method(
                interp,
                this,
                "_get_spec",
                &[
                    loader_class.clone(),
                    fullname.clone(),
                    Value::string(full_path),
                    Value::none(),
                    target.clone(),
                ],
            );
        }
    }
    if let Some(base_path) = namespace_base {
        let module_spec = machinery_attr(interp, "ModuleSpec")?;
        let spec = call(interp, &module_spec, &[fullname.clone(), Value::none()])?;
        set(
            interp,
            &spec,
            "submodule_search_locations",
            Value::list(vec![Value::string(base_path)]),
        )?;
        return Ok(spec);
    }
    Ok(Value::none())
}

/// `os.listdir(path)`, with `FileNotFoundError`, `PermissionError` and
/// `NotADirectoryError` reading as an empty directory.
fn list_directory(interp: &mut Interpreter, path: &Value) -> Result<Vec<String>> {
    if let Some(text) = path.as_str() {
        match std::fs::read_dir(text) {
            Ok(entries) => {
                return Ok(entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::NotADirectory
                ) =>
            {
                return Ok(Vec::new());
            }
            // Anything else: let `os.listdir` raise CPython's error.
            Err(_) => {}
        }
    }
    let os = interp.load_module("os")?;
    let listed = attempt_any(
        interp,
        &["FileNotFoundError", "PermissionError", "NotADirectoryError"],
        |interp| call_method(interp, &os, "listdir", std::slice::from_ref(path)),
    )?;
    // Directory has either been removed, turned into a file, or made
    // unreadable.
    let Ok(listed) = listed else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for name in interp.collect_iterable(&listed)? {
        names.push(str_of(interp, &name)?);
    }
    Ok(names)
}

pub(super) fn file_finder_fill_cache(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let path = get(interp, this, "path")?;
    let directory = if truthy(interp, &path)? {
        path
    } else {
        getcwd(interp)?
    };
    let contents = list_directory(interp, &directory)?;
    // We store two cached versions, to handle runtime changes of the
    // PYTHONCASEOK environment variable.
    #[cfg(not(windows))]
    let path_cache = str_set(interp, contents.iter().cloned())?;
    // Windows users can import modules with case-insensitive file
    // suffixes (for legacy reasons). Make the suffix lowercase here
    // so it's done once instead of for every import. This is safe as
    // the specified suffixes to check against are always specified in a
    // case-sensitive manner.
    #[cfg(windows)]
    let path_cache = str_set(
        interp,
        contents.iter().map(|item| match item.split_once('.') {
            Some((name, suffix)) => format!("{name}.{}", suffix.to_lowercase()),
            None => item.clone(),
        }),
    )?;
    set(interp, this, "_path_cache", path_cache)?;
    #[cfg(any(windows, target_os = "macos"))]
    {
        let relaxed = str_set(interp, contents.iter().map(|name| name.to_lowercase()))?;
        set(interp, this, "_relaxed_path_cache", relaxed)?;
    }
    Ok(())
}

/// `path_hook_for_FileFinder(path)`.
pub(super) fn path_hook_call(
    interp: &mut Interpreter,
    cls: &Value,
    loader_details: &[Value],
    path: Value,
) -> Result<Value> {
    if !path_isdir(interp, &path)? {
        return Err(import_error(
            interp,
            "only directories are supported",
            None,
            Some(path),
        ));
    }
    let mut args = vec![path];
    args.extend(loader_details.iter().cloned());
    call(interp, cls, &args)
}
