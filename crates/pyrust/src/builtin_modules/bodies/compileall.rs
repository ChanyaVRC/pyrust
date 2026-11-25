// `compileall` module — 100% native Rust implementation of Python 3.12's
// `compileall` module (issue #3249).
//
// Recursively compiles `.py` files in directories or on `sys.path` via
// `py_compile`, honoring `maxlevels`, `ddir`, `force`, `rx`, `quiet`, `legacy`,
// `optimize` (single int or iterable of ints), `invalidation_mode`, `stripdir`,
// `prependdir`, and `limit_sl_dest`.
//
// Reference: <https://docs.python.org/3.12/library/compileall.html>

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use pyrust_derive::pyrust_module;

use super::py_compile::{
    INVALIDATION_CHECKED_HASH, INVALIDATION_TIMESTAMP, PYC_MAGIC_NUMBER, compile_impl,
    compute_cache_path, resolve_invalidation_mode, source_hash_bytes,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyPath, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

fn write_stream(interp: &mut Interpreter, stream_name: &str, text: &str) {
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(stream) = interp.get_attr(&sys_mod, stream_name)
        && !stream.is_none()
        && let Ok(write_fn) = interp.get_attr(&stream, "write")
    {
        let _ = interp.call_function_expanded(
            write_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string(text),
            }],
        );
    }
}

fn rx_matches(interp: &mut Interpreter, rx: Option<&Value>, fullname: &str) -> Result<bool> {
    let Some(rx_val) = rx else {
        return Ok(false);
    };
    if rx_val.is_none() {
        return Ok(false);
    }
    let search_fn = interp.get_attr(rx_val, "search")?;
    let res = interp.call_function_expanded(
        search_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(fullname),
        }],
    )?;
    interp.truthy_value(&res)
}

fn extract_optimize_list(interp: &mut Interpreter, optimize: Option<&Value>) -> Result<Vec<i64>> {
    let Some(val) = optimize else {
        return Ok(vec![-1]);
    };
    match val.kind() {
        ValueKind::Int(n) => Ok(vec![n]),
        ValueKind::Bool(b) => Ok(vec![i64::from(b)]),
        _ => {
            let items = interp.collect_iterable(val)?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.kind() {
                    ValueKind::Int(n) => out.push(n),
                    ValueKind::Bool(b) => out.push(i64::from(b)),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "optimize must be an integer or sequence of integers".to_string(),
                        ));
                    }
                }
            }
            if out.is_empty() {
                out.push(-1);
            }
            Ok(out)
        }
    }
}

