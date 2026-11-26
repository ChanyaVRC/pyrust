// `site` module — 100% native-Rust CPython 3.12 implementation using PyRust's
// native module macros (`pyrust_module!`, `PyPath`, `#[varargs]`).
//
// Automatically imported during CLI startup (unless `-S` is supplied) to
// configure `sys.path`, process `.pth` files, and install interactive builtins
// (`quit`, `exit`, `copyright`, `credits`, `license`, `help`).
//
// Reference: <https://docs.python.org/3/library/site.html>

use std::path::Path;

use pyrust_derive::pyrust_module;

#[cfg(windows)]
use crate::builtin_modules::ntpath::ops as path_ops;
#[cfg(not(windows))]
use crate::builtin_modules::posixpath::ops as path_ops;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyPath, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_instance_str, render_value_repr};
use crate::value::{PyDict, PyKey, PySet, Value, ValueKind};

const CREDITS_TEXT: &str = "Thanks to CWI, CNRI, BeOpen, Zope Corporation, the Python Software\n\
Foundation, and a cast of thousands for supporting Python\n\
development.  See www.python.org for more information.";

// ── internal helpers ─────────────────────────────────────────────────────────

fn site_module_rc(
    interp: &mut Interpreter,
) -> Option<std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>> {
    let sys_modules = interp.import_module_registry().ok()?;
    let val = sys_modules
        .as_dict()
        .and_then(|d| d.get(&PyKey::str_from("site")).cloned())?;
    match val.kind() {
        ValueKind::PyModule(rc) => Some(std::rc::Rc::clone(rc)),
        _ => None,
    }
}

fn get_site_attr(interp: &mut Interpreter, name: &str) -> Option<Value> {
    let m = site_module_rc(interp)?;
    m.borrow().get_attr_value(name)
}

fn set_site_attr(interp: &mut Interpreter, name: &str, value: Value) {
    if let Some(m) = site_module_rc(interp) {
        m.borrow_mut().insert_attr(name.to_string(), value);
    }
}

/// `os.path.join(*parts)` through the host path implementation `os.path`
/// itself delegates to, without importing `os.path` or dispatching a Python
/// call (issue #3293).
fn path_join(interp: &mut Interpreter, parts: &[Value]) -> Result<Value> {
    let Some((first, rest)) = parts.split_first() else {
        return Err(PyError::named(
            "TypeError",
            "join() missing 1 required positional argument: 'path'",
        ));
    };
    path_ops::join(interp, first, rest)
}

/// The cached `sys` module object, when `sys` has been imported.
fn cached_sys_module(
    interp: &Interpreter,
) -> Option<std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>> {
    let cached = interp.cold.module_cache.borrow().get("sys").cloned()?;
    match cached.kind() {
        ValueKind::PyModule(module) => Some(std::rc::Rc::clone(module)),
        _ => None,
    }
}

/// `sys.flags.<name>` as an integer.
///
/// `sys.flags` is deferred until a script reads it (issue #3293).  While it
/// is untouched it still describes exactly this interpreter's configuration,
/// so `site` answers from the configuration instead of forcing the
/// struct-sequence (and the primitive classes behind it) into existence
/// during startup.  Once a script has read or replaced `sys.flags`, the
/// Python-visible object is authoritative.
fn sys_flag(interp: &mut Interpreter, name: &str) -> Result<i64> {
    let untouched = cached_sys_module(interp)
        .is_some_and(|sys_module| sys_module.borrow().deferred_attr_is_unresolved("flags"));
    if untouched {
        let config = &interp.cold.runtime_config;
        return Ok(match name {
            "no_user_site" => config.no_user_site,
            "no_site" => config.no_site,
            _ => 0,
        });
    }
    let sys_mod = interp.load_module("sys")?;
    let flags = interp.get_attr(&sys_mod, "flags")?;
    Ok(interp
        .get_attr(&flags, name)
        .ok()
        .and_then(|v| v.as_int())
        .unwrap_or(0))
}

/// `sys.version_info[:2]`, without building the deferred `sys.version_info`
/// when no script has read or replaced it.
fn sys_version_major_minor(interp: &mut Interpreter) -> Result<(i64, i64)> {
    const DEFAULT: (i64, i64) = (3, 12);
    let untouched = cached_sys_module(interp).is_some_and(|sys_module| {
        sys_module
            .borrow()
            .deferred_attr_is_unresolved("version_info")
    });
    if untouched {
        return Ok(DEFAULT);
    }
    let sys_mod = interp.load_module("sys")?;
    let vi = interp.get_attr(&sys_mod, "version_info")?;
    let field = |interp: &mut Interpreter, name: &str, default: i64| {
        interp
            .get_attr(&vi, name)
            .ok()
            .and_then(|v| v.as_int())
            .unwrap_or(default)
    };
    Ok((
        field(interp, "major", DEFAULT.0),
        field(interp, "minor", DEFAULT.1),
    ))
}

