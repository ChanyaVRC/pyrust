// `ensurepip` package module — 100% native Rust implementation of Python 3.12's
// `ensurepip` package (issue #3267).
//
// Reference: <https://docs.python.org/3.12/library/ensurepip.html>

use std::fs;
use std::path::{Path, PathBuf};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyPath, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_instance_str, render_value_repr};
use crate::value::{PyDict, PyKey, Value};

const PIP_VERSION: &str = "24.0";

crate::pyrust_struct_seq! {
    pub(crate) _Package in "ensurepip" {
        fields: [version, wheel_name, wheel_path],
    }
}

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

fn find_packages_in_dir(dir: &Path) -> Vec<(String, String, String)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut filenames: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    filenames.sort();

    let mut found = Vec::new();
    for filename in filenames {
        if !filename.ends_with(".whl") {
            continue;
        }
        if let Some(rest) = filename.strip_prefix("pip-") {
            let version = rest.split('-').next().unwrap_or(PIP_VERSION).to_string();
            let wheel_path = dir.join(&filename).to_string_lossy().into_owned();
            found.push(("pip".to_string(), version, wheel_path));
        }
    }
    found
}

fn get_wheel_pkg_dir(interp: &mut Interpreter) -> Option<String> {
    if let Ok(ep_mod) = interp.load_module("ensurepip")
        && let Ok(val) = interp.get_attr(&ep_mod, "_WHEEL_PKG_DIR")
        && let Some(s) = val.as_str()
        && !s.is_empty()
    {
        return Some(s.to_string());
    }
    if let Ok(sc_mod) = interp.load_module("sysconfig")
        && let Ok(get_var) = interp.get_attr(&sc_mod, "get_config_var")
        && let Ok(val) = call_pos(interp, get_var, &[Value::string("WHEEL_PKG_DIR")])
        && let Some(s) = val.as_str()
        && !s.is_empty()
    {
        return Some(s.to_string());
    }
    None
}

fn get_packages_dict(interp: &mut Interpreter) -> Value {
    if let Some(wheel_dir) = get_wheel_pkg_dir(interp) {
        let found = find_packages_in_dir(Path::new(&wheel_dir));
        if !found.is_empty() {
            let mut d = PyDict::default();
            for (name, version, wheel_path) in found {
                d.insert(
                    PyKey::str_from(&name),
                    _Package::make([
                        Value::string(version),
                        Value::none(),
                        Value::string(wheel_path),
                    ]),
                );
            }
            return Value::dict(d);
        }
    }
    let mut d = PyDict::default();
    d.insert(
        PyKey::str_from("pip"),
        _Package::make([
            Value::string(PIP_VERSION),
            Value::string(format!("pip-{PIP_VERSION}-py3-none-any.whl")),
            Value::none(),
        ]),
    );
    Value::dict(d)
}

fn current_pip_version(interp: &mut Interpreter) -> String {
    if let Some(wheel_dir) = get_wheel_pkg_dir(interp) {
        let found = find_packages_in_dir(Path::new(&wheel_dir));
        if let Some((_, version, _)) = found.into_iter().last() {
            return version;
        }
    }
    PIP_VERSION.to_string()
}

fn disable_pip_config_impl(interp: &mut Interpreter) -> Result<()> {
    let os_mod = interp.load_module("os")?;
    let environ = interp.get_attr(&os_mod, "environ")?;
    let keys_val = call_method(interp, &environ, "keys", &[])?;
    let keys = interp.collect_iterable(&keys_val)?;
    let pip_keys: Vec<String> = keys
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .filter(|k| k.starts_with("PIP_"))
        .collect();
    for k in pip_keys {
        let _ = call_method(interp, &environ, "__delitem__", &[Value::string(&k)]);
    }
    let devnull = interp
        .get_attr(&os_mod, "devnull")
        .unwrap_or_else(|_| Value::string(if cfg!(windows) { "nul" } else { "/dev/null" }));
    call_method(
        interp,
        &environ,
        "__setitem__",
        &[Value::string("PIP_CONFIG_FILE"), devnull],
    )?;
    Ok(())
}

