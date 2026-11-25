// `webbrowser` module — 100% native Rust implementation (issue #3253).
//
// Provides:
// - `Error(Exception)`
// - `BaseBrowser`, `GenericBrowser(BaseBrowser)`, `BackgroundBrowser(GenericBrowser)`
// - `register(name, klass, instance=None, *, preferred=False)`
// - `get(using=None)`
// - `open(url, new=0, autoraise=True)`
// - `open_new(url)`
// - `open_new_tab(url)`
//
// Browser registry state (`_browsers` dict and `_tryorder` list) is stored on
// the live `webbrowser` module object for clean per-interpreter isolation and
// CPython 3.12 parity.
//
// Reference: <https://docs.python.org/3/library/webbrowser.html>

use std::path::Path;
use std::process::{Command, Stdio};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

fn basename_of(name: &str) -> String {
    Path::new(name)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.to_string())
}

#[cfg(unix)]
fn find_executable_on_path(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    let p = Path::new(cmd);
    if p.components().count() > 1 {
        return p.is_file();
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&path_var) {
        if dir.join(cmd).is_file() {
            return true;
        }
    }
    false
}

fn webbrowser_error(interp: &mut Interpreter, msg: &str) -> PyError {
    if let Ok(wb_mod) = interp.load_module("webbrowser")
        && let Ok(err_cls) = interp.get_attr(&wb_mod, "Error")
        && let ValueKind::PyClass(cls_rc) = err_cls.kind()
    {
        return PyError::class(std::rc::Rc::clone(cls_rc), msg.to_string());
    }
    PyError::named("webbrowser.Error", msg.to_string())
}

/// Ensure `_browsers` and `_tryorder` are initialized on the live `webbrowser`
/// module object (matching CPython's lazy `register_standard_browsers()`).
fn ensure_registry(interp: &mut Interpreter) -> Result<(Value, Value)> {
    let wb_mod = interp.load_module("webbrowser")?;
    let tryorder = interp
        .get_attr(&wb_mod, "_tryorder")
        .unwrap_or_else(|_| Value::none());
    let browsers = interp
        .get_attr(&wb_mod, "_browsers")
        .unwrap_or_else(|_| Value::none());

    if !tryorder.is_none() && !browsers.is_none() {
        return Ok((browsers, tryorder));
    }

    let new_browsers = Value::dict(PyDict::default());
    let new_tryorder = Value::list(Vec::new());

    if let ValueKind::PyModule(m) = wb_mod.kind() {
        let mut mb = m.borrow_mut();
        mb.attrs
            .insert("_browsers".to_string(), new_browsers.clone());
        mb.attrs
            .insert("_tryorder".to_string(), new_tryorder.clone());
    }

    // Seed standard platform defaults without ever launching anything.
    let bg_cls = interp.get_attr(&wb_mod, "BackgroundBrowser")?;
    let mut seed_entry = |name: &str, cmd: &str| -> Result<()> {
        let inst = interp.call_function_expanded(
            bg_cls.clone(),
            &[ExpandedCallArg {
                name: None,
                value: Value::string(cmd),
            }],
        )?;
        let key = name.to_ascii_lowercase();
        new_browsers.dict_insert(
            PyKey::str_from(&key),
            Value::list(vec![Value::none(), inst]),
        )?;
        let _ = new_tryorder.list_push(Value::string(key));
        Ok(())
    };

    #[cfg(target_os = "macos")]
    {
        if find_executable_on_path("open") {
            let _ = seed_entry("macosx", "open");
        }
    }
    #[cfg(target_os = "linux")]
    {
        if find_executable_on_path("xdg-open") {
            let _ = seed_entry("xdg-open", "xdg-open");
        }
    }
    #[cfg(windows)]
    {
        let _ = seed_entry("windows-default", "cmd");
    }

    // Honor BROWSER environment variable (matches CPython's os.pathsep split).
    if let Ok(env_browsers) = std::env::var("BROWSER")
        && !env_browsers.is_empty()
    {
        let sep = if cfg!(windows) { ';' } else { ':' };
        let mut insert_idx = 0usize;
        for raw in env_browsers.split(sep) {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            let lower = entry.to_ascii_lowercase();
            let already_present = new_browsers
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from(&lower)));
            if !already_present {
                let parts: Vec<Value> = entry.split_whitespace().map(Value::string).collect();
                if !parts.is_empty() {
                    let arg = if parts.len() == 1 {
                        parts[0].clone()
                    } else {
                        Value::list(parts)
                    };
                    if let Ok(inst) = interp.call_function_expanded(
                        bg_cls.clone(),
                        &[ExpandedCallArg {
                            name: None,
                            value: arg,
                        }],
                    ) {
                        let _ = new_browsers.dict_insert(
                            PyKey::str_from(&lower),
                            Value::list(vec![Value::none(), inst]),
                        );
                    }
                }
            }
            let val = Value::string(lower);
            let _ = new_tryorder.list_insert(insert_idx, val);
            insert_idx += 1;
        }
    }

    Ok((new_browsers, new_tryorder))
}

