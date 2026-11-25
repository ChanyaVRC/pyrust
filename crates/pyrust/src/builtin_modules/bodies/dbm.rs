// `dbm` package module — 100% native Rust implementation of Python 3.12's
// generic `dbm` interface (`open`, `whichdb`, `error`, and `dbm.dumb`)
// (issue #3259).
//
// Reference: <https://docs.python.org/3/library/dbm.html>

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyPath, PyStr};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class};
use crate::value::{Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn append_suffix(base: &Path, suffix: &str) -> PathBuf {
    let mut os = OsString::from(base.as_os_str());
    os.push(suffix);
    PathBuf::from(os)
}

fn raise_dbm_error(interp: &mut Interpreter, msg: &str) -> PyError {
    if let Ok(dbm_mod) = interp.load_module("dbm")
        && let Ok(err_attr) = interp.get_attr(&dbm_mod, "error")
    {
        let err_cls = match err_attr.kind() {
            ValueKind::Tuple(items) if !items.is_empty() => items[0].clone(),
            _ => err_attr.clone(),
        };
        if let Ok(inst) = call_pos(interp, err_cls, &[Value::string(msg)]) {
            return PyError::raised(inst);
        }
    }
    PyError::named("OSError", msg)
}

fn detect_whichdb(path: &Path) -> Option<&'static str> {
    // 1. Check for ndbm (.pag + .dir).
    let pag_path = append_suffix(path, ".pag");
    let dir_path = append_suffix(path, ".dir");
    if File::open(&pag_path).is_ok() && File::open(&dir_path).is_ok() {
        return Some("dbm.ndbm");
    }

    // 2. Check for dbm.dumb (.dat + .dir).
    let dat_path = append_suffix(path, ".dat");
    if fs::metadata(&dat_path).is_ok()
        && let Ok(dir_meta) = fs::metadata(&dir_path)
    {
        if dir_meta.len() == 0 {
            return Some("dbm.dumb");
        }
        if let Ok(mut f) = File::open(&dir_path) {
            let mut first = [0u8; 1];
            if f.read_exact(&mut first).is_ok() && (first[0] == b'\'' || first[0] == b'"') {
                return Some("dbm.dumb");
            }
        }
    }

    // 3. Check if the path itself exists; None if unreadable/absent.
    let mut f = File::open(path).ok()?;
    let mut buf = [0u8; 16];
    let n = f.read(&mut buf).unwrap_or(0);
    if n < 4 {
        return Some("");
    }

    // Native-endian 4-byte magic check for GNU dbm.
    let magic = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if matches!(magic, 0x1357_9ACD..=0x1357_9ACF) {
        return Some("dbm.gnu");
    }

    // Unknown readable file format.
    Some("")
}

/// Replace `dbm.error` with CPython 3.12's exact 2-tuple `(dbm.error, OSError)`.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let Some(err_cls) = mod_rc.borrow().attrs.get("error").cloned() else {
        return;
    };
    let os_err = Value::py_class(lookup_exc_class("OSError").expect("OSError must exist"));
    mod_rc
        .borrow_mut()
        .insert_attr("error".to_string(), Value::tuple(vec![err_cls, os_err]));
}

pyrust_module! {
    constants {
        "__package__" => Value::string("dbm"),
        "__all__" => Value::list(vec![
            Value::string("open"),
            Value::string("whichdb"),
            Value::string("error"),
        ]),
        "dumb" => super::dbm_dumb::module(),
    }

    /// Generic DBM error (wrapped into `dbm.error = (error, OSError)` during
    /// module preparation, matching CPython 3.12).
    exception error;

    fn whichdb(filename: PyPath) -> Result<Value> {
        match detect_whichdb(filename.as_path()) {
            None => Ok(Value::none()),
            Some(name) => Ok(Value::string(name)),
        }
    }

    fn open(
        file: PyPath,
        #[default("r".into())] flag: PyStr<'_>,
        #[default(0o666i64.into())] mode: PyIndex,
    ) -> Result<Value> {
        let flag_str = &*flag;
        let mode_i64 = mode.expect_i64(FN_NAME, "mode")?;
        let detected = if flag_str.contains('n') {
            None
        } else {
            detect_whichdb(file.as_path())
        };

        let target_mod = match detected {
            None => {
                if flag_str.contains('c') || flag_str.contains('n') {
                    "dbm.dumb"
                } else {
                    return Err(raise_dbm_error(
                        _interp,
                        "db file doesn't exist; use 'c' or 'n' flag to create a new db",
                    ));
                }
            }
            Some("") => {
                return Err(raise_dbm_error(_interp, "db type could not be determined"));
            }
            Some("dbm.dumb") => "dbm.dumb",
            Some(other) => {
                return Err(raise_dbm_error(
                    _interp,
                    &format!("db type is {other}, but the module is not available"),
                ));
            }
        };

        let backend = _interp.load_module(target_mod)?;
        let open_fn = _interp.get_attr(&backend, "open")?;
        let path_val = if file.is_bytes() {
            Value::bytes(file.as_path().as_os_str().as_encoded_bytes().to_vec())
        } else {
            Value::string(file.as_path().to_string_lossy())
        };
        call_pos(
            _interp,
            open_fn,
            &[path_val, Value::string(flag_str), Value::int(mode_i64)],
        )
    }
}