fn makepath_strings(interp: &mut Interpreter, parts: &[String]) -> Result<(String, String)> {
    let part_vals: Vec<Value> = parts.iter().map(Value::string).collect();
    let joined = path_join(interp, &part_vals)?;
    let dir_val = match path_ops::abspath(interp, &joined) {
        Ok(v) => v,
        Err(ref e) if e.class_name_is("OSError") => joined,
        Err(e) => return Err(e),
    };
    let normcase_val = path_ops::normcase(interp, &dir_val)?;
    let dir = render_instance_str(interp, &dir_val)?;
    let dircase = render_instance_str(interp, &normcase_val)?;
    Ok((dir, dircase))
}

fn sys_path_value(interp: &mut Interpreter) -> Result<Value> {
    let sys_mod = interp.load_module("sys")?;
    interp.get_attr(&sys_mod, "path")
}

fn set_contains_str(interp: &mut Interpreter, known_paths: &Value, key: &str) -> Result<bool> {
    if let ValueKind::Set(set) = known_paths.kind() {
        return Ok(set.contains(&PyKey::str_from(key)));
    }
    let res = interp.eval_in(known_paths.clone(), Value::string(key))?;
    interp.truthy_value(&res)
}

fn set_add_str(interp: &mut Interpreter, known_paths: &Value, key: &str) -> Result<()> {
    let add_fn = interp.get_attr(known_paths, "add")?;
    interp.call_function_expanded(
        add_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(key),
        }],
    )?;
    Ok(())
}

fn init_pathinfo_impl(interp: &mut Interpreter) -> Result<Value> {
    let path_val = sys_path_value(interp)?;
    let items = interp.collect_iterable(&path_val)?;
    let mut known = PySet::default();
    for item in items {
        let Some(s) = item.as_str() else {
            continue;
        };
        if Path::new(s).exists()
            && let Ok((_, itemcase)) = makepath_strings(interp, &[s.to_string()])
        {
            known.insert(PyKey::str_from(&itemcase));
        }
    }
    Ok(Value::set(known))
}

fn write_stderr_text(interp: &mut Interpreter, text: &str) {
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(stderr) = interp.get_attr(&sys_mod, "stderr")
        && !stderr.is_none()
        && let Ok(write_fn) = interp.get_attr(&stderr, "write")
    {
        let _ = interp.call_function_expanded(
            write_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string(text),
            }],
        );
        return;
    }
    eprint!("{text}");
}

fn trace_site(interp: &mut Interpreter, msg: &str) {
    if interp.cold.runtime_config.verbose > 0 {
        write_stderr_text(interp, &format!("{msg}\n"));
    }
}

fn compute_userbase(interp: &mut Interpreter) -> Result<Option<String>> {
    let os_mod = interp.load_module("os")?;
    let environ = interp.get_attr(&os_mod, "environ")?;
    let get_fn = interp.get_attr(&environ, "get")?;
    let env_base = interp.call_function_expanded(
        get_fn.clone(),
        &[ExpandedCallArg {
            name: None,
            value: Value::string("PYTHONUSERBASE"),
        }],
    )?;
    if let Some(s) = env_base.as_str()
        && !s.is_empty()
    {
        return Ok(Some(s.to_string()));
    }

    if cfg!(windows) {
        let appdata = interp.call_function_expanded(
            get_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string("APPDATA"),
            }],
        )?;
        let base = appdata
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("~")
            .to_string();
        let joined = path_join(interp, &[Value::string(base), Value::string("Python")])?;
        let expanded = path_ops::expanduser(interp, &joined)?;
        Ok(Some(render_instance_str(interp, &expanded)?))
    } else {
        let joined = path_join(interp, &[Value::string("~"), Value::string(".local")])?;
        let expanded = path_ops::expanduser(interp, &joined)?;
        Ok(Some(render_instance_str(interp, &expanded)?))
    }
}

fn compute_usersite_path(interp: &mut Interpreter, userbase: &str) -> Result<String> {
    let (major, minor) = sys_version_major_minor(interp)?;
    if cfg!(windows) {
        Ok(format!("{userbase}\\Python{major}{minor}\\site-packages"))
    } else {
        Ok(format!(
            "{userbase}/lib/python{major}.{minor}/site-packages"
        ))
    }
}

