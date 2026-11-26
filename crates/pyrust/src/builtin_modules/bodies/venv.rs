// `venv` package module — 100% native Rust implementation of Python 3.12's
// PEP 405 virtual environment creation package (issue #3267).
//
// Reference: <https://docs.python.org/3.12/library/venv.html>

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyPath, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_instance_str, render_value_repr};
use crate::value::{PyDict, PyKey, PyModule, Value};

#[path = "venv_scripts.rs"]
mod scripts;

// ── helpers ──────────────────────────────────────────────────────────────────

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded = Vec::with_capacity(pos.len() + kw.len());
    for v in pos {
        expanded.push(ExpandedCallArg {
            name: None,
            value: v.clone(),
        });
    }
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_pos(interp, m, args)
}

fn call_method_kw(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_kw(interp, m, pos, kw)
}

fn os_path_call(interp: &mut Interpreter, func_name: &str, args: &[Value]) -> Result<Value> {
    let os_path = interp.load_module("os.path")?;
    let f = interp.get_attr(&os_path, func_name)?;
    call_pos(interp, f, args)
}

fn attr_truthy(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    let v = interp.get_attr(obj, name)?;
    interp.truthy_value(&v)
}

fn attr_str(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<String> {
    let v = interp.get_attr(obj, name)?;
    render_instance_str(interp, &v)
}

fn io_err_to_py(err: std::io::Error) -> PyError {
    match err.kind() {
        std::io::ErrorKind::NotFound => PyError::named("FileNotFoundError", err.to_string()),
        std::io::ErrorKind::AlreadyExists => PyError::named("FileExistsError", err.to_string()),
        std::io::ErrorKind::PermissionDenied => PyError::named("PermissionError", err.to_string()),
        _ => PyError::named("OSError", err.to_string()),
    }
}

fn is_symlink_path(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn create_dir_if_needed(d: &str) -> Result<()> {
    let p = Path::new(d);
    if !p.exists() && !is_symlink_path(p) {
        fs::create_dir_all(p).map_err(io_err_to_py)?;
    } else if is_symlink_path(p) || p.is_file() {
        return Err(PyError::named(
            "ValueError",
            format!("Unable to create directory {d:?}"),
        ));
    }
    Ok(())
}

fn clear_dir_fs(path: &Path) -> Result<()> {
    let entries = fs::read_dir(path).map_err(io_err_to_py)?;
    for entry in entries {
        let entry = entry.map_err(io_err_to_py)?;
        let p = entry.path();
        let meta = fs::symlink_metadata(&p).map_err(io_err_to_py)?;
        if meta.file_type().is_symlink() {
            if fs::remove_file(&p).is_err() {
                fs::remove_dir(&p).map_err(io_err_to_py)?;
            }
        } else if meta.is_file() {
            fs::remove_file(&p).map_err(io_err_to_py)?;
        } else if meta.is_dir() {
            fs::remove_dir_all(&p).map_err(io_err_to_py)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable_mode(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn set_executable_mode(_path: &Path) {}

fn symlink_or_copy_fs(
    src: &Path,
    dst: &Path,
    use_symlinks: bool,
    relative_symlinks_ok: bool,
) -> Result<()> {
    // Avoid copying/linking a file onto itself.
    if src == dst {
        return Ok(());
    }

    let mut force_copy = !use_symlinks;
    if !force_copy {
        if is_symlink_path(dst) {
            // Already a symlink; replace cleanly if target differs.
            let _ = fs::remove_file(dst);
        } else if dst.exists() {
            let _ = fs::remove_file(dst);
        }

        let target: PathBuf = if relative_symlinks_ok {
            src.file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| src.to_path_buf())
        } else {
            src.to_path_buf()
        };

        #[cfg(unix)]
        let link_res = std::os::unix::fs::symlink(&target, dst);
        #[cfg(windows)]
        let link_res = std::os::windows::fs::symlink_file(&target, dst);
        #[cfg(not(any(unix, windows)))]
        let link_res: std::io::Result<()> = Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks unsupported",
        ));

        if link_res.is_err() {
            force_copy = true;
        }
    }

    if force_copy {
        if is_symlink_path(dst) {
            let _ = fs::remove_file(dst);
        }
        fs::copy(src, dst).map_err(io_err_to_py)?;
    }
    Ok(())
}

fn walk_install_scripts(
    interp: &mut Interpreter,
    self_val: &Value,
    context: &Value,
    base_dir: &Path,
    current_dir: &Path,
    rel_subdir: &Path,
    bin_path: &Path,
) -> Result<()> {
    let Ok(entries) = fs::read_dir(current_dir) else {
        return Ok(());
    };
    let mut items: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    items.sort();

    for item in items {
        let Some(fname) = item.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if item.is_dir() {
            if current_dir == base_dir {
                let os_name = if cfg!(windows) { "nt" } else { "posix" };
                if fname != "common" && fname != os_name {
                    continue;
                }
                walk_install_scripts(
                    interp,
                    self_val,
                    context,
                    base_dir,
                    &item,
                    Path::new(""),
                    bin_path,
                )?;
            } else {
                let next_rel = rel_subdir.join(fname);
                walk_install_scripts(
                    interp, self_val, context, base_dir, &item, &next_rel, bin_path,
                )?;
            }
            continue;
        }

        // Ignore files directly at the top-level `scripts/` directory.
        if current_dir == base_dir {
            continue;
        }

        if cfg!(windows)
            && fname.starts_with("python")
            && (fname.ends_with(".exe") || fname.ends_with(".pdb"))
        {
            continue;
        }

        let dst_dir = if rel_subdir.as_os_str().is_empty() {
            bin_path.to_path_buf()
        } else {
            bin_path.join(rel_subdir)
        };
        if !dst_dir.exists() {
            fs::create_dir_all(&dst_dir).map_err(io_err_to_py)?;
        }
        let dst_file = dst_dir.join(fname);
        let raw_bytes = fs::read(&item).map_err(io_err_to_py)?;
        let out_bytes = if fname.ends_with(".exe") || fname.ends_with(".pdb") {
            Some(raw_bytes)
        } else {
            match String::from_utf8(raw_bytes) {
                Ok(text) => {
                    let replaced = call_method(
                        interp,
                        self_val,
                        "replace_variables",
                        &[Value::string(text), context.clone()],
                    )?;
                    let replaced_str = render_instance_str(interp, &replaced)?;
                    Some(replaced_str.into_bytes())
                }
                Err(_) => None,
            }
        };

        if let Some(bytes) = out_bytes {
            fs::write(&dst_file, bytes).map_err(io_err_to_py)?;
            if let Ok(meta) = fs::metadata(&item) {
                let _ = fs::set_permissions(&dst_file, meta.permissions());
            }
        }
    }
    Ok(())
}

/// Post-load initialization for `venv`: attaches `logger = logging.getLogger("venv")`
/// matching CPython 3.12's `venv/__init__.py`.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let logging_mod = interp.load_module("logging")?;
    let get_logger = interp.get_attr(&logging_mod, "getLogger")?;
    let logger = call_pos(interp, get_logger, &[Value::string("venv")])?;
    module
        .borrow_mut()
        .insert_attr("logger".to_string(), logger);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__"    => Value::string("venv"),
        "__path__"       => Value::list(Vec::new()),
        "CORE_VENV_DEPS" => Value::tuple(vec![Value::string("pip")]),
    }

    class EnvBuilder {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(PyValue(Value::bool_(false)))] system_site_packages: PyValue,
            #[default(PyValue(Value::bool_(false)))] clear: PyValue,
            #[default(PyValue(Value::bool_(false)))] symlinks: PyValue,
            #[default(PyValue(Value::bool_(false)))] upgrade: PyValue,
            #[default(PyValue(Value::bool_(false)))] with_pip: PyValue,
            #[default(PyValue(Value::none()))] prompt: PyValue,
            #[default(PyValue(Value::bool_(false)))] upgrade_deps: PyValue,
        ) -> Result<Value> {
            let s = self_val.0.clone();
            _interp.assign_attr(s.clone(), "system_site_packages", system_site_packages.0)?;
            _interp.assign_attr(s.clone(), "clear", clear.0)?;
            _interp.assign_attr(s.clone(), "symlinks", symlinks.0)?;
            _interp.assign_attr(s.clone(), "upgrade", upgrade.0)?;
            _interp.assign_attr(s.clone(), "with_pip", with_pip.0)?;
            _interp.assign_attr(s.clone(), "orig_prompt", prompt.0.clone())?;
            let resolved_prompt = if prompt.0.as_str() == Some(".") {
                let cwd = std::env::current_dir().map_err(io_err_to_py)?;
                let base = cwd
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Value::string(base)
            } else {
                prompt.0
            };
            _interp.assign_attr(s.clone(), "prompt", resolved_prompt)?;
            _interp.assign_attr(s, "upgrade_deps", upgrade_deps.0)?;
            Ok(Value::none())
        }

        fn create(#[positional_only] self_val: PyValueRef<'_>, env_dir: PyPath) -> Result<Value> {
            let s = self_val.0;
            let env_dir_val = Value::string(env_dir.as_path().to_string_lossy());
            let abs_env_dir = os_path_call(_interp, "abspath", &[env_dir_val])?;
            let context = call_method(_interp, s, "ensure_directories", &[abs_env_dir])?;

            // Match CPython issue #24875: keep `system_site_packages` False until
            // after pip is installed, then restore and rewrite `pyvenv.cfg`.
            let true_system_site_packages = attr_truthy(_interp, s, "system_site_packages")?;
            _interp.assign_attr(s.clone(), "system_site_packages", Value::bool_(false))?;
            call_method(_interp, s, "create_configuration", std::slice::from_ref(&context))?;
            call_method(_interp, s, "setup_python", std::slice::from_ref(&context))?;

            if attr_truthy(_interp, s, "with_pip")? {
                call_method(_interp, s, "_setup_pip", std::slice::from_ref(&context))?;
            }
            if !attr_truthy(_interp, s, "upgrade")? {
                call_method(_interp, s, "setup_scripts", std::slice::from_ref(&context))?;
                call_method(_interp, s, "post_setup", std::slice::from_ref(&context))?;
            }
            if true_system_site_packages {
                _interp.assign_attr(s.clone(), "system_site_packages", Value::bool_(true))?;
                call_method(_interp, s, "create_configuration", std::slice::from_ref(&context))?;
            }
            if attr_truthy(_interp, s, "upgrade_deps")? {
                call_method(_interp, s, "upgrade_dependencies", &[context])?;
            }
            Ok(Value::none())
        }

        fn clear_directory(#[positional_only] _self: PyValueRef<'_>, path: PyPath) -> Result<Value> {
            clear_dir_fs(path.as_path())?;
            Ok(Value::none())
        }

        fn _venv_path(
            #[positional_only] _self: PyValueRef<'_>,
            env_dir: PyPath,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let env_dir_str = env_dir.as_path().to_string_lossy().into_owned();
            let mut vars = PyDict::default();
            for key in ["base", "platbase", "installed_base", "installed_platbase"] {
                vars.insert(PyKey::str_from(key), Value::string(&env_dir_str));
            }
            let sysconfig_mod = _interp.load_module("sysconfig")?;
            let get_path = _interp.get_attr(&sysconfig_mod, "get_path")?;
            call_kw(
                _interp,
                get_path,
                &[Value::string(&*name)],
                &[
                    ("scheme", Value::string("venv")),
                    ("vars", Value::dict(vars)),
                ],
            )
        }

        fn ensure_directories(
            #[positional_only] self_val: PyValueRef<'_>,
            env_dir: PyPath,
        ) -> Result<Value> {
            let s = self_val.0;
            let env_dir_str = env_dir.as_path().to_string_lossy().into_owned();
            let pathsep = if cfg!(windows) { ';' } else { ':' };
            if env_dir_str.contains(pathsep) {
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "Refusing to create a venv in {env_dir_str} because it contains the PATH separator {pathsep}."
                    ),
                ));
            }
            if Path::new(&env_dir_str).exists() && attr_truthy(_interp, s, "clear")? {
                call_method(_interp, s, "clear_directory", &[Value::string(&env_dir_str)])?;
            }

            let types_mod = _interp.load_module("types")?;
            let ns_cls = _interp.get_attr(&types_mod, "SimpleNamespace")?;
            let context = call_pos(_interp, ns_cls, &[])?;

            _interp.assign_attr(context.clone(), "env_dir", Value::string(&env_dir_str))?;
            let split_res = os_path_call(_interp, "split", &[Value::string(&env_dir_str)])?;
            let split_items = _interp.collect_iterable(&split_res)?;
            let env_name = split_items
                .get(1)
                .map(|v| render_instance_str(_interp, v))
                .transpose()?
                .unwrap_or_default();
            _interp.assign_attr(context.clone(), "env_name", Value::string(&env_name))?;

            let self_prompt = _interp.get_attr(s, "prompt")?;
            let prompt_inner = if self_prompt.is_none() {
                env_name.clone()
            } else {
                render_instance_str(_interp, &self_prompt)?
            };
            _interp.assign_attr(
                context.clone(),
                "prompt",
                Value::string(format!("({prompt_inner}) ")),
            )?;

            create_dir_if_needed(&env_dir_str)?;

            let sys_mod = _interp.load_module("sys")?;
            let base_exe = _interp
                .get_attr(&sys_mod, "_base_executable")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .filter(|s| !s.is_empty());
            let executable = match base_exe {
                Some(p) => p,
                None => _interp
                    .get_attr(&sys_mod, "executable")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default(),
            };
            if executable.is_empty() {
                return Err(PyError::named(
                    "ValueError",
                    "Unable to determine path to the running Python interpreter. Provide an explicit path or check that your PATH environment variable is correctly set.",
                ));
            }

            let abs_exe = os_path_call(_interp, "abspath", &[Value::string(&executable)])?;
            let exe_split = os_path_call(_interp, "split", &[abs_exe])?;
            let exe_parts = _interp.collect_iterable(&exe_split)?;
            let python_dir = exe_parts
                .first()
                .map(|v| render_instance_str(_interp, v))
                .transpose()?
                .unwrap_or_default();
            let python_exe = exe_parts
                .get(1)
                .map(|v| render_instance_str(_interp, v))
                .transpose()?
                .unwrap_or_default();

            _interp.assign_attr(context.clone(), "executable", Value::string(&executable))?;
            _interp.assign_attr(context.clone(), "python_dir", Value::string(&python_dir))?;
            _interp.assign_attr(context.clone(), "python_exe", Value::string(&python_exe))?;

            let binpath_val = call_method(
                _interp,
                s,
                "_venv_path",
                &[Value::string(&env_dir_str), Value::string("scripts")],
            )?;
            let incpath_val = call_method(
                _interp,
                s,
                "_venv_path",
                &[Value::string(&env_dir_str), Value::string("include")],
            )?;
            let libpath_val = call_method(
                _interp,
                s,
                "_venv_path",
                &[Value::string(&env_dir_str), Value::string("purelib")],
            )?;
            let binpath = render_instance_str(_interp, &binpath_val)?;
            let incpath = render_instance_str(_interp, &incpath_val)?;
            let libpath = render_instance_str(_interp, &libpath_val)?;

            _interp.assign_attr(context.clone(), "inc_path", Value::string(&incpath))?;
            create_dir_if_needed(&incpath)?;
            _interp.assign_attr(context.clone(), "lib_path", Value::string(&libpath))?;
            create_dir_if_needed(&libpath)?;

            // Issue 21197: create `lib64` symlink to `lib` on 64-bit non-macOS POSIX.
            #[cfg(all(unix, not(target_os = "macos"), target_pointer_width = "64"))]
            {
                let link_path = Path::new(&env_dir_str).join("lib64");
                if !link_path.exists() && !is_symlink_path(&link_path) {
                    let _ = std::os::unix::fs::symlink("lib", &link_path);
                }
            }

            let bin_name_val = os_path_call(
                _interp,
                "relpath",
                &[Value::string(&binpath), Value::string(&env_dir_str)],
            )?;
            let bin_name = render_instance_str(_interp, &bin_name_val)?;
            let env_exe_val = os_path_call(
                _interp,
                "join",
                &[Value::string(&binpath), Value::string(&python_exe)],
            )?;
            let env_exe = render_instance_str(_interp, &env_exe_val)?;

            _interp.assign_attr(context.clone(), "bin_path", Value::string(&binpath))?;
            _interp.assign_attr(context.clone(), "bin_name", Value::string(&bin_name))?;
            _interp.assign_attr(context.clone(), "env_exe", Value::string(&env_exe))?;
            create_dir_if_needed(&binpath)?;
            _interp.assign_attr(context.clone(), "env_exec_cmd", Value::string(&env_exe))?;

            Ok(context)
        }

        fn create_configuration(
            #[positional_only] self_val: PyValueRef<'_>,
            context: PyValue,
        ) -> Result<Value> {
            let s = self_val.0;
            let ctx = &context.0;
            let env_dir = attr_str(_interp, ctx, "env_dir")?;
            let python_dir = attr_str(_interp, ctx, "python_dir")?;
            let cfg_path_val = os_path_call(
                _interp,
                "join",
                &[Value::string(&env_dir), Value::string("pyvenv.cfg")],
            )?;
            let cfg_path = render_instance_str(_interp, &cfg_path_val)?;
            _interp.assign_attr(ctx.clone(), "cfg_path", Value::string(&cfg_path))?;

            let sys_site = attr_truthy(_interp, s, "system_site_packages")?;
            let symlinks = attr_truthy(_interp, s, "symlinks")?;
            let with_pip = attr_truthy(_interp, s, "with_pip")?;
            let clear = attr_truthy(_interp, s, "clear")?;
            let upgrade = attr_truthy(_interp, s, "upgrade")?;
            let upgrade_deps = attr_truthy(_interp, s, "upgrade_deps")?;
            let prompt_val = _interp.get_attr(s, "prompt")?;
            let orig_prompt_val = _interp.get_attr(s, "orig_prompt")?;

            let sys_mod = _interp.load_module("sys")?;
            let vi = _interp.get_attr(&sys_mod, "version_info")?;
            let major = _interp
                .get_attr(&vi, "major")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(3);
            let minor = _interp
                .get_attr(&vi, "minor")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(12);
            let micro = _interp
                .get_attr(&vi, "micro")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(0);

            let sys_exe = _interp
                .get_attr(&sys_mod, "executable")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let real_exe_val = os_path_call(_interp, "realpath", &[Value::string(&sys_exe)])?;
            let real_exe = render_instance_str(_interp, &real_exe_val)?;

            let mut out = String::new();
            out.push_str(&format!("home = {python_dir}\n"));
            out.push_str(&format!(
                "include-system-site-packages = {}\n",
                if sys_site { "true" } else { "false" }
            ));
            out.push_str(&format!("version = {major}.{minor}.{micro}\n"));
            if !prompt_val.is_none() {
                let prompt_repr = render_value_repr(_interp, &prompt_val)?;
                out.push_str(&format!("prompt = {prompt_repr}\n"));
            }
            out.push_str(&format!("executable = {real_exe}\n"));

            let mut args: Vec<String> = Vec::new();
            if cfg!(windows) && symlinks {
                args.push("--symlinks".to_string());
            }
            if !cfg!(windows) && !symlinks {
                args.push("--copies".to_string());
            }
            if !with_pip {
                args.push("--without-pip".to_string());
            }
            if sys_site {
                args.push("--system-site-packages".to_string());
            }
            if clear {
                args.push("--clear".to_string());
            }
            if upgrade {
                args.push("--upgrade".to_string());
            }
            if upgrade_deps {
                args.push("--upgrade-deps".to_string());
            }
            if !orig_prompt_val.is_none() {
                let orig_prompt = render_instance_str(_interp, &orig_prompt_val)?;
                args.push(format!("--prompt=\"{orig_prompt}\""));
            }
            args.push(env_dir);
            out.push_str(&format!("command = {sys_exe} -m venv {}\n", args.join(" ")));

            fs::write(&cfg_path, out.as_bytes()).map_err(io_err_to_py)?;
            Ok(Value::none())
        }

        fn symlink_or_copy(
            #[positional_only] self_val: PyValueRef<'_>,
            src: PyPath,
            dst: PyPath,
            #[default(PyValue(Value::bool_(false)))] relative_symlinks_ok: PyValue,
        ) -> Result<Value> {
            let use_symlinks = attr_truthy(_interp, self_val.0, "symlinks")?;
            let rel_ok = _interp.truthy_value(&relative_symlinks_ok.0)?;
            symlink_or_copy_fs(src.as_path(), dst.as_path(), use_symlinks, rel_ok)?;
            Ok(Value::none())
        }

        fn setup_python(
            #[positional_only] self_val: PyValueRef<'_>,
            context: PyValue,
        ) -> Result<Value> {
            let s = self_val.0;
            let ctx = &context.0;
            let binpath = attr_str(_interp, ctx, "bin_path")?;
            let executable = attr_str(_interp, ctx, "executable")?;
            let env_exe = attr_str(_interp, ctx, "env_exe")?;
            let python_exe = attr_str(_interp, ctx, "python_exe")?;

            let sys_mod = _interp.load_module("sys")?;
            let vi = _interp.get_attr(&sys_mod, "version_info")?;
            let minor = _interp
                .get_attr(&vi, "minor")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(12);

            if !cfg!(windows) {
                call_method(
                    _interp,
                    s,
                    "symlink_or_copy",
                    &[Value::string(&executable), Value::string(&env_exe)],
                )?;
                let env_exe_path = Path::new(&env_exe);
                if !is_symlink_path(env_exe_path) {
                    set_executable_mode(env_exe_path);
                }
                let ver_suffix = format!("python3.{minor}");
                for suffix in ["python", "python3", ver_suffix.as_str()] {
                    let dst_val = os_path_call(
                        _interp,
                        "join",
                        &[Value::string(&binpath), Value::string(suffix)],
                    )?;
                    let dst = render_instance_str(_interp, &dst_val)?;
                    let dst_path = Path::new(&dst);
                    if !dst_path.exists() && !is_symlink_path(dst_path) {
                        call_method_kw(
                            _interp,
                            s,
                            "symlink_or_copy",
                            &[Value::string(&env_exe), Value::string(&dst)],
                            &[("relative_symlinks_ok", Value::bool_(true))],
                        )?;
                        if !is_symlink_path(dst_path) {
                            set_executable_mode(dst_path);
                        }
                    }
                }
            } else {
                call_method(
                    _interp,
                    s,
                    "symlink_or_copy",
                    &[Value::string(&executable), Value::string(&env_exe)],
                )?;
                // PyRust's binary is typically named `pyrust.exe`; also install
                // `Scripts/python.exe` so standard venv activation & tooling work.
                if !python_exe.eq_ignore_ascii_case("python.exe") {
                    let py_dst_val = os_path_call(
                        _interp,
                        "join",
                        &[Value::string(&binpath), Value::string("python.exe")],
                    )?;
                    let py_dst = render_instance_str(_interp, &py_dst_val)?;
                    call_method(
                        _interp,
                        s,
                        "symlink_or_copy",
                        &[Value::string(&executable), Value::string(&py_dst)],
                    )?;
                }
            }
            Ok(Value::none())
        }

        fn _call_new_python(
            #[positional_only] _self: PyValueRef<'_>,
            context: PyValue,
            #[varargs] py_args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let ctx = &context.0;
            let env_exec_cmd = attr_str(_interp, ctx, "env_exec_cmd")?;
            let env_dir = attr_str(_interp, ctx, "env_dir")?;

            let mut cmd_args = Vec::with_capacity(py_args.len() + 1);
            cmd_args.push(Value::string(&env_exec_cmd));
            cmd_args.extend(py_args.iter().cloned());

            let os_mod = _interp.load_module("os")?;
            let environ = _interp.get_attr(&os_mod, "environ")?;
            let items_val = call_method(_interp, &environ, "items", &[])?;
            let env_pairs = _interp.collect_iterable(&items_val)?;
            let mut env_dict = PyDict::default();
            for pair in env_pairs {
                let elems = _interp.collect_iterable(&pair)?;
                if elems.len() == 2
                    && let Some(k) = elems[0].as_str()
                    && !matches!(k, "PYTHONHOME" | "PYTHONPATH")
                {
                    env_dict.insert(PyKey::str_from(k), elems[1].clone());
                }
            }
            env_dict.insert(PyKey::str_from("VIRTUAL_ENV"), Value::string(&env_dir));
            let env_copy = Value::dict(env_dict);

            let mut call_args = vec![ExpandedCallArg {
                name: None,
                value: Value::list(cmd_args),
            }];
            for (k, v) in kwargs {
                if !matches!(k.as_str(), "env" | "cwd" | "executable") {
                    call_args.push(ExpandedCallArg {
                        name: Some(k),
                        value: v,
                    });
                }
            }
            call_args.push(ExpandedCallArg {
                name: Some("env".to_string()),
                value: env_copy,
            });
            call_args.push(ExpandedCallArg {
                name: Some("cwd".to_string()),
                value: Value::string(&env_dir),
            });
            call_args.push(ExpandedCallArg {
                name: Some("executable".to_string()),
                value: Value::string(&env_exec_cmd),
            });

            let subprocess_mod = _interp.load_module("subprocess")?;
            let check_output = _interp.get_attr(&subprocess_mod, "check_output")?;
            _interp.call_function_expanded(check_output, &call_args)?;
            Ok(Value::none())
        }

        fn _setup_pip(
            #[positional_only] self_val: PyValueRef<'_>,
            context: PyValue,
        ) -> Result<Value> {
            let subprocess_mod = _interp.load_module("subprocess")?;
            let stdout_val = _interp.get_attr(&subprocess_mod, "STDOUT")?;
            call_method_kw(
                _interp,
                self_val.0,
                "_call_new_python",
                &[
                    context.0,
                    Value::string("-m"),
                    Value::string("ensurepip"),
                    Value::string("--upgrade"),
                    Value::string("--default-pip"),
                ],
                &[("stderr", stdout_val)],
            )?;
            Ok(Value::none())
        }

        fn replace_variables(
            #[positional_only] _self: PyValueRef<'_>,
            text: PyStr<'_>,
            context: PyValue,
        ) -> Result<Value> {
            let ctx = &context.0;
            let env_dir = attr_str(_interp, ctx, "env_dir")?;
            let env_name = attr_str(_interp, ctx, "env_name")?;
            let prompt = attr_str(_interp, ctx, "prompt")?;
            let bin_name = attr_str(_interp, ctx, "bin_name")?;
            let env_exe = attr_str(_interp, ctx, "env_exe")?;

            let replaced = text
                .replace("__VENV_DIR__", &env_dir)
                .replace("__VENV_NAME__", &env_name)
                .replace("__VENV_PROMPT__", &prompt)
                .replace("__VENV_BIN_NAME__", &bin_name)
                .replace("__VENV_PYTHON__", &env_exe);
            Ok(Value::string(replaced))
        }

        fn setup_scripts(
            #[positional_only] self_val: PyValueRef<'_>,
            context: PyValue,
        ) -> Result<Value> {
            let s = self_val.0;
            let ctx = &context.0;
            let bin_path = attr_str(_interp, ctx, "bin_path")?;
            let bin_dir = Path::new(&bin_path);
            if !bin_dir.exists() {
                fs::create_dir_all(bin_dir).map_err(io_err_to_py)?;
            }

            for &(filename, template) in scripts::default_scripts() {
                let replaced = call_method(
                    _interp,
                    s,
                    "replace_variables",
                    &[Value::string(template), ctx.clone()],
                )?;
                let content = render_instance_str(_interp, &replaced)?;
                let dst = bin_dir.join(filename);
                fs::write(&dst, content.as_bytes()).map_err(io_err_to_py)?;
            }
            Ok(Value::none())
        }

        fn install_scripts(
            #[positional_only] self_val: PyValueRef<'_>,
            context: PyValue,
            path: PyPath,
        ) -> Result<Value> {
            let bin_path = attr_str(_interp, &context.0, "bin_path")?;
            let base = path.as_path();
            walk_install_scripts(
                _interp,
                self_val.0,
                &context.0,
                base,
                base,
                Path::new(""),
                Path::new(&bin_path),
            )?;
            Ok(Value::none())
        }

        fn post_setup(
            #[positional_only] _self: PyValueRef<'_>,
            _context: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn upgrade_dependencies(
            #[positional_only] self_val: PyValueRef<'_>,
            context: PyValue,
        ) -> Result<Value> {
            let venv_mod = _interp.load_module("venv")?;
            let deps_val = _interp.get_attr(&venv_mod, "CORE_VENV_DEPS")?;
            let deps = _interp.collect_iterable(&deps_val)?;
            let mut pos = vec![
                context.0,
                Value::string("-m"),
                Value::string("pip"),
                Value::string("install"),
                Value::string("--upgrade"),
            ];
            pos.extend(deps);
            call_method(_interp, self_val.0, "_call_new_python", &pos)?;
            Ok(Value::none())
        }
    }

    fn create(
        env_dir: PyPath,
        #[default(PyValue(Value::bool_(false)))] system_site_packages: PyValue,
        #[default(PyValue(Value::bool_(false)))] clear: PyValue,
        #[default(PyValue(Value::bool_(false)))] symlinks: PyValue,
        #[default(PyValue(Value::bool_(false)))] with_pip: PyValue,
        #[default(PyValue(Value::none()))] prompt: PyValue,
        #[default(PyValue(Value::bool_(false)))] upgrade_deps: PyValue,
    ) -> Result<Value> {
        let venv_mod = _interp.load_module("venv")?;
        let builder_cls = _interp.get_attr(&venv_mod, "EnvBuilder")?;
        let builder = call_kw(
            _interp,
            builder_cls,
            &[],
            &[
                ("system_site_packages", system_site_packages.0),
                ("clear", clear.0),
                ("symlinks", symlinks.0),
                ("with_pip", with_pip.0),
                ("prompt", prompt.0),
                ("upgrade_deps", upgrade_deps.0),
            ],
        )?;
        let env_dir_val = Value::string(env_dir.as_path().to_string_lossy());
        call_method(_interp, &builder, "create", &[env_dir_val])?;
        Ok(Value::none())
    }

    fn main(#[default(PyValue(Value::none()))] args: PyValue) -> Result<Value> {
        let argparse_mod = _interp.load_module("argparse")?;
        let parser_cls = _interp.get_attr(&argparse_mod, "ArgumentParser")?;
        let parser = call_kw(
            _interp,
            parser_cls,
            &[],
            &[
                ("prog", Value::string("venv")),
                (
                    "description",
                    Value::string(
                        "Creates virtual Python environments in one or more target directories.",
                    ),
                ),
                (
                    "epilog",
                    Value::string(
                        "Once an environment has been created, you may wish to activate it, e.g. by sourcing an activate script in its bin directory.",
                    ),
                ),
            ],
        )?;

        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("dirs")],
            &[
                ("metavar", Value::string("ENV_DIR")),
                ("nargs", Value::string("+")),
                (
                    "help",
                    Value::string("A directory to create the environment in."),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--system-site-packages")],
            &[
                ("default", Value::bool_(false)),
                ("action", Value::string("store_true")),
                ("dest", Value::string("system_site")),
                (
                    "help",
                    Value::string(
                        "Give the virtual environment access to the system site-packages dir.",
                    ),
                ),
            ],
        )?;

        let use_symlinks = !cfg!(windows);
        let group = call_method(_interp, &parser, "add_mutually_exclusive_group", &[])?;
        call_method_kw(
            _interp,
            &group,
            "add_argument",
            &[Value::string("--symlinks")],
            &[
                ("default", Value::bool_(use_symlinks)),
                ("action", Value::string("store_true")),
                ("dest", Value::string("symlinks")),
                (
                    "help",
                    Value::string(
                        "Try to use symlinks rather than copies, when symlinks are not the default for the platform.",
                    ),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &group,
            "add_argument",
            &[Value::string("--copies")],
            &[
                ("default", Value::bool_(!use_symlinks)),
                ("action", Value::string("store_false")),
                ("dest", Value::string("symlinks")),
                (
                    "help",
                    Value::string(
                        "Try to use copies rather than symlinks, even when symlinks are the default for the platform.",
                    ),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--clear")],
            &[
                ("default", Value::bool_(false)),
                ("action", Value::string("store_true")),
                ("dest", Value::string("clear")),
                (
                    "help",
                    Value::string(
                        "Delete the contents of the environment directory if it already exists, before environment creation.",
                    ),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--upgrade")],
            &[
                ("default", Value::bool_(false)),
                ("action", Value::string("store_true")),
                ("dest", Value::string("upgrade")),
                (
                    "help",
                    Value::string(
                        "Upgrade the environment directory to use this version of Python, assuming Python has been upgraded in-place.",
                    ),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--without-pip")],
            &[
                ("dest", Value::string("with_pip")),
                ("default", Value::bool_(true)),
                ("action", Value::string("store_false")),
                (
                    "help",
                    Value::string(
                        "Skips installing or upgrading pip in the virtual environment (pip is bootstrapped by default)",
                    ),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--prompt")],
            &[(
                "help",
                Value::string("Provides an alternative prompt prefix for this environment."),
            )],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--upgrade-deps")],
            &[
                ("default", Value::bool_(false)),
                ("action", Value::string("store_true")),
                ("dest", Value::string("upgrade_deps")),
                (
                    "help",
                    Value::string("Upgrade core dependencies (pip) to the latest version in PyPI"),
                ),
            ],
        )?;

        let options = call_method(_interp, &parser, "parse_args", &[args.0])?;
        let upgrade = attr_truthy(_interp, &options, "upgrade")?;
        let clear = attr_truthy(_interp, &options, "clear")?;
        if upgrade && clear {
            return Err(PyError::named(
                "ValueError",
                "you cannot supply --upgrade and --clear together.",
            ));
        }

        let opt_system_site = _interp.get_attr(&options, "system_site")?;
        let opt_clear = _interp.get_attr(&options, "clear")?;
        let opt_symlinks = _interp.get_attr(&options, "symlinks")?;
        let opt_upgrade = _interp.get_attr(&options, "upgrade")?;
        let opt_with_pip = _interp.get_attr(&options, "with_pip")?;
        let opt_prompt = _interp.get_attr(&options, "prompt")?;
        let opt_upgrade_deps = _interp.get_attr(&options, "upgrade_deps")?;

        let venv_mod = _interp.load_module("venv")?;
        let builder_cls = _interp.get_attr(&venv_mod, "EnvBuilder")?;
        let builder = call_kw(
            _interp,
            builder_cls,
            &[],
            &[
                ("system_site_packages", opt_system_site),
                ("clear", opt_clear),
                ("symlinks", opt_symlinks),
                ("upgrade", opt_upgrade),
                ("with_pip", opt_with_pip),
                ("prompt", opt_prompt),
                ("upgrade_deps", opt_upgrade_deps),
            ],
        )?;

        let dirs_val = _interp.get_attr(&options, "dirs")?;
        let dirs = _interp.collect_iterable(&dirs_val)?;
        for d in dirs {
            call_method(_interp, &builder, "create", &[d])?;
        }
        Ok(Value::none())
    }
}