fn resolve_browser_entry(interp: &mut Interpreter, entry: &Value) -> Result<Option<Value>> {
    let items: Vec<Value> = match entry.kind() {
        ValueKind::List(l) => l.clone(),
        ValueKind::Tuple(t) => t.as_ref().to_vec(),
        _ => return Ok(None),
    };
    if items.len() < 2 {
        return Ok(None);
    }
    let klass = &items[0];
    let instance = &items[1];
    if !instance.is_none() {
        return Ok(Some(instance.clone()));
    }
    if !klass.is_none() {
        let built = interp.call_function_expanded(klass.clone(), &[])?;
        return Ok(Some(built));
    }
    Ok(None)
}

fn build_browser_command(
    interp: &mut Interpreter,
    self_val: &Value,
    url: &str,
) -> Result<(String, Vec<String>)> {
    let name_val = interp.get_attr(self_val, "name")?;
    let name = match name_val.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => {
            return Err(PyError::named("TypeError", "browser name must be a str"));
        }
    };
    let args_val = interp.get_attr(self_val, "args")?;
    let arg_items = interp.collect_iterable(&args_val)?;
    let mut cmd_args = Vec::with_capacity(arg_items.len());
    for item in arg_items {
        let s = match item.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => item.repr_raw(),
        };
        cmd_args.push(s.replace("%s", url));
    }
    Ok((name, cmd_args))
}