fn is_hidden_pth_file(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        std::fs::symlink_metadata(path)
            .map(|m| (m.file_attributes() & FILE_ATTRIBUTE_HIDDEN) != 0)
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

fn addpackage_impl(
    interp: &mut Interpreter,
    sitedir: &str,
    name: &str,
    known_paths: Value,
) -> Result<Value> {
    let (known_paths, reset) = if known_paths.is_none() {
        (init_pathinfo_impl(interp)?, true)
    } else {
        (known_paths, false)
    };
    let fullname_val = path_join(interp, &[Value::string(sitedir), Value::string(name)])?;
    let fullname = render_instance_str(interp, &fullname_val)?;
    let full_path = Path::new(&fullname);
    if std::fs::symlink_metadata(full_path).is_err() {
        return Ok(Value::none());
    }
    if is_hidden_pth_file(full_path) {
        trace_site(interp, &format!("Skipping hidden .pth file: {fullname:?}"));
        return Ok(Value::none());
    }
    trace_site(interp, &format!("Processing .pth file: {fullname:?}"));
    let raw_bytes = match std::fs::read(full_path) {
        Ok(b) => b,
        Err(_) => return Ok(Value::none()),
    };
    // Decode UTF-8 (stripping UTF-8 BOM if present), falling back to Latin-1.
    let body = raw_bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF])
        .unwrap_or(&raw_bytes);
    let content = match std::str::from_utf8(body) {
        Ok(s) => s.to_owned(),
        Err(_) => body.iter().map(|&b| b as char).collect(),
    };

    let sys_path = sys_path_value(interp)?;
    for (idx, line) in content.lines().enumerate() {
        let lineno = idx + 1;
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let step_result: Result<()> = (|| {
            if line.starts_with("import ") || line.starts_with("import\t") {
                let exec_globals = Value::dict(PyDict::default());
                let builtins = interp.load_module("builtins")?;
                let sys_mod = interp.load_module("sys")?;
                let os_mod = interp.load_module("os")?;
                exec_globals.dict_insert(PyKey::str_from("__builtins__"), builtins)?;
                exec_globals.dict_insert(PyKey::str_from("sys"), sys_mod)?;
                exec_globals.dict_insert(PyKey::str_from("os"), os_mod)?;
                exec_globals.dict_insert(PyKey::str_from("sitedir"), Value::string(sitedir))?;
                exec_globals.dict_insert(PyKey::str_from("name"), Value::string(name))?;
                exec_globals.dict_insert(PyKey::str_from("known_paths"), known_paths.clone())?;
                interp.exec_source(line, Some(exec_globals), None)?;
                return Ok(());
            }
            let trimmed = line.trim_end();
            let (dir, dircase) =
                makepath_strings(interp, &[sitedir.to_string(), trimmed.to_string()])?;
            if !set_contains_str(interp, &known_paths, &dircase)? && Path::new(&dir).exists() {
                let append_fn = interp.get_attr(&sys_path, "append")?;
                interp.call_function_expanded(
                    append_fn,
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(dir),
                    }],
                )?;
                set_add_str(interp, &known_paths, &dircase)?;
            }
            Ok(())
        })();

        if let Err(err) = step_result {
            let err_line = interp.format_error_line(&err);
            write_stderr_text(
                interp,
                &format!(
                    "Error processing line {lineno} of {fullname}:\n\n  {err_line}\n\nRemainder of file ignored\n"
                ),
            );
            break;
        }
    }

    if reset {
        Ok(Value::none())
    } else {
        Ok(known_paths)
    }
}

fn addsitedir_impl(interp: &mut Interpreter, sitedir: &str, known_paths: Value) -> Result<Value> {
    trace_site(interp, &format!("Adding directory: {sitedir:?}"));
    let (known_paths, reset) = if known_paths.is_none() {
        (init_pathinfo_impl(interp)?, true)
    } else {
        (known_paths, false)
    };
    let (sitedir_abs, sitedircase) = makepath_strings(interp, &[sitedir.to_string()])?;
    if !set_contains_str(interp, &known_paths, &sitedircase)? {
        let sys_path = sys_path_value(interp)?;
        let append_fn = interp.get_attr(&sys_path, "append")?;
        interp.call_function_expanded(
            append_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string(&sitedir_abs),
            }],
        )?;
        set_add_str(interp, &known_paths, &sitedircase)?;
    }
    let Ok(entries) = std::fs::read_dir(&sitedir_abs) else {
        return Ok(Value::none());
    };
    let mut pth_names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.ends_with(".pth") && !name.starts_with('.'))
        .collect();
    pth_names.sort();
    for name in pth_names {
        addpackage_impl(interp, &sitedir_abs, &name, known_paths.clone())?;
    }
    if reset {
        Ok(Value::none())
    } else {
        Ok(known_paths)
    }
}