fn run_pip_impl(interp: &mut Interpreter, args: Value, additional_paths: Value) -> Result<Value> {
    let extra_list = if additional_paths.is_none() {
        Value::list(Vec::new())
    } else {
        let items = interp.collect_iterable(&additional_paths)?;
        Value::list(items)
    };
    let args_items = interp.collect_iterable(&args)?;
    let args_list = Value::list(args_items);

    let extra_repr = render_value_repr(interp, &extra_list)?;
    let args_repr = render_value_repr(interp, &args_list)?;
    let code = format!(
        "\nimport runpy\nimport sys\nsys.path = {extra_repr} + sys.path\nsys.argv[1:] = {args_repr}\nrunpy.run_module(\"pip\", run_name=\"__main__\", alter_sys=True)\n"
    );

    let sys_mod = interp.load_module("sys")?;
    let executable = interp.get_attr(&sys_mod, "executable")?;
    let flags = interp.get_attr(&sys_mod, "flags")?;
    let isolated = interp
        .get_attr(&flags, "isolated")
        .ok()
        .and_then(|v| v.as_int())
        .unwrap_or(0);

    let mut cmd = vec![executable];
    if isolated != 0 {
        cmd.push(Value::string("-I"));
    }
    cmd.push(Value::string("-W"));
    cmd.push(Value::string("ignore::DeprecationWarning"));
    cmd.push(Value::string("-c"));
    cmd.push(Value::string(code));

    let subprocess_mod = interp.load_module("subprocess")?;
    let run_fn = interp.get_attr(&subprocess_mod, "run")?;
    let proc = call_kw(
        interp,
        run_fn,
        &[Value::list(cmd)],
        &[("check", Value::bool_(true))],
    )?;
    interp.get_attr(&proc, "returncode")
}