fn open_impl(interp: &mut Interpreter, url: &str, new: i64, autoraise: bool) -> Result<Value> {
    let (browsers, tryorder) = ensure_registry(interp)?;
    let order = interp.collect_iterable(&tryorder)?;
    for name_val in order {
        let Some(name_str) = name_val.as_str() else {
            continue;
        };
        let key = PyKey::str_from(name_str.to_ascii_lowercase());
        let entry = browsers.as_dict().and_then(|d| d.get(&key).cloned());
        let Some(entry) = entry else {
            continue;
        };
        if let Some(browser) = resolve_browser_entry(interp, &entry)? {
            let open_fn = interp.get_attr(&browser, "open")?;
            let res = interp.call_function_expanded(
                open_fn,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: Value::string(url),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: Value::int(new),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: Value::bool_(autoraise),
                    },
                ],
            )?;
            if interp.truthy_value(&res)? {
                return Ok(Value::bool_(true));
            }
        }
    }
    Ok(Value::bool_(false))
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("Error"),
            Value::string("open"),
            Value::string("open_new"),
            Value::string("open_new_tab"),
            Value::string("get"),
            Value::string("register"),
        ]),
        "_browsers" => Value::none(),
        "_tryorder" => Value::none(),
    }

    /// Exception raised when a browser control error occurs.
    exception Error(Exception);

    /// Parent class for all browsers. Do not use directly.
    class BaseBrowser {
        constants {
            "args" => Value::list(vec![Value::string("%s")]),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] name: PyStr<'_>,
        ) -> Result<Value> {
            let ValueKind::PyInstance(inst) = self_val.0.kind() else {
                return Err(PyError::named("TypeError", "expected BaseBrowser instance"));
            };
            let mut ib = inst.borrow_mut();
            ib.attrs.insert("name", Value::string(&*name));
            ib.attrs.insert("basename", Value::string(&*name));
            Ok(Value::none())
        }

        fn open(
            #[positional_only] _self: PyValueRef<'_>,
            url: PyStr<'_>,
            #[default(0_i64.into())] new: PyIndex,
            #[default(true.into())] autoraise: PyBool,
        ) -> Result<Value> {
            let _ = (url, new, autoraise);
            Err(PyError::named("NotImplementedError", ""))
        }

        fn open_new(
            #[positional_only] self_val: PyValueRef<'_>,
            url: PyStr<'_>,
        ) -> Result<Value> {
            let open_fn = _interp.get_attr(self_val.0, "open")?;
            _interp.call_function_expanded(
                open_fn,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: Value::string(&*url),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: Value::int(1),
                    },
                ],
            )
        }

        fn open_new_tab(
            #[positional_only] self_val: PyValueRef<'_>,
            url: PyStr<'_>,
        ) -> Result<Value> {
            let open_fn = _interp.get_attr(self_val.0, "open")?;
            _interp.call_function_expanded(
                open_fn,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: Value::string(&*url),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: Value::int(2),
                    },
                ],
            )
        }
    }

    /// Class for all browsers started with a command and without remote functionality.
    class GenericBrowser("BaseBrowser") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let ValueKind::PyInstance(inst) = self_val.0.kind() else {
                return Err(PyError::named("TypeError", "expected GenericBrowser instance"));
            };
            let (cmd_name, args_list) = match name.0.kind() {
                ValueKind::Str(s) => (s.to_string(), vec![Value::string("%s")]),
                _ => {
                    let items = _interp.collect_iterable(&name.0)?;
                    if items.is_empty() {
                        return Err(PyError::named(
                            "IndexError",
                            "GenericBrowser name sequence cannot be empty",
                        ));
                    }
                    let first = match items[0].kind() {
                        ValueKind::Str(s) => s.to_string(),
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "GenericBrowser command must be str",
                            ));
                        }
                    };
                    (first, items[1..].to_vec())
                }
            };
            let base = basename_of(&cmd_name);
            let mut ib = inst.borrow_mut();
            ib.attrs.insert("name", Value::string(cmd_name));
            ib.attrs.insert("args", Value::list(args_list));
            ib.attrs.insert("basename", Value::string(base));
            Ok(Value::none())
        }

        fn open(
            #[positional_only] self_val: PyValueRef<'_>,
            url: PyStr<'_>,
            #[default(0_i64.into())] new: PyIndex,
            #[default(true.into())] autoraise: PyBool,
        ) -> Result<Value> {
            let _ = (new, autoraise);
            let (cmd, args) = build_browser_command(_interp, self_val.0, &url)?;
            let ok = Command::new(&cmd)
                .args(&args)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            Ok(Value::bool_(ok))
        }
    }

    /// Class for all browsers which are to be started in the background.
    class BackgroundBrowser("GenericBrowser") {
        fn open(
            #[positional_only] self_val: PyValueRef<'_>,
            url: PyStr<'_>,
            #[default(0_i64.into())] new: PyIndex,
            #[default(true.into())] autoraise: PyBool,
        ) -> Result<Value> {
            let _ = (new, autoraise);
            let (cmd, args) = build_browser_command(_interp, self_val.0, &url)?;
            let ok = Command::new(&cmd)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok();
            Ok(Value::bool_(ok))
        }
    }

    /// Register a browser connector.
    fn register(
        name: PyStr<'_>,
        klass: PyValue,
        #[default(None)] instance: Option<PyValue>,
        #[keyword_only]
        #[default(false.into())]
        preferred: PyBool,
    ) -> Result<Value> {
        let (browsers, tryorder) = ensure_registry(_interp)?;
        let lower = name.to_ascii_lowercase();
        let inst_val = instance.map(|v| v.0).unwrap_or_else(Value::none);
        browsers.dict_insert(
            PyKey::str_from(&lower),
            Value::list(vec![klass.0, inst_val]),
        )?;
        if *preferred {
            let _ = tryorder.list_insert(0, Value::string(lower));
        } else {
            let _ = tryorder.list_push(Value::string(lower));
        }
        Ok(Value::none())
    }

    /// Return a browser launcher instance appropriate for the environment.
    fn get(#[default(None)] using: Option<PyStr<'_>>) -> Result<Value> {
        let (browsers, tryorder) = ensure_registry(_interp)?;
        let candidates: Vec<String> = match using {
            Some(name) => vec![name.to_string()],
            None => _interp
                .collect_iterable(&tryorder)?
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
        };

        for candidate in candidates {
            if candidate.contains("%s") {
                let parts: Vec<Value> = candidate
                    .split_whitespace()
                    .map(Value::string)
                    .collect();
                if !parts.is_empty() {
                    let wb_mod = _interp.load_module("webbrowser")?;
                    let bg_cls = _interp.get_attr(&wb_mod, "BackgroundBrowser")?;
                    return _interp.call_function_expanded(
                        bg_cls,
                        &[ExpandedCallArg {
                            name: None,
                            value: Value::list(parts),
                        }],
                    );
                }
            }
            let key = PyKey::str_from(candidate.to_ascii_lowercase());
            let entry = browsers.as_dict().and_then(|d| d.get(&key).cloned());
            if let Some(entry) = entry
                && let Some(browser) = resolve_browser_entry(_interp, &entry)?
            {
                return Ok(browser);
            }
        }
        Err(webbrowser_error(_interp, "could not locate runnable browser"))
    }

    /// Display url using the default browser.
    fn open(
        url: PyStr<'_>,
        #[default(0_i64.into())] new: PyIndex,
        #[default(true.into())] autoraise: PyBool,
    ) -> Result<Value> {
        let new_val = new.expect_i64(FN_NAME, "new")?;
        open_impl(_interp, &url, new_val, *autoraise)
    }

    /// Open url in a new window of the default browser, if possible.
    fn open_new(url: PyStr<'_>) -> Result<Value> {
        open_impl(_interp, &url, 1, true)
    }

    /// Open url in a new page ("tab") of the default browser, if possible.
    fn open_new_tab(url: PyStr<'_>) -> Result<Value> {
        open_impl(_interp, &url, 2, true)
    }
}