fn check_enableusersite_impl(interp: &mut Interpreter) -> Result<Value> {
    if sys_flag(interp, "no_user_site")? != 0 {
        return Ok(Value::bool_(false));
    }
    #[cfg(unix)]
    {
        let os_mod = interp.load_module("os")?;
        if let (Ok(getuid), Ok(geteuid)) = (
            interp.get_attr(&os_mod, "getuid"),
            interp.get_attr(&os_mod, "geteuid"),
        ) && let (Ok(uid), Ok(euid)) = (
            interp.call_function_expanded(getuid, &[]),
            interp.call_function_expanded(geteuid, &[]),
        ) && uid.as_int() != euid.as_int()
        {
            return Ok(Value::none());
        }
        if let (Ok(getgid), Ok(getegid)) = (
            interp.get_attr(&os_mod, "getgid"),
            interp.get_attr(&os_mod, "getegid"),
        ) && let (Ok(gid), Ok(egid)) = (
            interp.call_function_expanded(getgid, &[]),
            interp.call_function_expanded(getegid, &[]),
        ) && gid.as_int() != egid.as_int()
        {
            return Ok(Value::none());
        }
    }
    Ok(Value::bool_(true))
}

fn getuserbase_impl(interp: &mut Interpreter) -> Result<Value> {
    if let Some(existing) = get_site_attr(interp, "USER_BASE")
        && !existing.is_none()
    {
        return Ok(existing);
    }
    let computed = compute_userbase(interp)?;
    let val = computed.map(Value::string).unwrap_or_else(Value::none);
    set_site_attr(interp, "USER_BASE", val.clone());
    Ok(val)
}

fn getusersitepackages_impl(interp: &mut Interpreter) -> Result<Value> {
    let userbase = getuserbase_impl(interp)?;
    if let Some(existing) = get_site_attr(interp, "USER_SITE")
        && !existing.is_none()
    {
        return Ok(existing);
    }
    if userbase.is_none() {
        set_site_attr(interp, "ENABLE_USER_SITE", Value::bool_(false));
        return Ok(Value::none());
    }
    let userbase_str = render_instance_str(interp, &userbase)?;
    let path = compute_usersite_path(interp, &userbase_str)?;
    let val = Value::string(path);
    set_site_attr(interp, "USER_SITE", val.clone());
    Ok(val)
}

fn getsitepackages_impl(interp: &mut Interpreter, prefixes: Value) -> Result<Value> {
    let prefixes_val = if prefixes.is_none() {
        get_site_attr(interp, "PREFIXES").unwrap_or_else(|| Value::list(Vec::new()))
    } else {
        prefixes
    };
    let prefix_items = interp.collect_iterable(&prefixes_val)?;
    let sys_mod = interp.load_module("sys")?;
    let platlibdir = interp
        .get_attr(&sys_mod, "platlibdir")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| if cfg!(windows) { "DLLs" } else { "lib" }.to_string());
    let (major, minor) = sys_version_major_minor(interp)?;

    let mut sitepackages: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in prefix_items {
        let prefix = render_instance_str(interp, &item)?;
        if prefix.is_empty() || !seen.insert(prefix.clone()) {
            continue;
        }
        if cfg!(windows) {
            sitepackages.push(Value::string(&prefix));
            let lib_site = path_join(
                interp,
                &[
                    Value::string(&prefix),
                    Value::string("Lib"),
                    Value::string("site-packages"),
                ],
            )?;
            sitepackages.push(lib_site);
        } else {
            let mut libdirs = vec![platlibdir.clone()];
            if platlibdir != "lib" {
                libdirs.push("lib".to_string());
            }
            for libdir in libdirs {
                let path = path_join(
                    interp,
                    &[
                        Value::string(&prefix),
                        Value::string(libdir),
                        Value::string(format!("python{major}.{minor}")),
                        Value::string("site-packages"),
                    ],
                )?;
                sitepackages.push(path);
            }
        }
    }
    Ok(Value::list(sitepackages))
}

fn abs_paths_impl(interp: &mut Interpreter) -> Result<()> {
    // The active registry, read without *observing* `sys.modules`: the walk
    // only rewrites `__file__` / `__cached__` of source-backed modules, so it
    // must not be what makes the deferred `zipimport` entry appear.
    let modules = interp.import_module_registry()?;
    let values_fn = interp.get_attr(&modules, "values")?;
    let values_view = interp.call_function_expanded(values_fn, &[])?;
    let mod_list = interp.collect_iterable(&values_view)?;
    for m in mod_list {
        let loader_mod = interp
            .get_attr(&m, "__loader__")
            .ok()
            .and_then(|l| interp.get_attr(&l, "__module__").ok())
            .or_else(|| {
                interp
                    .get_attr(&m, "__spec__")
                    .ok()
                    .and_then(|s| interp.get_attr(&s, "loader").ok())
                    .and_then(|l| interp.get_attr(&l, "__module__").ok())
            })
            .and_then(|v| v.as_str().map(str::to_owned));
        if !matches!(
            loader_mod.as_deref(),
            Some("_frozen_importlib" | "_frozen_importlib_external")
        ) {
            continue;
        }
        for attr in ["__file__", "__cached__"] {
            if let Ok(val) = interp.get_attr(&m, attr)
                && val.as_str().is_some()
                && let Ok(abs) = path_ops::abspath(interp, &val)
            {
                let _ = interp.assign_attr(m.clone(), attr, abs);
            }
        }
    }
    Ok(())
}