fn is_pyc_up_to_date(source_path: &Path, cfile_path: &Path, inv_mode: i64) -> bool {
    let Ok(pyc) = std::fs::read(cfile_path) else {
        return false;
    };
    if pyc.len() < 16 || pyc[0..4] != PYC_MAGIC_NUMBER {
        return false;
    }
    let bitfield = u32::from_le_bytes([pyc[4], pyc[5], pyc[6], pyc[7]]);
    if inv_mode == INVALIDATION_TIMESTAMP {
        if bitfield != 0 {
            return false;
        }
        let Ok(meta) = std::fs::metadata(source_path) else {
            return false;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        let size = meta.len() as u32;
        let expected_mtime = u32::from_le_bytes([pyc[8], pyc[9], pyc[10], pyc[11]]);
        let expected_size = u32::from_le_bytes([pyc[12], pyc[13], pyc[14], pyc[15]]);
        mtime == expected_mtime && size == expected_size
    } else if inv_mode == INVALIDATION_CHECKED_HASH {
        if bitfield != 3 {
            return false;
        }
        let Ok(src) = std::fs::read(source_path) else {
            return false;
        };
        let expected_hash = source_hash_bytes(&src);
        pyc[8..16] == expected_hash
    } else {
        // UNCHECKED_HASH: if bitfield == 1, assume up to date.
        bitfield == 1
    }
}

fn compute_dfile(
    fullname: &str,
    ddir: Option<&str>,
    stripdir: Option<&str>,
    prependdir: Option<&str>,
) -> Option<String> {
    if let Some(d) = ddir {
        let name = Path::new(fullname)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(fullname);
        return Some(Path::new(d).join(name).to_string_lossy().into_owned());
    }
    if stripdir.is_none() && prependdir.is_none() {
        return None;
    }
    let mut cur = fullname.to_string();
    if let Some(strip) = stripdir {
        let stripped = Path::new(&cur)
            .strip_prefix(strip)
            .ok()
            .map(|p| p.to_string_lossy().into_owned());
        if let Some(s) = stripped {
            cur = s;
        }
    }
    if let Some(pre) = prependdir {
        cur = Path::new(pre).join(&cur).to_string_lossy().into_owned();
    }
    Some(cur)
}

#[allow(clippy::too_many_arguments)]
fn compile_file_impl(
    interp: &mut Interpreter,
    fullname: &str,
    ddir: Option<&str>,
    force: bool,
    rx: Option<&Value>,
    quiet: i64,
    legacy: bool,
    optimizes: &[i64],
    invalidation_mode: Option<&Value>,
    stripdir: Option<&str>,
    prependdir: Option<&str>,
    limit_sl_dest: Option<&str>,
) -> Result<bool> {
    if ddir.is_some() && (stripdir.is_some() || prependdir.is_some()) {
        return Err(PyError::named(
            "ValueError",
            "Destination dir (ddir) cannot be used in combination with stripdir or prependdir"
                .to_string(),
        ));
    }

    if rx_matches(interp, rx, fullname)? {
        return Ok(true);
    }

    let path = Path::new(fullname);
    if let Some(limit) = limit_sl_dest
        && path.is_symlink()
        && let Ok(real_target) = std::fs::canonicalize(path)
        && let Ok(real_limit) = std::fs::canonicalize(limit)
        && !real_target.starts_with(&real_limit)
    {
        return Ok(true);
    }

    if !path.is_file() {
        return Ok(true);
    }
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext != "py" {
        return Ok(true);
    }

    let inv_mode = resolve_invalidation_mode(interp, invalidation_mode)?;
    let dfile = compute_dfile(fullname, ddir, stripdir, prependdir);
    let mut success = true;

    for &opt in optimizes {
        let cfile = if legacy {
            format!("{fullname}c")
        } else {
            compute_cache_path(interp, fullname, opt)?
        };

        if !force && is_pyc_up_to_date(path, Path::new(&cfile), inv_mode) {
            continue;
        }

        if quiet == 0 {
            write_stream(interp, "stdout", &format!("Compiling '{}'...\n", fullname));
        }

        match compile_impl(
            interp,
            fullname,
            Some(cfile),
            dfile.clone(),
            true,
            opt,
            invalidation_mode,
            0,
        ) {
            Ok(_) => {}
            Err(err) => {
                success = false;
                if quiet < 2 {
                    let msg = match interp.materialize_pyerror(err) {
                        Ok(exc) => interp.render_value_as_str(&exc).unwrap_or_default(),
                        Err(e) => format!("{e}"),
                    };
                    if quiet == 0 {
                        write_stream(interp, "stdout", &format!("*** {msg}\n"));
                    } else {
                        write_stream(
                            interp,
                            "stdout",
                            &format!("*** Error compiling '{}'...\n{msg}\n", fullname),
                        );
                    }
                }
            }
        }
    }

    Ok(success)
}

#[allow(clippy::too_many_arguments)]
fn compile_dir_impl(
    interp: &mut Interpreter,
    dir: &str,
    maxlevels: i64,
    ddir: Option<&str>,
    force: bool,
    rx: Option<&Value>,
    quiet: i64,
    legacy: bool,
    optimizes: &[i64],
    invalidation_mode: Option<&Value>,
    stripdir: Option<&str>,
    prependdir: Option<&str>,
    limit_sl_dest: Option<&str>,
) -> Result<bool> {
    if ddir.is_some() && (stripdir.is_some() || prependdir.is_some()) {
        return Err(PyError::named(
            "ValueError",
            "Destination dir (ddir) cannot be used in combination with stripdir or prependdir"
                .to_string(),
        ));
    }

    if quiet == 0 {
        write_stream(interp, "stdout", &format!("Listing '{}'...\n", dir));
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => {
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
        Err(_) => {
            if quiet < 2 {
                write_stream(interp, "stdout", &format!("Can't list '{}'\n", dir));
            }
            return Ok(false);
        }
    };

    let mut success = true;
    for name in entries {
        if name == "__pycache__" || name == "." || name == ".." {
            continue;
        }
        let full_path = Path::new(dir).join(&name);
        let full_str = full_path.to_string_lossy().into_owned();
        if full_path.is_file() {
            if !compile_file_impl(
                interp,
                &full_str,
                ddir,
                force,
                rx,
                quiet,
                legacy,
                optimizes,
                invalidation_mode,
                stripdir,
                prependdir,
                limit_sl_dest,
            )? {
                success = false;
            }
        } else if maxlevels > 0 && full_path.is_dir() && !full_path.is_symlink() {
            if rx_matches(interp, rx, &full_str)? {
                continue;
            }
            let sub_ddir = ddir.map(|d| Path::new(d).join(&name).to_string_lossy().into_owned());
            if !compile_dir_impl(
                interp,
                &full_str,
                maxlevels - 1,
                sub_ddir.as_deref(),
                force,
                rx,
                quiet,
                legacy,
                optimizes,
                invalidation_mode,
                stripdir,
                prependdir,
                limit_sl_dest,
            )? {
                success = false;
            }
        }
    }
    Ok(success)
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("compile_dir"),
            Value::string("compile_file"),
            Value::string("compile_path"),
        ]),
    }

    fn compile_dir(
        dir: PyPath,
        #[default(None)] maxlevels: Option<PyIndex>,
        #[default(None)] ddir: Option<PyPath>,
        #[default(PyBool(false))] force: PyBool,
        #[default(None)] rx: Option<PyValue>,
        #[default(PyIndex::from(0i64))] quiet: PyIndex,
        #[default(PyBool(false))] legacy: PyBool,
        #[default(None)] optimize: Option<PyValue>,
        #[default(PyIndex::from(1i64))] workers: PyIndex,
        #[default(None)] invalidation_mode: Option<PyValue>,
        #[keyword_only] #[default(None)] stripdir: Option<PyPath>,
        #[keyword_only] #[default(None)] prependdir: Option<PyPath>,
        #[keyword_only] #[default(None)] limit_sl_dest: Option<PyPath>,
        #[keyword_only] #[default(PyBool(false))] hardlink_dupes: PyBool,
    ) -> Result<Value> {
        let _ = hardlink_dupes;
        let workers_i = workers.expect_i64("compile_dir", "workers")?;
        if workers_i < 0 {
            return Err(PyError::named(
                "ValueError",
                "workers must be greater or equal to 0".to_string(),
            ));
        }
        let max_lv = match maxlevels {
            Some(m) => m.expect_i64("compile_dir", "maxlevels")?,
            None => _interp
                .load_module("sys")
                .and_then(|s| _interp.get_attr(&s, "getrecursionlimit"))
                .and_then(|f| _interp.call_function_expanded(f, &[]))
                .ok()
                .and_then(|v| match v.kind() {
                    ValueKind::Int(n) => Some(n),
                    _ => None,
                })
                .unwrap_or(1000),
        };
        let quiet_i = quiet.expect_i64("compile_dir", "quiet")?;
        let optimizes = extract_optimize_list(_interp, optimize.as_ref().map(|v| &v.0))?;
        let dir_str = dir.as_path().to_string_lossy().into_owned();
        let ddir_str = ddir.map(|p| p.as_path().to_string_lossy().into_owned());
        let strip_str = stripdir.map(|p| p.as_path().to_string_lossy().into_owned());
        let prepend_str = prependdir.map(|p| p.as_path().to_string_lossy().into_owned());
        let limit_str = limit_sl_dest.map(|p| p.as_path().to_string_lossy().into_owned());
        let ok = compile_dir_impl(
            _interp,
            &dir_str,
            max_lv,
            ddir_str.as_deref(),
            force.0,
            rx.as_ref().map(|v| &v.0),
            quiet_i,
            legacy.0,
            &optimizes,
            invalidation_mode.as_ref().map(|v| &v.0),
            strip_str.as_deref(),
            prepend_str.as_deref(),
            limit_str.as_deref(),
        )?;
        Ok(Value::bool_(ok))
    }

    fn compile_file(
        fullname: PyPath,
        #[default(None)] ddir: Option<PyPath>,
        #[default(PyBool(false))] force: PyBool,
        #[default(None)] rx: Option<PyValue>,
        #[default(PyIndex::from(0i64))] quiet: PyIndex,
        #[default(PyBool(false))] legacy: PyBool,
        #[default(None)] optimize: Option<PyValue>,
        #[default(None)] invalidation_mode: Option<PyValue>,
        #[keyword_only] #[default(None)] stripdir: Option<PyPath>,
        #[keyword_only] #[default(None)] prependdir: Option<PyPath>,
        #[keyword_only] #[default(None)] limit_sl_dest: Option<PyPath>,
        #[keyword_only] #[default(PyBool(false))] hardlink_dupes: PyBool,
    ) -> Result<Value> {
        let _ = hardlink_dupes;
        let quiet_i = quiet.expect_i64("compile_file", "quiet")?;
        let optimizes = extract_optimize_list(_interp, optimize.as_ref().map(|v| &v.0))?;
        let full_str = fullname.as_path().to_string_lossy().into_owned();
        let ddir_str = ddir.map(|p| p.as_path().to_string_lossy().into_owned());
        let strip_str = stripdir.map(|p| p.as_path().to_string_lossy().into_owned());
        let prepend_str = prependdir.map(|p| p.as_path().to_string_lossy().into_owned());
        let limit_str = limit_sl_dest.map(|p| p.as_path().to_string_lossy().into_owned());
        let ok = compile_file_impl(
            _interp,
            &full_str,
            ddir_str.as_deref(),
            force.0,
            rx.as_ref().map(|v| &v.0),
            quiet_i,
            legacy.0,
            &optimizes,
            invalidation_mode.as_ref().map(|v| &v.0),
            strip_str.as_deref(),
            prepend_str.as_deref(),
            limit_str.as_deref(),
        )?;
        Ok(Value::bool_(ok))
    }

    fn compile_path(
        #[default(None)] skip_curdir: Option<PyValue>,
        #[default(PyIndex::from(0i64))] maxlevels: PyIndex,
        #[default(None)] force: Option<PyValue>,
        #[default(PyIndex::from(0i64))] quiet: PyIndex,
        #[default(None)] legacy: Option<PyValue>,
        #[default(None)] optimize: Option<PyValue>,
        #[default(None)] invalidation_mode: Option<PyValue>,
    ) -> Result<Value> {
        let skip_curdir_b = match skip_curdir {
            None => true,
            Some(v) => _interp.truthy_value(&v.0)?,
        };
        let force_b = match force {
            None => false,
            Some(v) => _interp.truthy_value(&v.0)?,
        };
        let legacy_b = match legacy {
            None => false,
            Some(v) => _interp.truthy_value(&v.0)?,
        };
        let max_lv = maxlevels.expect_i64("compile_path", "maxlevels")?;
        let quiet_i = quiet.expect_i64("compile_path", "quiet")?;
        let optimizes = extract_optimize_list(_interp, optimize.as_ref().map(|v| &v.0))?;
        let sys_mod = _interp.load_module("sys")?;
        let path_val = _interp.get_attr(&sys_mod, "path")?;
        let entries = _interp.collect_iterable(&path_val)?;
        let mut success = true;
        for entry in entries {
            let Some(dir_str) = entry.as_str() else {
                continue;
            };
            if skip_curdir_b && (dir_str.is_empty() || dir_str == ".") {
                if quiet_i == 0 {
                    write_stream(_interp, "stdout", "Skipping current directory\n");
                }
                continue;
            }
            if !Path::new(dir_str).is_dir() {
                continue;
            }
            if !compile_dir_impl(
                _interp,
                dir_str,
                max_lv,
                None,
                force_b,
                None,
                quiet_i,
                legacy_b,
                &optimizes,
                invalidation_mode.as_ref().map(|v| &v.0),
                None,
                None,
                None,
            )? {
                success = false;
            }
        }
        Ok(Value::bool_(success))
    }

    fn main() -> Result<Value> {
        let sys_mod = _interp.load_module("sys")?;
        let argv = _interp.get_attr(&sys_mod, "argv")?;
        let items = _interp.collect_iterable(&argv)?;
        let targets: Vec<String> = items
            .into_iter()
            .skip(1)
            .filter_map(|v| v.as_str().map(str::to_owned))
            .filter(|s| !s.starts_with('-'))
            .collect();
        let mut ok = true;
        if targets.is_empty() {
            let res = compile_dir_impl(
                _interp,
                ".",
                10,
                None,
                false,
                None,
                0,
                false,
                &[-1],
                None,
                None,
                None,
                None,
            )?;
            ok &= res;
        } else {
            for t in targets {
                let p = PathBuf::from(&t);
                if p.is_dir() {
                    ok &= compile_dir_impl(
                        _interp, &t, 10, None, false, None, 0, false, &[-1], None, None, None,
                        None,
                    )?;
                } else {
                    ok &= compile_file_impl(
                        _interp, &t, None, false, None, 0, false, &[-1], None, None, None, None,
                    )?;
                }
            }
        }
        Ok(Value::bool_(ok))
    }
}