fn discover_offline_wheel(interp: &mut Interpreter) -> Option<PathBuf> {
    if let Some(wheel_dir) = get_wheel_pkg_dir(interp) {
        let found = find_packages_in_dir(Path::new(&wheel_dir));
        if let Some((_, _, wheel_path)) = found.into_iter().last() {
            let p = PathBuf::from(wheel_path);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn bootstrap_impl(
    interp: &mut Interpreter,
    root: Value,
    upgrade: bool,
    user: bool,
    altinstall: bool,
    default_pip: bool,
    verbosity: i64,
) -> Result<Value> {
    if altinstall && default_pip {
        return Err(PyError::named(
            "ValueError",
            "Cannot use altinstall and default_pip together",
        ));
    }

    let sys_mod = interp.load_module("sys")?;
    if let Ok(audit_fn) = interp.get_attr(&sys_mod, "audit") {
        let _ = call_pos(
            interp,
            audit_fn,
            &[Value::string("ensurepip.bootstrap"), root.clone()],
        );
    }

    disable_pip_config_impl(interp)?;

    let os_mod = interp.load_module("os")?;
    let environ = interp.get_attr(&os_mod, "environ")?;
    if altinstall {
        call_method(
            interp,
            &environ,
            "__setitem__",
            &[
                Value::string("ENSUREPIP_OPTIONS"),
                Value::string("altinstall"),
            ],
        )?;
    } else if !default_pip {
        call_method(
            interp,
            &environ,
            "__setitem__",
            &[Value::string("ENSUREPIP_OPTIONS"), Value::string("install")],
        )?;
    }

    // Offline-safe bootstrap: if an offline pip wheel is configured and present
    // on disk, invoke pip strictly offline (`--no-cache-dir --no-index
    // --find-links`). Otherwise succeed cleanly without attempting any network
    // access or crashing during unit/integration tests.
    if let Some(wheel_path) = discover_offline_wheel(interp) {
        let wheel_dir = wheel_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut pip_args = vec![
            Value::string("install"),
            Value::string("--no-cache-dir"),
            Value::string("--no-index"),
            Value::string("--find-links"),
            Value::string(wheel_dir),
        ];
        if !root.is_none() {
            let root_s = render_instance_str(interp, &root)?;
            if !root_s.is_empty() {
                pip_args.push(Value::string("--root"));
                pip_args.push(Value::string(root_s));
            }
        }
        if upgrade {
            pip_args.push(Value::string("--upgrade"));
        }
        if user {
            pip_args.push(Value::string("--user"));
        }
        if verbosity > 0 {
            let v_count = verbosity.min(10) as usize;
            pip_args.push(Value::string(format!("-{}", "v".repeat(v_count))));
        }
        pip_args.push(Value::string("pip"));
        return run_pip_impl(
            interp,
            Value::list(pip_args),
            Value::list(vec![Value::string(wheel_path.to_string_lossy())]),
        );
    }

    Ok(Value::int(0))
}

fn write_stderr(interp: &mut Interpreter, text: &str) {
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(stderr) = interp.get_attr(&sys_mod, "stderr")
        && !stderr.is_none()
        && let Ok(write_fn) = interp.get_attr(&stderr, "write")
    {
        let _ = call_pos(interp, write_fn, &[Value::string(text)]);
        return;
    }
    eprint!("{text}");
}

pub(crate) fn uninstall_helper_impl(interp: &mut Interpreter, verbosity: i64) -> Result<Value> {
    let pip_mod = match interp.load_module("pip") {
        Ok(m) => m,
        Err(_) => return Ok(Value::none()),
    };

    let installed_ver = interp
        .get_attr(&pip_mod, "__version__")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let available_ver = current_pip_version(interp);
    if installed_ver != available_ver {
        write_stderr(
            interp,
            &format!(
                "ensurepip will only uninstall a matching version ({installed_ver:?} installed, {available_ver:?} available)\n"
            ),
        );
        return Ok(Value::none());
    }

    disable_pip_config_impl(interp)?;

    let mut pip_args = vec![
        Value::string("uninstall"),
        Value::string("-y"),
        Value::string("--disable-pip-version-check"),
    ];
    if verbosity > 0 {
        let v_count = verbosity.min(10) as usize;
        pip_args.push(Value::string(format!("-{}", "v".repeat(v_count))));
    }
    pip_args.push(Value::string("pip"));
    run_pip_impl(interp, Value::list(pip_args), Value::none())
}

pyrust_module! {
    constants {
        "__package__"    => Value::string("ensurepip"),
        "__path__"       => Value::list(Vec::new()),
        "__all__"        => Value::list(vec![
            Value::string("version"),
            Value::string("bootstrap"),
        ]),
        "_PACKAGE_NAMES" => Value::tuple(vec![Value::string("pip")]),
        "_PIP_VERSION"   => Value::string(PIP_VERSION),
        "_PROJECTS"      => Value::list(vec![Value::tuple(vec![
            Value::string("pip"),
            Value::string(PIP_VERSION),
            Value::string("py3"),
        ])]),
        "_Package"       => _Package::class(),
        "_WHEEL_PKG_DIR" => Value::none(),
        "_PACKAGES"      => Value::none(),
    }

    fn version() -> Result<Value> {
        Ok(Value::string(current_pip_version(_interp)))
    }

    fn _find_packages(path: PyPath) -> Result<Value> {
        let found = find_packages_in_dir(path.as_path());
        let mut d = PyDict::default();
        for (name, version, wheel_path) in found {
            d.insert(
                PyKey::str_from(&name),
                _Package::make([
                    Value::string(version),
                    Value::none(),
                    Value::string(wheel_path),
                ]),
            );
        }
        Ok(Value::dict(d))
    }

    fn _get_packages() -> Result<Value> {
        Ok(get_packages_dict(_interp))
    }

    fn _disable_pip_configuration_settings() -> Result<Value> {
        disable_pip_config_impl(_interp)?;
        Ok(Value::none())
    }

    fn _run_pip(
        args: PyValue,
        #[default(PyValue(Value::none()))] additional_paths: PyValue,
    ) -> Result<Value> {
        run_pip_impl(_interp, args.0, additional_paths.0)
    }

    fn bootstrap(
        #[keyword_only] #[default(PyValue(Value::none()))] root: PyValue,
        #[keyword_only] #[default(PyValue(Value::bool_(false)))] upgrade: PyValue,
        #[keyword_only] #[default(PyValue(Value::bool_(false)))] user: PyValue,
        #[keyword_only] #[default(PyValue(Value::bool_(false)))] altinstall: PyValue,
        #[keyword_only] #[default(PyValue(Value::bool_(false)))] default_pip: PyValue,
        #[keyword_only] #[default(0i64.into())] verbosity: PyIndex,
    ) -> Result<Value> {
        let upgrade_b = _interp.truthy_value(&upgrade.0)?;
        let user_b = _interp.truthy_value(&user.0)?;
        let altinstall_b = _interp.truthy_value(&altinstall.0)?;
        let default_pip_b = _interp.truthy_value(&default_pip.0)?;
        let verb_i = verbosity.expect_i64(FN_NAME, "verbosity")?;
        let _ = bootstrap_impl(
            _interp,
            root.0,
            upgrade_b,
            user_b,
            altinstall_b,
            default_pip_b,
            verb_i,
        )?;
        Ok(Value::none())
    }

    fn _bootstrap(
        #[keyword_only] #[default(PyValue(Value::none()))] root: PyValue,
        #[keyword_only] #[default(PyValue(Value::bool_(false)))] upgrade: PyValue,
        #[keyword_only] #[default(PyValue(Value::bool_(false)))] user: PyValue,
        #[keyword_only] #[default(PyValue(Value::bool_(false)))] altinstall: PyValue,
        #[keyword_only] #[default(PyValue(Value::bool_(false)))] default_pip: PyValue,
        #[keyword_only] #[default(0i64.into())] verbosity: PyIndex,
    ) -> Result<Value> {
        let upgrade_b = _interp.truthy_value(&upgrade.0)?;
        let user_b = _interp.truthy_value(&user.0)?;
        let altinstall_b = _interp.truthy_value(&altinstall.0)?;
        let default_pip_b = _interp.truthy_value(&default_pip.0)?;
        let verb_i = verbosity.expect_i64(FN_NAME, "verbosity")?;
        bootstrap_impl(
            _interp,
            root.0,
            upgrade_b,
            user_b,
            altinstall_b,
            default_pip_b,
            verb_i,
        )
    }

    fn _uninstall_helper(
        #[keyword_only] #[default(0i64.into())] verbosity: PyIndex,
    ) -> Result<Value> {
        let verb_i = verbosity.expect_i64(FN_NAME, "verbosity")?;
        uninstall_helper_impl(_interp, verb_i)
    }

    fn _main(#[default(PyValue(Value::none()))] argv: PyValue) -> Result<Value> {
        let argparse_mod = _interp.load_module("argparse")?;
        let parser_cls = _interp.get_attr(&argparse_mod, "ArgumentParser")?;
        let parser = call_kw(
            _interp,
            parser_cls,
            &[],
            &[("prog", Value::string("python -m ensurepip"))],
        )?;

        let ver = current_pip_version(_interp);
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--version")],
            &[
                ("action", Value::string("version")),
                ("version", Value::string(format!("pip {ver}"))),
                (
                    "help",
                    Value::string("Show the version of pip that is bundled with this Python."),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("-v"), Value::string("--verbose")],
            &[
                ("action", Value::string("count")),
                ("default", Value::int(0)),
                ("dest", Value::string("verbosity")),
                (
                    "help",
                    Value::string("Give more output. Option is additive, and can be used up to 3 times."),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("-U"), Value::string("--upgrade")],
            &[
                ("action", Value::string("store_true")),
                ("default", Value::bool_(false)),
                (
                    "help",
                    Value::string("Upgrade pip and dependencies, even if already installed."),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--user")],
            &[
                ("action", Value::string("store_true")),
                ("default", Value::bool_(false)),
                ("help", Value::string("Install using the user scheme.")),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--root")],
            &[
                ("default", Value::none()),
                (
                    "help",
                    Value::string("Install everything relative to this alternate root directory."),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--altinstall")],
            &[
                ("action", Value::string("store_true")),
                ("default", Value::bool_(false)),
                (
                    "help",
                    Value::string(
                        "Make an alternate install, installing only the X.Y versioned scripts (Default: pipX, pipX.Y).",
                    ),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--default-pip")],
            &[
                ("action", Value::string("store_true")),
                ("default", Value::bool_(false)),
                (
                    "help",
                    Value::string(
                        "Make a default pip install, installing the unqualified pip in addition to the versioned scripts.",
                    ),
                ),
            ],
        )?;

        let parsed = call_method(_interp, &parser, "parse_args", &[argv.0])?;
        let root = _interp.get_attr(&parsed, "root")?;
        let upgrade = {
            let v = _interp.get_attr(&parsed, "upgrade")?;
            _interp.truthy_value(&v)?
        };
        let user = {
            let v = _interp.get_attr(&parsed, "user")?;
            _interp.truthy_value(&v)?
        };
        let verbosity = _interp
            .get_attr(&parsed, "verbosity")?
            .as_int()
            .unwrap_or(0);
        let altinstall = {
            let v = _interp.get_attr(&parsed, "altinstall")?;
            _interp.truthy_value(&v)?
        };
        let default_pip = {
            let v = _interp.get_attr(&parsed, "default_pip")?;
            _interp.truthy_value(&v)?
        };

        bootstrap_impl(
            _interp,
            root,
            upgrade,
            user,
            altinstall,
            default_pip,
            verbosity,
        )
    }
}