fn removeduppaths_impl(interp: &mut Interpreter) -> Result<Value> {
    let sys_path = sys_path_value(interp)?;
    let entries = interp.collect_iterable(&sys_path)?;
    let mut deduped: Vec<Value> = Vec::with_capacity(entries.len());
    let mut known = PySet::default();
    for entry in entries {
        let dir_s = render_instance_str(interp, &entry)?;
        let (dir, dircase) = makepath_strings(interp, &[dir_s])?;
        let key = PyKey::str_from(&dircase);
        if !known.contains(&key) {
            deduped.push(Value::string(dir));
            known.insert(key);
        }
    }
    sys_path.list_with_mut(|list| {
        *list = deduped;
    });
    Ok(Value::set(known))
}

fn setquit_impl(interp: &mut Interpreter) -> Result<()> {
    let eof = if cfg!(windows) {
        "Ctrl-Z plus Return"
    } else {
        "Ctrl-D (i.e. EOF)"
    };
    let sb = interp.load_module("_sitebuiltins")?;
    let quitter_cls = interp.get_attr(&sb, "Quitter")?;
    let quit_obj = interp.call_function_expanded(
        quitter_cls.clone(),
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string("quit"),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(eof),
            },
        ],
    )?;
    let exit_obj = interp.call_function_expanded(
        quitter_cls,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string("exit"),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(eof),
            },
        ],
    )?;
    let builtins = interp.load_module("builtins")?;
    interp.assign_attr(builtins.clone(), "quit", quit_obj)?;
    interp.assign_attr(builtins, "exit", exit_obj)?;
    Ok(())
}

fn setcopyright_impl(interp: &mut Interpreter) -> Result<()> {
    let sb = interp.load_module("_sitebuiltins")?;
    let printer_cls = interp.get_attr(&sb, "_Printer")?;
    let sys_mod = interp.load_module("sys")?;
    let copyright_text = interp
        .get_attr(&sys_mod, "copyright")
        .unwrap_or_else(|_| Value::string(""));
    let copyright_obj = interp.call_function_expanded(
        printer_cls.clone(),
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string("copyright"),
            },
            ExpandedCallArg {
                name: None,
                value: copyright_text,
            },
        ],
    )?;
    let credits_obj = interp.call_function_expanded(
        printer_cls.clone(),
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string("credits"),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(CREDITS_TEXT),
            },
        ],
    )?;
    let license_obj = interp.call_function_expanded(
        printer_cls,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string("license"),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string("See https://www.python.org/psf/license/"),
            },
            ExpandedCallArg {
                name: None,
                value: Value::list(Vec::new()),
            },
            ExpandedCallArg {
                name: None,
                value: Value::list(Vec::new()),
            },
        ],
    )?;
    let builtins = interp.load_module("builtins")?;
    interp.assign_attr(builtins.clone(), "copyright", copyright_obj)?;
    interp.assign_attr(builtins.clone(), "credits", credits_obj)?;
    interp.assign_attr(builtins, "license", license_obj)?;
    Ok(())
}

fn sethelper_impl(interp: &mut Interpreter) -> Result<()> {
    let sb = interp.load_module("_sitebuiltins")?;
    let helper_cls = interp.get_attr(&sb, "_Helper")?;
    let helper_obj = interp.call_function_expanded(helper_cls, &[])?;
    let builtins = interp.load_module("builtins")?;
    interp.assign_attr(builtins, "help", helper_obj)?;
    Ok(())
}

fn exec_customize_module(interp: &mut Interpreter, mod_name: &str) {
    match interp.load_module(mod_name) {
        Ok(_) => {}
        Err(ref err) if interp.is_missing_module_error(err, mod_name) => {}
        Err(err) => {
            let line = interp.format_error_line(&err);
            write_stderr_text(
                interp,
                &format!("Error in {mod_name}; set PYTHONVERBOSE for traceback:\n{line}\n"),
            );
        }
    }
}

fn addusersitepackages_impl(interp: &mut Interpreter, known_paths: Value) -> Result<Value> {
    trace_site(interp, "Processing user site-packages");
    let user_site = getusersitepackages_impl(interp)?;
    let enabled = get_site_attr(interp, "ENABLE_USER_SITE")
        .map(|v| interp.truthy_value(&v).unwrap_or(false))
        .unwrap_or(false);
    if enabled
        && let Some(dir) = user_site.as_str()
        && Path::new(dir).is_dir()
    {
        addsitedir_impl(interp, dir, known_paths.clone())?;
    }
    Ok(known_paths)
}

