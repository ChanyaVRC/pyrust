//! `ModuleSpec`, `BuiltinImporter`, `FrozenImporter` (`_bootstrap`) and the
//! file loaders of `_bootstrap_external`: `_LoaderBasics`, `SourceLoader`,
//! `FileLoader`, `SourceFileLoader`, `SourcelessFileLoader` and
//! `ExtensionFileLoader`.

use std::io::Write as _;

use super::*;
use crate::interpreter::builtin_args::{FromValue, PyBytesLike};

// ── ModuleSpec ───────────────────────────────────────────────────────────────

pub(super) fn spec_init(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<()> {
    let mut b = args::bind(&SIG_SPEC_INIT, args)?;
    let this = b.take_or_none(0);
    let name = b.take_or_none(1);
    let loader = b.take_or_none(2);
    let origin = b.take_or_none(3);
    let loader_state = b.take_or_none(4);
    let is_package = b.take_or_none(5);
    set(interp, &this, "name", name)?;
    set(interp, &this, "loader", loader)?;
    set(interp, &this, "origin", origin)?;
    set(interp, &this, "loader_state", loader_state)?;
    let locations = if interp.truthy_value(&is_package)? {
        Value::list(Vec::new())
    } else {
        Value::none()
    };
    set(interp, &this, "submodule_search_locations", locations)?;
    set(
        interp,
        &this,
        "_uninitialized_submodules",
        Value::list(Vec::new()),
    )?;
    // file-location attributes
    set(interp, &this, "_set_fileattr", Value::bool_(false))?;
    set(interp, &this, "_cached", Value::none())
}

pub(super) fn spec_repr(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let name = get(interp, this, "name")?;
    let loader = get(interp, this, "loader")?;
    let mut parts = vec![
        format!("name={}", repr_of(interp, &name)?),
        format!("loader={}", repr_of(interp, &loader)?),
    ];
    let origin = get(interp, this, "origin")?;
    if !origin.is_none() {
        parts.push(format!("origin={}", repr_of(interp, &origin)?));
    }
    let locations = get(interp, this, "submodule_search_locations")?;
    if !locations.is_none() {
        let text = call(interp, &builtin("format"), &[locations])?;
        let text = str_of(interp, &text)?;
        parts.push(format!("submodule_search_locations={text}"));
    }
    let class = get(interp, this, "__class__")?;
    let class_name = get(interp, &class, "__name__")?;
    let class_name = str_of(interp, &class_name)?;
    Ok(Value::string(format!("{class_name}({})", parts.join(", "))))
}

/// The `and` chain of `ModuleSpec.__eq__`; `AttributeError` escapes.
fn spec_eq_fields(
    interp: &mut Interpreter,
    this: &Value,
    other: &Value,
    locations: &Value,
) -> Result<Value> {
    let mut last = Value::bool_(true);
    for field in [
        "name",
        "loader",
        "origin",
        "submodule_search_locations",
        "cached",
        "has_location",
    ] {
        let mine = if field == "submodule_search_locations" {
            locations.clone()
        } else {
            get(interp, this, field)?
        };
        let theirs = get(interp, other, field)?;
        last = eq(interp, &mine, &theirs)?;
        if !truthy(interp, &last)? {
            return Ok(last);
        }
    }
    Ok(last)
}

pub(super) fn spec_eq(interp: &mut Interpreter, this: &Value, other: &Value) -> Result<Value> {
    let locations = get(interp, this, "submodule_search_locations")?;
    let result = attempt(interp, "AttributeError", |interp| {
        spec_eq_fields(interp, this, other, &locations)
    })?;
    Ok(result.unwrap_or_else(Value::not_implemented))
}

/// `_bootstrap_external._get_cached(filename)`.
pub(crate) fn get_cached(interp: &mut Interpreter, filename: &Value) -> Result<Value> {
    let source = machinery_attr(interp, "SOURCE_SUFFIXES")?;
    if ends_with_any(interp, filename, &source)? {
        let cached = attempt(interp, "NotImplementedError", |interp| {
            util::cache_from_source_impl(interp, filename, &Value::none(), Value::none())
                .map(Value::string)
        })?;
        return Ok(cached.unwrap_or_else(Value::none));
    }
    let bytecode = machinery_attr(interp, "BYTECODE_SUFFIXES")?;
    if ends_with_any(interp, filename, &bytecode)? {
        return Ok(filename.clone());
    }
    Ok(Value::none())
}

pub(super) fn spec_cached(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let cached = get(interp, this, "_cached")?;
    if cached.is_none() {
        let origin = get(interp, this, "origin")?;
        if !origin.is_none() {
            let set_fileattr = get(interp, this, "_set_fileattr")?;
            if truthy(interp, &set_fileattr)? {
                let cached = get_cached(interp, &origin)?;
                set(interp, this, "_cached", cached)?;
            }
        }
        return get(interp, this, "_cached");
    }
    Ok(cached)
}

pub(super) fn spec_parent(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let locations = get(interp, this, "submodule_search_locations")?;
    let name = get(interp, this, "name")?;
    if locations.is_none() {
        Ok(rpartition_dot(interp, &name)?.0)
    } else {
        Ok(name)
    }
}

// ── `_bootstrap` loader helpers ──────────────────────────────────────────────

/// `fullname in sys.builtin_module_names`.
fn is_builtin_name(interp: &mut Interpreter, fullname: &Value) -> Result<bool> {
    let names = sys_attr(interp, "builtin_module_names")?;
    contains(interp, &names, fullname)
}

/// `_requires_builtin`.
pub(super) fn requires_builtin(interp: &mut Interpreter, fullname: &Value) -> Result<()> {
    if is_builtin_name(interp, fullname)? {
        return Ok(());
    }
    let repr = repr_of(interp, fullname)?;
    Err(import_error(
        interp,
        format!("{repr} is not a built-in module"),
        Some(fullname.clone()),
        None,
    ))
}

pub(super) fn builtin_find_spec(
    interp: &mut Interpreter,
    cls: &Value,
    fullname: &Value,
) -> Result<Value> {
    // `_imp.is_builtin(fullname)` takes a `str` (or subclass).
    if !super::finders::is_str(interp, fullname)? {
        return Err(type_error(format!(
            "is_builtin() argument must be str, not {}",
            crate::interpreter::value_type_name_str(fullname)
        )));
    }
    if !is_builtin_name(interp, fullname)? {
        return Ok(Value::none());
    }
    let origin = get(interp, cls, "_ORIGIN")?;
    util::spec_from_loader_impl(interp, fullname, cls, origin, Value::none())
}

pub(super) fn builtin_create_module(interp: &mut Interpreter, spec: &Value) -> Result<Value> {
    let name = get(interp, spec, "name")?;
    if !is_builtin_name(interp, &name)? {
        let repr = repr_of(interp, &name)?;
        return Err(import_error(
            interp,
            format!("{repr} is not a built-in module"),
            Some(name),
            None,
        ));
    }
    let imp = interp.load_module("_imp")?;
    call_method(interp, &imp, "create_builtin", std::slice::from_ref(spec))
}

/// `_imp.is_frozen(fullname)`.
fn is_frozen(interp: &mut Interpreter, fullname: &Value) -> Result<bool> {
    let imp = interp.load_module("_imp")?;
    let frozen = call_method(interp, &imp, "is_frozen", std::slice::from_ref(fullname))?;
    interp.truthy_value(&frozen)
}

/// `_requires_frozen`.
pub(super) fn requires_frozen(interp: &mut Interpreter, fullname: &Value) -> Result<()> {
    if is_frozen(interp, fullname)? {
        return Ok(());
    }
    let repr = repr_of(interp, fullname)?;
    Err(import_error(
        interp,
        format!("{repr} is not a frozen module"),
        Some(fullname.clone()),
        None,
    ))
}

/// `_imp.get_frozen_object(name)` for a module PyRust never freezes.
pub(super) fn no_frozen_object(interp: &mut Interpreter, name: Value) -> PyError {
    let repr = match repr_of(interp, &name) {
        Ok(repr) => repr,
        Err(error) => return error,
    };
    import_error(
        interp,
        format!("No such frozen object named {repr}"),
        Some(name),
        None,
    )
}

pub(super) fn frozen_find_spec(
    interp: &mut Interpreter,
    cls: &Value,
    fullname: &Value,
) -> Result<Value> {
    if !is_frozen(interp, fullname)? {
        return Ok(Value::none());
    }
    let origin = get(interp, cls, "_ORIGIN")?;
    let imp = interp.load_module("_imp")?;
    let is_package = call_method(
        interp,
        &imp,
        "is_frozen_package",
        std::slice::from_ref(fullname),
    )?;
    util::spec_from_loader_impl(interp, fullname, cls, origin, is_package)
}

pub(super) fn frozen_create_module(interp: &mut Interpreter, spec: &Value) -> Result<Value> {
    let name = get(interp, spec, "name")?;
    let module_type = Value::py_class(crate::interpreter::module_type_singleton());
    let module = call(interp, &module_type, &[name])?;
    let state = get(interp, spec, "loader_state")?;
    let filename = attempt(interp, "AttributeError", |interp| {
        interp_get(interp, &state, "filename")
    })?;
    if let Some(filename) = filename
        && truthy(interp, &filename)?
    {
        set(interp, &module, "__file__", filename)?;
    }
    Ok(module)
}

fn interp_get(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<Value> {
    interp.get_attr(obj, name)
}

pub(super) fn frozen_resolve_filename(
    interp: &mut Interpreter,
    cls: &Value,
    fullname: &Value,
    alias: &Value,
    ispkg: &Value,
) -> Result<Value> {
    let none_pair = || Value::tuple(vec![Value::none(), Value::none()]);
    if !truthy(interp, fullname)? {
        return Ok(none_pair());
    }
    let sys = interp.load_module("sys")?;
    let stdlib_dir = get_attr_opt(interp, &sys, "_stdlib_dir")?.unwrap_or_else(Value::none);
    if !truthy(interp, &stdlib_dir)? {
        return Ok(none_pair());
    }
    let sep = match attempt(interp, "AttributeError", |interp| {
        interp_get(interp, cls, "_SEP")
    })? {
        Some(sep) => sep,
        None => {
            let sep = s(&util::PATH_SEP.to_string());
            set(interp, cls, "_SEP", sep.clone())?;
            sep
        }
    };
    let sep = str_of(interp, &sep)?;
    let stdlib_dir = str_of(interp, &stdlib_dir)?;
    let mut fullname = str_of(interp, fullname)?;
    let mut ispkg = truthy(interp, ispkg)?;
    if ne(interp, &s(&fullname), alias)? {
        if let Some(stripped) = fullname.strip_prefix('<') {
            fullname = if ispkg {
                stripped.to_string()
            } else {
                format!("{stripped}.__init__")
            };
        } else {
            ispkg = false;
        }
    }
    let relfile = fullname.replace('.', &sep);
    if ispkg {
        let pkgdir = format!("{stdlib_dir}{sep}{relfile}");
        let filename = format!("{pkgdir}{sep}__init__.py");
        Ok(Value::tuple(vec![
            Value::string(filename),
            Value::string(pkgdir),
        ]))
    } else {
        let filename = format!("{stdlib_dir}{sep}{relfile}.py");
        Ok(Value::tuple(vec![Value::string(filename), Value::none()]))
    }
}

// ── `_load_module_shim` ──────────────────────────────────────────────────────

fn sys_modules(interp: &mut Interpreter) -> Result<Value> {
    sys_attr(interp, "modules")
}

/// `module = sys.modules.pop(name); sys.modules[name] = module`.
fn move_to_end_of_sys_modules(interp: &mut Interpreter, name: &Value) -> Result<Value> {
    let modules = sys_modules(interp)?;
    let module = call_method(interp, &modules, "pop", std::slice::from_ref(name))?;
    call_method(
        interp,
        &modules,
        "__setitem__",
        &[name.clone(), module.clone()],
    )?;
    Ok(module)
}

/// `_bootstrap._exec(spec, module)`.
fn exec_spec(interp: &mut Interpreter, spec: &Value, module: &Value) -> Result<Value> {
    let name = get(interp, spec, "name")?;
    let modules = sys_modules(interp)?;
    let current = call_method(interp, &modules, "get", std::slice::from_ref(&name))?;
    if !current.is_identical_to(module) {
        let repr = repr_of(interp, &name)?;
        return Err(import_error(
            interp,
            format!("module {repr} not in sys.modules"),
            Some(name),
            None,
        ));
    }
    let result = exec_spec_body(interp, spec, module);
    let spec_name = get(interp, spec, "name")?;
    let moved = move_to_end_of_sys_modules(interp, &spec_name);
    result?;
    moved
}

fn exec_spec_body(interp: &mut Interpreter, spec: &Value, module: &Value) -> Result<()> {
    let loader = get(interp, spec, "loader")?;
    if loader.is_none() {
        let locations = get(interp, spec, "submodule_search_locations")?;
        if locations.is_none() {
            let name = get(interp, spec, "name")?;
            return Err(import_error(interp, "missing loader", Some(name), None));
        }
        // Namespace package.
        util::init_module_attrs_impl(interp, spec, module, true)?;
        return Ok(());
    }
    util::init_module_attrs_impl(interp, spec, module, true)?;
    let loader = get(interp, spec, "loader")?;
    call_method(interp, &loader, "exec_module", std::slice::from_ref(module))?;
    Ok(())
}

/// `_bootstrap._load_unlocked(spec)`.
fn load_unlocked(interp: &mut Interpreter, spec: &Value) -> Result<Value> {
    let module = util::module_from_spec_impl(interp, spec)?;
    set(interp, spec, "_initializing", Value::bool_(true))?;
    let result = load_unlocked_body(interp, spec, &module);
    set(interp, spec, "_initializing", Value::bool_(false))?;
    result
}

fn load_unlocked_body(interp: &mut Interpreter, spec: &Value, module: &Value) -> Result<Value> {
    let name = get(interp, spec, "name")?;
    let modules = sys_modules(interp)?;
    call_method(
        interp,
        &modules,
        "__setitem__",
        &[name.clone(), module.clone()],
    )?;
    let loader = get(interp, spec, "loader")?;
    let executed = if loader.is_none() {
        let locations = get(interp, spec, "submodule_search_locations")?;
        if locations.is_none() {
            Err(import_error(
                interp,
                "missing loader",
                Some(name.clone()),
                None,
            ))
        } else {
            // A namespace package so do nothing.
            Ok(())
        }
    } else {
        call_method(interp, &loader, "exec_module", std::slice::from_ref(module)).map(|_| ())
    };
    if let Err(error) = executed {
        let _ = call_method(interp, &modules, "pop", &[name.clone(), Value::none()]);
        return Err(error);
    }
    move_to_end_of_sys_modules(interp, &name)
}

/// `_bootstrap._load_module_shim(self, fullname)`.
pub(super) fn load_module_shim(
    interp: &mut Interpreter,
    loader: &Value,
    fullname: &Value,
) -> Result<Value> {
    let warnings = interp.load_module("warnings")?;
    let warn = get(interp, &warnings, "warn")?;
    let category = exc_class("DeprecationWarning")?;
    call(
        interp,
        &warn,
        &[
            s(
                "the load_module() method is deprecated and slated for removal in Python 3.12; use exec_module() instead",
            ),
            category,
        ],
    )?;
    let spec = util::spec_from_loader_impl(interp, fullname, loader, Value::none(), Value::none())?;
    let modules = sys_modules(interp)?;
    if contains(interp, &modules, fullname)? {
        let module = call_method(
            interp,
            &modules,
            "__getitem__",
            std::slice::from_ref(fullname),
        )?;
        exec_spec(interp, &spec, &module)?;
        let modules = sys_modules(interp)?;
        return call_method(
            interp,
            &modules,
            "__getitem__",
            std::slice::from_ref(fullname),
        );
    }
    load_unlocked(interp, &spec)
}

// ── `_LoaderBasics` ──────────────────────────────────────────────────────────

pub(super) fn basics_is_package(
    interp: &mut Interpreter,
    this: &Value,
    fullname: &Value,
) -> Result<Value> {
    let filename = call_method(interp, this, "get_filename", std::slice::from_ref(fullname))?;
    let filename = util::fspath_str(interp, &filename)?;
    let filename = util::path_split(&filename).1;
    let filename_base = filename.rsplit_once('.').map_or(filename, |(base, _)| base);
    let tail_name = rpartition_dot(interp, fullname)?.1;
    let tail_is_init = tail_name.as_str() == Some("__init__");
    Ok(Value::bool_(filename_base == "__init__" && !tail_is_init))
}

pub(super) fn basics_exec_module(
    interp: &mut Interpreter,
    this: &Value,
    module: &Value,
) -> Result<()> {
    let name = get(interp, module, "__name__")?;
    let code = call_method(interp, this, "get_code", &[name])?;
    if code.is_none() {
        let name = get(interp, module, "__name__")?;
        let repr = repr_of(interp, &name)?;
        return Err(import_error(
            interp,
            format!("cannot load module {repr} when get_code() returns None"),
            None,
            None,
        ));
    }
    let namespace = get(interp, module, "__dict__")?;
    call(interp, &builtin("exec"), &[code, namespace])?;
    Ok(())
}

// ── `SourceLoader` ───────────────────────────────────────────────────────────

pub(super) fn source_get_source(
    interp: &mut Interpreter,
    this: &Value,
    fullname: &Value,
) -> Result<Value> {
    let path = call_method(interp, this, "get_filename", std::slice::from_ref(fullname))?;
    let read = attempt_any(interp, &["OSError"], |interp| {
        call_method(interp, this, "get_data", &[path])
    })?;
    let source_bytes = match read {
        Ok(data) => data,
        Err(cause) => {
            let error = import_error(
                interp,
                "source not available through get_data()",
                Some(fullname.clone()),
                None,
            );
            return Err(util::from_cause(interp, error, cause));
        }
    };
    util::decode_source_impl(interp, &source_bytes)
}

pub(super) fn source_to_code(
    interp: &mut Interpreter,
    data: Value,
    path: Value,
    optimize: Value,
) -> Result<Value> {
    call_kw(
        interp,
        &builtin("compile"),
        &[data, path, s("exec")],
        &[("dont_inherit", Value::bool_(true)), ("optimize", optimize)],
    )
}

/// `SourceLoader.get_code`.  PyRust cannot execute CPython bytecode, so it
/// probes the cache exactly like CPython (`path_stats`, then
/// `get_data(bytecode_path)`) but always compiles the source, and never
/// writes `__pycache__` entries (its code objects do not marshal to CPython
/// bytecode).
pub(super) fn source_get_code(
    interp: &mut Interpreter,
    this: &Value,
    fullname: &Value,
) -> Result<Value> {
    let source_path = call_method(interp, this, "get_filename", std::slice::from_ref(fullname))?;
    let bytecode_path = attempt(interp, "NotImplementedError", |interp| {
        util::cache_from_source_impl(interp, &source_path, &Value::none(), Value::none())
            .map(Value::string)
    })?;
    if let Some(bytecode_path) = bytecode_path {
        let stats = attempt(interp, "OSError", |interp| {
            call_method(
                interp,
                this,
                "path_stats",
                std::slice::from_ref(&source_path),
            )
        })?;
        if let Some(stats) = stats {
            let mtime = crate::builtin_modules::operator::getitem_value(interp, stats, s("mtime"))?;
            call(interp, &builtin("int"), &[mtime])?;
            // The cached bytecode, if any, is read and discarded.
            attempt(interp, "OSError", |interp| {
                call_method(interp, this, "get_data", &[bytecode_path])
            })?;
        }
    }
    let source_bytes = call_method(interp, this, "get_data", std::slice::from_ref(&source_path))?;
    call_method(interp, this, "source_to_code", &[source_bytes, source_path])
}

// ── `FileLoader` ─────────────────────────────────────────────────────────────

pub(super) fn file_loader_eq(
    interp: &mut Interpreter,
    this: &Value,
    other: &Value,
) -> Result<Value> {
    let mine = get(interp, this, "__class__")?;
    let theirs = get(interp, other, "__class__")?;
    let same_class = eq(interp, &mine, &theirs)?;
    if !truthy(interp, &same_class)? {
        return Ok(same_class);
    }
    let mine = get(interp, this, "__dict__")?;
    let theirs = get(interp, other, "__dict__")?;
    eq(interp, &mine, &theirs)
}

pub(super) fn file_loader_hash(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let name = get(interp, this, "name")?;
    let path = get(interp, this, "path")?;
    let name_hash = call(interp, &builtin("hash"), &[name])?;
    let path_hash = call(interp, &builtin("hash"), &[path])?;
    interp.eval_binary(name_hash, BinaryOp::BitXor, path_hash)
}

/// `isinstance(self, (SourceLoader, ExtensionFileLoader))`.
fn reads_with_open_code(interp: &mut Interpreter, this: &Value) -> Result<bool> {
    let source_loader = hidden_class(interp, "SourceLoader")?;
    let extension_loader = machinery_attr(interp, "ExtensionFileLoader")?;
    let classes = Value::tuple(vec![source_loader, extension_loader]);
    let result = call(interp, &builtin("isinstance"), &[this.clone(), classes])?;
    interp.truthy_value(&result)
}

pub(super) fn file_get_data(interp: &mut Interpreter, this: &Value, path: &Value) -> Result<Value> {
    // A `str` path reads the same through either branch; any failure is
    // re-raised by the `_io` call for CPython's exact `OSError`.
    if let Some(text) = path.as_str()
        && let Ok(data) = std::fs::read(text)
    {
        return Ok(Value::bytes(data));
    }
    // `_io.open_code(str(path))` / `_io.FileIO(path, 'r')`: both raise what
    // `open(path, 'rb')` raises.
    let target = if reads_with_open_code(interp, this)? {
        call(interp, &builtin("str"), std::slice::from_ref(path))?
    } else {
        path.clone()
    };
    let file = call(interp, &builtin("open"), &[target, s("rb")])?;
    let data = call_method(interp, &file, "read", &[]);
    let closed = call_method(interp, &file, "close", &[]);
    let data = data?;
    closed?;
    Ok(data)
}

// ── `SourceFileLoader` ───────────────────────────────────────────────────────

/// `_path_stat(path)` (`os.stat`).
fn path_stat(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let os = interp.load_module("os")?;
    call_method(interp, &os, "stat", std::slice::from_ref(path))
}

pub(super) fn source_file_path_stats(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let st = path_stat(interp, path)?;
    let stats = new_dict();
    let mtime = get(interp, &st, "st_mtime")?;
    dict_set(&stats, "mtime", mtime)?;
    let size = get(interp, &st, "st_size")?;
    dict_set(&stats, "size", size)?;
    Ok(stats)
}

/// `_calc_mode(path)`: the source file's mode, writable by the owner.
pub(super) fn calc_mode(interp: &mut Interpreter, path: &Value) -> Result<i64> {
    let st = attempt(interp, "OSError", |interp| path_stat(interp, path))?;
    let mode = match st {
        Some(st) => get(interp, &st, "st_mode")?.as_int().unwrap_or(0o666),
        None => 0o666,
    };
    Ok(mode | 0o200)
}

/// `_write_atomic(path, data, mode)`: write to a temporary sibling, then
/// replace `path` with it.
fn write_atomic(path: &str, data: &[u8], mode: i64) -> std::io::Result<()> {
    let temporary = format!("{path}.{}", path.as_ptr() as usize);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode((mode & 0o666) as u32);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let written = options
        .open(&temporary)
        .and_then(|mut file| file.write_all(data))
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// `SourceFileLoader.set_data`: create the missing parent directories and
/// write `data`, silently giving up on any `OSError`.
pub(super) fn source_file_set_data(
    interp: &mut Interpreter,
    path: &Value,
    data: &Value,
    mode: &Value,
) -> Result<()> {
    let path = util::fspath_str(interp, path)?;
    let data = PyBytesLike::try_from_value_with_interp(interp, data, "", "").map_err(|_| {
        type_error(format!(
            "a bytes-like object is required, not '{}'",
            crate::interpreter::value_type_name_str(data)
        ))
    })?;
    let mode = mode.as_int().unwrap_or(0o666);
    let (mut parent, _) = util::path_split(&path);
    let mut path_parts = Vec::new();
    // Figure out what directories are missing.
    while !parent.is_empty() && !std::path::Path::new(parent).is_dir() {
        let (head, part) = util::path_split(parent);
        path_parts.push(part);
        parent = head;
    }
    // Create needed directories.
    let mut parent = parent.to_string();
    for part in path_parts.into_iter().rev() {
        parent = util::path_join(&[&parent, part]);
        match std::fs::create_dir(&parent) {
            Ok(()) => {}
            // Probably another process already created the dir.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            // Could be a permission error, read-only filesystem: just forget
            // about writing the data.
            Err(_) => return Ok(()),
        }
    }
    // Same as above: just don't write the bytecode.
    let _ = write_atomic(&path, data.as_bytes(), mode);
    Ok(())
}

// ── `SourcelessFileLoader` ───────────────────────────────────────────────────

/// `_classify_pyc(data, name, exc_details)`.
fn classify_pyc(interp: &mut Interpreter, data: &[u8], name: &Value, path: &Value) -> Result<u32> {
    let magic = &data[..data.len().min(4)];
    if magic != crate::builtin_modules::py_compile::PYC_MAGIC_NUMBER {
        let name_repr = repr_of(interp, name)?;
        let magic_repr = repr_of(interp, &Value::bytes(magic.to_vec()))?;
        return Err(import_error(
            interp,
            format!("bad magic number in {name_repr}: {magic_repr}"),
            Some(name.clone()),
            Some(path.clone()),
        ));
    }
    if data.len() < 16 {
        let name_repr = repr_of(interp, name)?;
        return Err(raise_class(
            interp,
            "EOFError",
            &[Value::string(format!(
                "reached EOF while reading pyc header of {name_repr}"
            ))],
        ));
    }
    let flags = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    // Only the first two flags are defined.
    if flags & !0b11 != 0 {
        let name_repr = repr_of(interp, name)?;
        return Err(import_error(
            interp,
            format!("invalid flags {flags} in {name_repr}"),
            Some(name.clone()),
            Some(path.clone()),
        ));
    }
    Ok(flags)
}

pub(super) fn sourceless_get_code(
    interp: &mut Interpreter,
    this: &Value,
    fullname: &Value,
) -> Result<Value> {
    let path = call_method(interp, this, "get_filename", std::slice::from_ref(fullname))?;
    let data = call_method(interp, this, "get_data", std::slice::from_ref(&path))?;
    let bytes = PyBytesLike::try_from_value_with_interp(interp, &data, "", "")
        .map_err(|_| {
            type_error(format!(
                "a bytes-like object is required, not '{}'",
                crate::interpreter::value_type_name_str(&data)
            ))
        })?
        .into_vec();
    classify_pyc(interp, &bytes, fullname, &path)?;
    // `_compile_bytecode(memoryview(data)[16:], name=fullname, bytecode_path=path)`.
    let marshal = interp.load_module("marshal")?;
    let code = call_method(
        interp,
        &marshal,
        "loads",
        &[Value::bytes(bytes[16..].to_vec())],
    )?;
    let code_type = module_attr(interp, "types", "CodeType")?;
    let is_code = call(interp, &builtin("isinstance"), &[code.clone(), code_type])?;
    if interp.truthy_value(&is_code)? {
        return Ok(code);
    }
    let repr = repr_of(interp, &path)?;
    Err(import_error(
        interp,
        format!("Non-code object in {repr}"),
        Some(fullname.clone()),
        Some(path),
    ))
}

// ── `ExtensionFileLoader` ────────────────────────────────────────────────────

/// PyRust cannot load CPython extension modules.
pub(super) fn extension_unsupported(
    interp: &mut Interpreter,
    this: &Value,
    name: Value,
) -> PyError {
    let repr = match repr_of(interp, &name) {
        Ok(repr) => repr,
        Err(error) => return error,
    };
    let path = get_attr_opt(interp, this, "path").ok().flatten();
    import_error(
        interp,
        format!("extension module loading is not supported for {repr}"),
        Some(name),
        path,
    )
}

pub(super) fn extension_is_package(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let path = get(interp, this, "path")?;
    let path = util::fspath_str(interp, &path)?;
    let file_name = util::path_split(&path).1.to_string();
    let suffixes = machinery_attr(interp, "EXTENSION_SUFFIXES")?;
    for suffix in interp.collect_iterable(&suffixes)? {
        let suffix = str_of(interp, &suffix)?;
        if file_name == format!("__init__{suffix}") {
            return Ok(Value::bool_(true));
        }
    }
    Ok(Value::bool_(false))
}