fn addsitepackages_impl(
    interp: &mut Interpreter,
    known_paths: Value,
    prefixes: Value,
) -> Result<Value> {
    trace_site(interp, "Processing global site-packages");
    let dirs = getsitepackages_impl(interp, prefixes)?;
    let items = interp.collect_iterable(&dirs)?;
    for item in items {
        if let Some(sitedir) = item.as_str()
            && Path::new(sitedir).is_dir()
        {
            addsitedir_impl(interp, sitedir, known_paths.clone())?;
        }
    }
    Ok(known_paths)
}

fn venv_impl(interp: &mut Interpreter, known_paths: Value) -> Result<Value> {
    let sys_mod = interp.load_module("sys")?;
    #[allow(unused_mut)]
    let mut executable = interp
        .get_attr(&sys_mod, "executable")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();

    #[cfg(target_os = "macos")]
    if let Ok(launcher) = std::env::var("__PYVENV_LAUNCHER__")
        && !launcher.is_empty()
    {
        executable = launcher.clone();
        let _ = interp.assign_attr(sys_mod.clone(), "_base_executable", Value::string(launcher));
    }

    if executable.is_empty() {
        return Ok(known_paths);
    }

    let abs_exe_val = path_ops::abspath(interp, &Value::string(&executable))?;
    let exe_dir_val = path_ops::dirname(interp, &abs_exe_val)?;
    let site_prefix_val = path_ops::dirname(interp, &exe_dir_val)?;
    let exe_dir = render_instance_str(interp, &exe_dir_val)?;
    let site_prefix = render_instance_str(interp, &site_prefix_val)?;

    let _ = interp.assign_attr(sys_mod.clone(), "_home", Value::none());

    let same_dir_cfg = Path::new(&exe_dir).join("pyvenv.cfg");
    let parent_dir_cfg = Path::new(&site_prefix).join("pyvenv.cfg");
    let found = if same_dir_cfg.is_file() {
        Some((exe_dir, same_dir_cfg))
    } else if parent_dir_cfg.is_file() {
        Some((site_prefix, parent_dir_cfg))
    } else {
        None
    };

    if let Some((venv_dir, cfg_path)) = found {
        let mut system_site = true;
        if let Ok(raw_bytes) = std::fs::read(&cfg_path) {
            let content = String::from_utf8_lossy(&raw_bytes);
            let kv = super::sys::parse_pyvenv_cfg(&content);
            if let Some(val) = kv.get("include-system-site-packages") {
                system_site = val.eq_ignore_ascii_case("true");
            }
            if let Some(home) = kv.get("home") {
                let _ = interp.assign_attr(sys_mod.clone(), "_home", Value::string(home));
            }
        }

        let venv_prefix_val = Value::string(&venv_dir);
        let _ = interp.assign_attr(sys_mod.clone(), "prefix", venv_prefix_val.clone());
        let _ = interp.assign_attr(sys_mod, "exec_prefix", venv_prefix_val.clone());

        // Add the venv's own site-packages before user-site and global site-packages.
        addsitepackages_impl(
            interp,
            known_paths.clone(),
            Value::list(vec![venv_prefix_val.clone()]),
        )?;

        if system_site {
            if let Some(prefixes) = get_site_attr(interp, "PREFIXES") {
                prefixes.list_with_mut(|items| {
                    items.insert(0, venv_prefix_val);
                });
            } else {
                set_site_attr(interp, "PREFIXES", Value::list(vec![venv_prefix_val]));
            }
        } else {
            set_site_attr(interp, "PREFIXES", Value::list(vec![venv_prefix_val]));
            set_site_attr(interp, "ENABLE_USER_SITE", Value::bool_(false));
        }
    }

    Ok(known_paths)
}

fn main_impl(interp: &mut Interpreter) -> Result<()> {
    let sys_path = sys_path_value(interp)?;
    let orig_path = interp.collect_iterable(&sys_path)?;
    let known_paths = removeduppaths_impl(interp)?;
    let new_path = interp.collect_iterable(&sys_path)?;
    if orig_path.len() != new_path.len()
        || orig_path
            .iter()
            .zip(new_path.iter())
            .any(|(a, b)| a.as_str() != b.as_str())
    {
        abs_paths_impl(interp)?;
    }

    let known_paths = venv_impl(interp, known_paths)?;

    let current_enable = get_site_attr(interp, "ENABLE_USER_SITE");
    if current_enable.is_none() || current_enable.as_ref().is_some_and(Value::is_none) {
        let enabled = check_enableusersite_impl(interp)?;
        set_site_attr(interp, "ENABLE_USER_SITE", enabled);
    }

    let known_paths = addusersitepackages_impl(interp, known_paths)?;
    let _known_paths = addsitepackages_impl(interp, known_paths, Value::none())?;
    setquit_impl(interp)?;
    setcopyright_impl(interp)?;
    sethelper_impl(interp)?;
    exec_customize_module(interp, "sitecustomize");
    let enabled = get_site_attr(interp, "ENABLE_USER_SITE")
        .map(|v| interp.truthy_value(&v).unwrap_or(false))
        .unwrap_or(false);
    if enabled {
        exec_customize_module(interp, "usercustomize");
    }
    Ok(())
}

/// Native post-load hook for `site` (replaces module-body top-level statements
/// in CPython's `Lib/site.py`, without any `@inject` or `site_py.py`).
pub(crate) fn post_load_init(interp: &mut Interpreter, module: &Value) -> Result<()> {
    let sys_mod = interp.load_module("sys")?;
    let no_site = sys_flag(interp, "no_site")?;

    let prefix = interp
        .get_attr(&sys_mod, "prefix")
        .unwrap_or_else(|_| Value::string(""));
    let exec_prefix = interp
        .get_attr(&sys_mod, "exec_prefix")
        .unwrap_or_else(|_| Value::string(""));
    let base_prefix = interp
        .get_attr(&sys_mod, "base_prefix")
        .unwrap_or_else(|_| prefix.clone());
    let base_exec_prefix = interp
        .get_attr(&sys_mod, "base_exec_prefix")
        .unwrap_or_else(|_| exec_prefix.clone());

    // When startup already detected `pyvenv.cfg`, seed `PREFIXES` with the base
    // prefixes before `main_impl()` -> `venv_impl()` runs, so that:
    // - `include-system-site-packages = true` prepends `sys.prefix` and keeps
    //   base prefixes available;
    // - `include-system-site-packages = false` replaces `PREFIXES` with `[sys.prefix]`.
    let initial_prefixes = if no_site == 0 && prefix.as_str() != base_prefix.as_str() {
        vec![base_prefix, base_exec_prefix]
    } else {
        vec![prefix, exec_prefix]
    };

    if let ValueKind::PyModule(m) = module.kind() {
        let mut borrowed = m.borrow_mut();
        borrowed.insert_attr("PREFIXES".to_string(), Value::list(initial_prefixes));
        borrowed.insert_attr("ENABLE_USER_SITE".to_string(), Value::none());
        borrowed.insert_attr("USER_SITE".to_string(), Value::none());
        borrowed.insert_attr("USER_BASE".to_string(), Value::none());
    }

    // CPython's `site.py` starts with `import os`, so `os` (and with it
    // `os.path` and `abc`) is in `sys.modules` once `site` has been imported.
    // The path helpers above no longer need the module object, so keep that
    // observable contract with one explicit import.
    interp.load_module("os")?;

    // Mirror CPython `site.py`:
    //   if not sys.flags.no_site:
    //       main()
    if no_site == 0 {
        main_impl(interp)?;
    }
    Ok(())
}

pyrust_module! {
    constants {
        "PREFIXES"         => Value::list(Vec::new()),
        "ENABLE_USER_SITE" => Value::none(),
        "USER_SITE"        => Value::none(),
        "USER_BASE"        => Value::none(),
    }

    fn makepath(#[varargs] paths: &[Value]) -> Result<Value> {
        if paths.is_empty() {
            return Err(PyError::named(
                "TypeError",
                "join() missing 1 required positional argument: 'path'",
            ));
        }
        let mut parts = Vec::with_capacity(paths.len());
        for p in paths {
            parts.push(render_instance_str(_interp, p)?);
        }
        let (dir, dircase) = makepath_strings(_interp, &parts)?;
        Ok(Value::tuple(vec![Value::string(dir), Value::string(dircase)]))
    }

    fn abs_paths() -> Result<Value> {
        abs_paths_impl(_interp)?;
        Ok(Value::none())
    }

    fn removeduppaths() -> Result<Value> {
        removeduppaths_impl(_interp)
    }

    fn _init_pathinfo() -> Result<Value> {
        init_pathinfo_impl(_interp)
    }

    fn addpackage(
        sitedir: PyPath,
        name: PyPath,
        known_paths: PyValue,
    ) -> Result<Value> {
        let sitedir_s = sitedir.as_path().to_string_lossy().into_owned();
        let name_s = name.as_path().to_string_lossy().into_owned();
        addpackage_impl(_interp, &sitedir_s, &name_s, known_paths.0)
    }

    fn addsitedir(
        sitedir: PyPath,
        #[default(PyValue(Value::none()))] known_paths: PyValue,
    ) -> Result<Value> {
        let sitedir_s = sitedir.as_path().to_string_lossy().into_owned();
        addsitedir_impl(_interp, &sitedir_s, known_paths.0)
    }

    fn check_enableusersite() -> Result<Value> {
        check_enableusersite_impl(_interp)
    }

    fn getuserbase() -> Result<Value> {
        getuserbase_impl(_interp)
    }

    fn getusersitepackages() -> Result<Value> {
        getusersitepackages_impl(_interp)
    }

    fn getsitepackages(
        #[default(PyValue(Value::none()))] prefixes: PyValue,
    ) -> Result<Value> {
        getsitepackages_impl(_interp, prefixes.0)
    }

    fn addusersitepackages(known_paths: PyValue) -> Result<Value> {
        addusersitepackages_impl(_interp, known_paths.0)
    }

    fn addsitepackages(
        known_paths: PyValue,
        #[default(PyValue(Value::none()))] prefixes: PyValue,
    ) -> Result<Value> {
        addsitepackages_impl(_interp, known_paths.0, prefixes.0)
    }

    fn setquit() -> Result<Value> {
        setquit_impl(_interp)?;
        Ok(Value::none())
    }

    fn setcopyright() -> Result<Value> {
        setcopyright_impl(_interp)?;
        Ok(Value::none())
    }

    fn sethelper() -> Result<Value> {
        sethelper_impl(_interp)?;
        Ok(Value::none())
    }

    fn execsitecustomize() -> Result<Value> {
        exec_customize_module(_interp, "sitecustomize");
        Ok(Value::none())
    }

    fn execusercustomize() -> Result<Value> {
        exec_customize_module(_interp, "usercustomize");
        Ok(Value::none())
    }

    fn venv(known_paths: PyValue) -> Result<Value> {
        venv_impl(_interp, known_paths.0)
    }

    fn main() -> Result<Value> {
        main_impl(_interp)?;
        Ok(Value::none())
    }

    fn _script() -> Result<Value> {
        let sys_mod = _interp.load_module("sys")?;
        let argv = _interp.get_attr(&sys_mod, "argv")?;
        let argv_items = _interp.collect_iterable(&argv)?;
        let args: Vec<String> = argv_items
            .iter()
            .skip(1)
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        let builtins = _interp.load_module("builtins")?;
        let print_fn = _interp.get_attr(&builtins, "print")?;
        let print_line = |interp: &mut Interpreter, line: &str| -> Result<()> {
            interp.call_function_expanded(
                print_fn.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: Value::string(line),
                }],
            )?;
            Ok(())
        };

        if args.is_empty() {
            let user_base = getuserbase_impl(_interp)?;
            let user_site = getusersitepackages_impl(_interp)?;
            let sys_path = sys_path_value(_interp)?;
            let entries = _interp.collect_iterable(&sys_path)?;
            print_line(_interp, "sys.path = [")?;
            for entry in entries {
                let r = render_value_repr(_interp, &entry)?;
                print_line(_interp, &format!("    {r},"))?;
            }
            print_line(_interp, "]")?;
            let exists_label = |v: &Value| -> &'static str {
                match v.as_str() {
                    Some(p) if Path::new(p).is_dir() => "exists",
                    _ => "doesn't exist",
                }
            };
            let ub_repr = render_value_repr(_interp, &user_base)?;
            let us_repr = render_value_repr(_interp, &user_site)?;
            let enabled = get_site_attr(_interp, "ENABLE_USER_SITE").unwrap_or_else(Value::none);
            let en_repr = render_value_repr(_interp, &enabled)?;
            print_line(
                _interp,
                &format!("USER_BASE: {ub_repr} ({})", exists_label(&user_base)),
            )?;
            print_line(
                _interp,
                &format!("USER_SITE: {us_repr} ({})", exists_label(&user_site)),
            )?;
            print_line(_interp, &format!("ENABLE_USER_SITE: {en_repr}"))?;
            return Ok(Value::none());
        }

        let mut buffer = Vec::new();
        if args.iter().any(|a| a == "--user-base") {
            let ub = getuserbase_impl(_interp)?;
            if let Some(s) = ub.as_str() {
                buffer.push(s.to_string());
            }
        }
        if args.iter().any(|a| a == "--user-site") {
            let us = getusersitepackages_impl(_interp)?;
            if let Some(s) = us.as_str() {
                buffer.push(s.to_string());
            }
        }
        if !buffer.is_empty() {
            let sep = if cfg!(windows) { ";" } else { ":" };
            print_line(_interp, &buffer.join(sep))?;
            let enabled = get_site_attr(_interp, "ENABLE_USER_SITE").unwrap_or_else(Value::none);
            let code = match enabled.kind() {
                ValueKind::Bool(true) => 0,
                ValueKind::Bool(false) => 1,
                ValueKind::None => 2,
                _ => 3,
            };
            let exit_fn = _interp.get_attr(&sys_mod, "exit")?;
            return _interp.call_function_expanded(
                exit_fn,
                &[ExpandedCallArg {
                    name: None,
                    value: Value::int(code),
                }],
            );
        }
        Ok(Value::none())
    }
}
