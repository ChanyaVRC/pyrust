// `trace` module — 100% native Rust implementation of Python 3.12's `trace`
// module (issue #3247).
//
// Provides `Trace` and `CoverageResults` built directly on PyRust's live
// `sys.settrace` hooks using `pyrust_module!`.
//
// Reference: <https://docs.python.org/3/library/trace.html>

use std::path::Path;
use std::time::Instant;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{PyDict, PySet, Value, ValueKind};

const PRAGMA_NOCOVER: &str = "#pragma NO COVER";

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
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

fn trace_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("trace")?;
    interp.get_attr(&m, name)
}

fn write_stdout(interp: &mut Interpreter, text: &str) -> Result<()> {
    let sys_mod = interp.load_module("sys")?;
    let stdout = interp.get_attr(&sys_mod, "stdout")?;
    call_method(interp, &stdout, "write", &[Value::string(text)])?;
    Ok(())
}

fn write_stderr(interp: &mut Interpreter, text: &str) -> Result<()> {
    let sys_mod = interp.load_module("sys")?;
    let stderr = interp.get_attr(&sys_mod, "stderr")?;
    call_method(interp, &stderr, "write", &[Value::string(text)])?;
    Ok(())
}

fn modname_rs(path: &str) -> String {
    let base = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    match base.rfind('.') {
        Some(idx) if idx > 0 => base[..idx].to_string(),
        _ => base.to_string(),
    }
}

fn fullmodname_rs(interp: &mut Interpreter, path: &str) -> Result<String> {
    let os_mod = interp.load_module("os")?;
    let sep = interp
        .get_attr(&os_mod, "sep")?
        .as_str()
        .unwrap_or("/")
        .to_string();
    let altsep = interp
        .get_attr(&os_mod, "altsep")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned));
    let os_path = interp.load_module("os.path")?;
    let normcase = interp.get_attr(&os_path, "normcase")?;
    let splitdrive = interp.get_attr(&os_path, "splitdrive")?;
    let splitext = interp.get_attr(&os_path, "splitext")?;

    let comparepath = call_pos(interp, normcase.clone(), &[Value::string(path)])?
        .as_str()
        .unwrap_or(path)
        .to_string();
    let sys_mod = interp.load_module("sys")?;
    let sys_path = interp.get_attr(&sys_mod, "path")?;
    let mut longest = String::new();
    for dir_val in interp.collect_iterable(&sys_path)? {
        let dir_nc = call_pos(interp, normcase.clone(), &[dir_val])?
            .as_str()
            .unwrap_or("")
            .to_string();
        if !dir_nc.is_empty()
            && comparepath.starts_with(&dir_nc)
            && comparepath[dir_nc.len()..].starts_with(&sep)
            && dir_nc.len() > longest.len()
        {
            longest = dir_nc;
        }
    }
    let base = if !longest.is_empty() && path.len() > longest.len() + sep.len() {
        path[longest.len() + sep.len()..].to_string()
    } else {
        path.to_string()
    };
    let sd = call_pos(interp, splitdrive, &[Value::string(&base)])?;
    let sd_parts = interp.collect_iterable(&sd)?;
    let mut b = sd_parts
        .get(1)
        .and_then(|v| v.as_str())
        .unwrap_or(&base)
        .replace(&sep, ".");
    if let Some(asep) = altsep
        && !asep.is_empty()
    {
        b = b.replace(&asep, ".");
    }
    let se = call_pos(interp, splitext, &[Value::string(&b)])?;
    let se_parts = interp.collect_iterable(&se)?;
    let fname = se_parts.first().and_then(|v| v.as_str()).unwrap_or(&b);
    Ok(fname.trim_start_matches('.').to_string())
}

fn find_executable_linenos_rs(interp: &mut Interpreter, filename: &str) -> Result<Value> {
    let source = match std::fs::read_to_string(filename) {
        Ok(s) => s,
        Err(e) => {
            let fn_r = render_value_repr(interp, &Value::string(filename))?;
            let _ = write_stderr(
                interp,
                &format!("Not printing coverage data for {fn_r}: {e}\n"),
            );
            return Ok(Value::dict(PyDict::default()));
        }
    };
    let code = match call_pos(
        interp,
        Value::builtin_function("compile"),
        &[
            Value::string(&source),
            Value::string(filename),
            Value::string("exec"),
        ],
    ) {
        Ok(c) => c,
        Err(_) => return Ok(Value::dict(PyDict::default())),
    };
    let dis_mod = interp.load_module("dis")?;
    let findlinestarts = interp.get_attr(&dis_mod, "findlinestarts")?;
    let inspect_mod = interp.load_module("inspect")?;
    let iscode = interp.get_attr(&inspect_mod, "iscode")?;

    let linenos = Value::dict(PyDict::default());
    let mut stack = vec![code];
    while let Some(co) = stack.pop() {
        if let Ok(starts) = call_pos(interp, findlinestarts.clone(), std::slice::from_ref(&co)) {
            for pair in interp.collect_iterable(&starts)? {
                let items = interp.collect_iterable(&pair)?;
                if items.len() == 2 && !items[1].is_none() {
                    call_method(
                        interp,
                        &linenos,
                        "__setitem__",
                        &[items[1].clone(), Value::int(1)],
                    )?;
                }
            }
        }
        if let Ok(consts) = interp.get_attr(&co, "co_consts") {
            for c in interp.collect_iterable(&consts)? {
                if let Ok(is_c) = call_pos(interp, iscode.clone(), std::slice::from_ref(&c))
                    && interp.truthy_value(&is_c)?
                {
                    stack.push(c);
                }
            }
        }
    }
    Ok(linenos)
}

fn set_sys_and_threading_trace(
    interp: &mut Interpreter,
    func: Value,
    with_threading: bool,
) -> Result<()> {
    if with_threading
        && let Ok(th_mod) = interp.load_module("threading")
        && let Ok(settrace) = interp.get_attr(&th_mod, "settrace")
    {
        call_pos(interp, settrace, std::slice::from_ref(&func))?;
    }
    let sys_mod = interp.load_module("sys")?;
    let settrace = interp.get_attr(&sys_mod, "settrace")?;
    call_pos(interp, settrace, &[func])?;
    Ok(())
}

struct TraceTiming {
    start: Option<Instant>,
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("Trace"),
            Value::string("CoverageResults"),
        ]),
        "PRAGMA_NOCOVER" => Value::string(PRAGMA_NOCOVER),
    }

    fn _modname(path: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(modname_rs(path.0.as_ref())))
    }

    fn _fullmodname(path: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(fullmodname_rs(_interp, path.0.as_ref())?))
    }

    class _Ignore {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] modules: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] dirs: OptValueRef<'_>,
        ) -> Result<Value> {
            let set_cls = crate::interpreter::primitive_class_by_name("set")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("set"));
            let mods = match modules.as_value() {
                Some(v) if _interp.truthy_value(v)? => {
                    call_pos(_interp, set_cls, std::slice::from_ref(v))?
                }
                _ => Value::set(PySet::default()),
            };
            let os_path = _interp.load_module("os.path")?;
            let normpath = _interp.get_attr(&os_path, "normpath")?;
            let mut dir_list = Vec::new();
            if let Some(d_val) = dirs.as_value()
                && _interp.truthy_value(d_val)?
            {
                for d in _interp.collect_iterable(d_val)? {
                    dir_list.push(call_pos(_interp, normpath.clone(), &[d])?);
                }
            }
            let ignore_map = Value::dict(PyDict::default());
            call_method(
                _interp,
                &ignore_map,
                "__setitem__",
                &[Value::string("<string>"), Value::int(1)],
            )?;
            _interp.assign_attr(self_val.0.clone(), "_mods", mods)?;
            _interp.assign_attr(self_val.0.clone(), "_dirs", Value::list(dir_list))?;
            _interp.assign_attr(self_val.0.clone(), "_ignore", ignore_map)?;
            Ok(Value::none())
        }

        fn names(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            modulename: PyValue,
        ) -> Result<Value> {
            let ignore = _interp.get_attr(self_val.0, "_ignore")?;
            let has = call_method(
                _interp,
                &ignore,
                "__contains__",
                std::slice::from_ref(&modulename.0),
            )?;
            if _interp.truthy_value(&has)? {
                return _interp.eval_index(&ignore, modulename.0);
            }
            let mods = _interp.get_attr(self_val.0, "_mods")?;
            let in_mods = call_method(
                _interp,
                &mods,
                "__contains__",
                std::slice::from_ref(&modulename.0),
            )?;
            if _interp.truthy_value(&in_mods)? {
                call_method(_interp, &ignore, "__setitem__", &[modulename.0, Value::int(1)])?;
                return Ok(Value::int(1));
            }
            let mod_str = modulename.0.as_str().unwrap_or("");
            for m in _interp.collect_iterable(&mods)? {
                if let Some(ms) = m.as_str()
                    && mod_str.starts_with(&format!("{ms}."))
                {
                    call_method(_interp, &ignore, "__setitem__", &[modulename.0, Value::int(1)])?;
                    return Ok(Value::int(1));
                }
            }
            if filename.0.is_none() {
                call_method(_interp, &ignore, "__setitem__", &[modulename.0, Value::int(1)])?;
                return Ok(Value::int(1));
            }
            let fname_str = filename.0.as_str().unwrap_or("");
            let os_mod = _interp.load_module("os")?;
            let sep = _interp
                .get_attr(&os_mod, "sep")?
                .as_str()
                .unwrap_or("/")
                .to_string();
            let dirs = _interp.get_attr(self_val.0, "_dirs")?;
            for d in _interp.collect_iterable(&dirs)? {
                if let Some(ds) = d.as_str()
                    && fname_str.starts_with(&format!("{ds}{sep}"))
                {
                    call_method(_interp, &ignore, "__setitem__", &[modulename.0, Value::int(1)])?;
                    return Ok(Value::int(1));
                }
            }
            call_method(_interp, &ignore, "__setitem__", &[modulename.0, Value::int(0)])?;
            Ok(Value::int(0))
        }
    }

    class CoverageResults {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] counts: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] calledfuncs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] infile: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] callers: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] outfile: OptValueRef<'_>,
        ) -> Result<Value> {
            let counts_val = match counts.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => Value::dict(PyDict::default()),
            };
            let counter_val = call_method(_interp, &counts_val, "copy", &[])?;
            let cf_val = match calledfuncs.as_value().filter(|v| !v.is_none()) {
                Some(v) => call_method(_interp, v, "copy", &[])?,
                None => Value::dict(PyDict::default()),
            };
            let callers_val = match callers.as_value().filter(|v| !v.is_none()) {
                Some(v) => call_method(_interp, v, "copy", &[])?,
                None => Value::dict(PyDict::default()),
            };
            let infile_val = infile.as_value().cloned().unwrap_or_else(Value::none);
            let outfile_val = outfile.as_value().cloned().unwrap_or_else(Value::none);

            _interp.assign_attr(self_val.0.clone(), "counts", counts_val)?;
            _interp.assign_attr(self_val.0.clone(), "counter", counter_val)?;
            _interp.assign_attr(self_val.0.clone(), "calledfuncs", cf_val)?;
            _interp.assign_attr(self_val.0.clone(), "callers", callers_val)?;
            _interp.assign_attr(self_val.0.clone(), "infile", infile_val.clone())?;
            _interp.assign_attr(self_val.0.clone(), "outfile", outfile_val)?;

            if _interp.truthy_value(&infile_val)? {
                let load_res = (|| -> Result<Value> {
                    let path_str = _interp.render_value_as_str(&infile_val)?;
                    let bytes = std::fs::read(&path_str)
                        .map_err(|e| PyError::from_io_error(&e, Some(&path_str)))?;
                    let pickle_mod = _interp.load_module("pickle")?;
                    let loads = _interp.get_attr(&pickle_mod, "loads")?;
                    call_pos(_interp, loads, &[Value::bytes(bytes)])
                })();
                match load_res {
                    Ok(loaded) => {
                        let triple = _interp.collect_iterable(&loaded)?;
                        if triple.len() == 3 {
                            let cr_cls = trace_attr(_interp, "CoverageResults")?;
                            let other = call_kw(
                                _interp,
                                cr_cls,
                                &[triple[0].clone(), triple[1].clone()],
                                &[("callers", triple[2].clone())],
                            )?;
                            call_method(_interp, self_val.0, "update", &[other])?;
                        }
                    }
                    Err(err) => {
                        let infile_r = render_value_repr(_interp, &infile_val)?;
                        let _ = write_stderr(
                            _interp,
                            &format!("Skipping counts file {infile_r}: {err}\n"),
                        );
                    }
                }
            }
            Ok(Value::none())
        }

        fn is_ignored_filename(
            #[positional_only] _self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            Ok(Value::bool_(
                filename.0.starts_with('<') && filename.0.ends_with('>'),
            ))
        }

        fn update(
            #[positional_only] self_val: PyValueRef<'_>,
            other: PyValue,
        ) -> Result<Value> {
            let counts = _interp.get_attr(self_val.0, "counts")?;
            let calledfuncs = _interp.get_attr(self_val.0, "calledfuncs")?;
            let callers = _interp.get_attr(self_val.0, "callers")?;
            let other_counts = _interp.get_attr(&other.0, "counts")?;
            let other_cf = _interp.get_attr(&other.0, "calledfuncs")?;
            let other_callers = _interp.get_attr(&other.0, "callers")?;

            let oc_items = call_method(_interp, &other_counts, "items", &[])?;
            for pair in _interp.collect_iterable(&oc_items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let prev = call_method(&mut *_interp, &counts, "get", &[kv[0].clone(), Value::int(0)])?;
                    let sum = _interp.eval_binary(prev, crate::ast::BinaryOp::Add, kv[1].clone())?;
                    call_method(_interp, &counts, "__setitem__", &[kv[0].clone(), sum])?;
                }
            }
            let ocf_keys = call_method(_interp, &other_cf, "keys", &[])?;
            for k in _interp.collect_iterable(&ocf_keys)? {
                call_method(_interp, &calledfuncs, "__setitem__", &[k, Value::int(1)])?;
            }
            let ocal_keys = call_method(_interp, &other_callers, "keys", &[])?;
            for k in _interp.collect_iterable(&ocal_keys)? {
                call_method(_interp, &callers, "__setitem__", &[k, Value::int(1)])?;
            }
            Ok(Value::none())
        }

        fn write_results(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] show_missing: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] summary: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] coverdir: OptValueRef<'_>,
        ) -> Result<Value> {
            let show_miss = match show_missing.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => true,
            };
            let do_summary = match summary.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => false,
            };
            let coverdir_opt = coverdir
                .as_value()
                .filter(|v| !v.is_none())
                .and_then(|v| v.as_str().map(str::to_owned));

            let calledfuncs = _interp.get_attr(self_val.0, "calledfuncs")?;
            if _interp.truthy_value(&calledfuncs)? {
                write_stdout(_interp, "\nfunctions called:\n")?;
                let keys = call_method(_interp, &calledfuncs, "keys", &[])?;
                let sorted_keys = call_pos(_interp, Value::builtin_function("sorted"), &[keys])?;
                for entry in _interp.collect_iterable(&sorted_keys)? {
                    let parts = _interp.collect_iterable(&entry)?;
                    if parts.len() == 3 {
                        let f = _interp.render_value_as_str(&parts[0])?;
                        let m = _interp.render_value_as_str(&parts[1])?;
                        let fn_name = _interp.render_value_as_str(&parts[2])?;
                        write_stdout(
                            _interp,
                            &format!("filename: {f}, modulename: {m}, funcname: {fn_name}\n"),
                        )?;
                    }
                }
            }

            let callers = _interp.get_attr(self_val.0, "callers")?;
            if _interp.truthy_value(&callers)? {
                write_stdout(_interp, "\ncalling relationships:\n")?;
                let keys = call_method(_interp, &callers, "keys", &[])?;
                let sorted_keys = call_pos(_interp, Value::builtin_function("sorted"), &[keys])?;
                let mut lastfile = String::new();
                let mut lastcfile = String::new();
                for entry in _interp.collect_iterable(&sorted_keys)? {
                    let outer = _interp.collect_iterable(&entry)?;
                    if outer.len() != 2 {
                        continue;
                    }
                    let parent = _interp.collect_iterable(&outer[0])?;
                    let child = _interp.collect_iterable(&outer[1])?;
                    if parent.len() != 3 || child.len() != 3 {
                        continue;
                    }
                    let pfile = _interp.render_value_as_str(&parent[0])?;
                    let pmod = _interp.render_value_as_str(&parent[1])?;
                    let pfunc = _interp.render_value_as_str(&parent[2])?;
                    let cfile = _interp.render_value_as_str(&child[0])?;
                    let cmod = _interp.render_value_as_str(&child[1])?;
                    let cfunc = _interp.render_value_as_str(&child[2])?;
                    if pfile != lastfile {
                        write_stdout(_interp, &format!("\n*** {pfile} ***\n"))?;
                        lastfile = pfile.clone();
                        lastcfile.clear();
                    }
                    if cfile != pfile && lastcfile != cfile {
                        write_stdout(_interp, &format!("  --> {cfile}\n"))?;
                        lastcfile = cfile;
                    }
                    write_stdout(_interp, &format!("    {pmod}.{pfunc} -> {cmod}.{cfunc}\n"))?;
                }
            }

            // Group counts per file.
            let counts = _interp.get_attr(self_val.0, "counts")?;
            let count_items = call_method(_interp, &counts, "items", &[])?;
            let mut per_file: IndexMap<String, Vec<(i64, i64)>> = IndexMap::new();
            for pair in _interp.collect_iterable(&count_items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() != 2 {
                    continue;
                }
                if let Some(t) = kv[0].as_tuple()
                    && t.len() == 2
                    && let (Some(fname), Some(lineno), Some(cnt)) =
                        (t[0].as_str(), t[1].as_int(), kv[1].as_int())
                {
                    per_file
                        .entry(fname.to_string())
                        .or_default()
                        .push((lineno, cnt));
                }
            }

            let mut sums: IndexMap<String, (i64, i64, String, String)> = IndexMap::new();
            let lc = _interp.load_module("linecache")?;
            let getlines = _interp.get_attr(&lc, "getlines")?;
            let os_path = _interp.load_module("os.path")?;
            let abspath = _interp.get_attr(&os_path, "abspath")?;
            let dirname = _interp.get_attr(&os_path, "dirname")?;
            let join_fn = _interp.get_attr(&os_path, "join")?;

            for (raw_filename, hits) in per_file {
                let ignored = call_method(
                    _interp,
                    self_val.0,
                    "is_ignored_filename",
                    &[Value::string(&raw_filename)],
                )?;
                if _interp.truthy_value(&ignored)? {
                    continue;
                }
                let filename = raw_filename
                    .strip_suffix(".pyc")
                    .map(|base| format!("{base}.py"))
                    .unwrap_or(raw_filename);

                let (dir, modulename) = match &coverdir_opt {
                    None => {
                        let abs = call_pos(_interp, abspath.clone(), &[Value::string(&filename)])?;
                        let d = call_pos(_interp, dirname.clone(), &[abs])?;
                        (_interp.render_value_as_str(&d)?, modname_rs(&filename))
                    }
                    Some(cd) => {
                        let _ = std::fs::create_dir_all(cd);
                        (cd.clone(), fullmodname_rs(_interp, &filename)?)
                    }
                };
                let lnotab = if show_miss {
                    find_executable_linenos_rs(_interp, &filename)?
                } else {
                    Value::dict(PyDict::default())
                };
                let source = call_pos(_interp, getlines.clone(), &[Value::string(&filename)])?;
                let coverpath_val = call_pos(
                    _interp,
                    join_fn.clone(),
                    &[Value::string(&dir), Value::string(format!("{modulename}.cover"))],
                )?;
                let lines_hit = Value::dict(PyDict::default());
                for (ln, cnt) in hits {
                    call_method(
                        _interp,
                        &lines_hit,
                        "__setitem__",
                        &[Value::int(ln), Value::int(cnt)],
                    )?;
                }
                let res_pair = call_method(
                    _interp,
                    self_val.0,
                    "write_results_file",
                    &[coverpath_val, source, lnotab, lines_hit, Value::string("utf-8")],
                )?;
                let res_items = _interp.collect_iterable(&res_pair)?;
                let n_hits = res_items.first().and_then(|v| v.as_int()).unwrap_or(0);
                let n_lines = res_items.get(1).and_then(|v| v.as_int()).unwrap_or(0);
                if do_summary && n_lines > 0 {
                    let percent = (100 * n_hits) / n_lines;
                    sums.insert(
                        modulename.clone(),
                        (n_lines, percent, modulename, filename),
                    );
                }
            }

            if do_summary && !sums.is_empty() {
                write_stdout(_interp, "lines   cov%   module   (path)\n")?;
                let mut keys: Vec<String> = sums.keys().cloned().collect();
                keys.sort();
                for k in keys {
                    if let Some((n_lines, percent, modulename, filename)) = sums.get(&k) {
                        write_stdout(
                            _interp,
                            &format!("{n_lines:5}   {percent:3}%   {modulename}   ({filename})\n"),
                        )?;
                    }
                }
            }

            let outfile = _interp.get_attr(self_val.0, "outfile")?;
            if _interp.truthy_value(&outfile)? {
                let path_str = _interp.render_value_as_str(&outfile)?;
                let pickle_mod = _interp.load_module("pickle")?;
                let dumps = _interp.get_attr(&pickle_mod, "dumps")?;
                let payload = Value::tuple(vec![counts, calledfuncs, callers]);
                match call_pos(_interp, dumps, &[payload, Value::int(1)]) {
                    Ok(bytes_val) => {
                        if let ValueKind::Bytes(b) = bytes_val.kind()
                            && let Err(e) = std::fs::write(&path_str, b.as_slice())
                        {
                            let _ = write_stderr(
                                _interp,
                                &format!("Can't save counts files because {e}\n"),
                            );
                        }
                    }
                    Err(e) => {
                        let _ = write_stderr(
                            _interp,
                            &format!("Can't save counts files because {e}\n"),
                        );
                    }
                }
            }
            Ok(Value::none())
        }

        fn write_results_file(
            #[positional_only] _self_val: PyValueRef<'_>,
            path: PyStr<'_>,
            lines: PyValue,
            lnotab: PyValue,
            lines_hit: PyValue,
            #[default(OptionalArg::Omitted)] _encoding: OptValueRef<'_>,
        ) -> Result<Value> {
            let mut out = String::new();
            let mut n_lines = 0i64;
            let mut n_hits = 0i64;
            for (idx, line_val) in _interp.collect_iterable(&lines.0)?.into_iter().enumerate() {
                let lineno = (idx + 1) as i64;
                let line_str = line_val.as_str().unwrap_or("");
                let has_hit = call_method(
                    _interp,
                    &lines_hit.0,
                    "__contains__",
                    &[Value::int(lineno)],
                )?;
                if _interp.truthy_value(&has_hit)? {
                    let cnt = _interp
                        .eval_index(&lines_hit.0, Value::int(lineno))?
                        .as_int()
                        .unwrap_or(0);
                    out.push_str(&format!("{cnt:5}: "));
                    n_hits += 1;
                    n_lines += 1;
                } else {
                    let in_lno = call_method(
                        _interp,
                        &lnotab.0,
                        "__contains__",
                        &[Value::int(lineno)],
                    )?;
                    if _interp.truthy_value(&in_lno)? && !line_str.contains(PRAGMA_NOCOVER) {
                        out.push_str(">>>>>> ");
                        n_lines += 1;
                    } else {
                        out.push_str("       ");
                    }
                }
                let expanded = call_method(
                    _interp,
                    &Value::string(line_str),
                    "expandtabs",
                    &[Value::int(8)],
                )?;
                out.push_str(expanded.as_str().unwrap_or(line_str));
            }
            let path_ref: &str = path.0.as_ref();
            if let Err(err) = std::fs::write(path_ref, out.as_bytes()) {
                let pr = render_value_repr(_interp, &Value::string(path_ref))?;
                let _ = write_stderr(
                    _interp,
                    &format!("trace: Could not open {pr} for writing: {err} - skipping\n"),
                );
                return Ok(Value::tuple(vec![Value::int(0), Value::int(0)]));
            }
            Ok(Value::tuple(vec![Value::int(n_hits), Value::int(n_lines)]))
        }
    }

    #[native_state(TraceTiming)]
    class Trace {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] count: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] trace: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] countfuncs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] countcallers: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] ignoremods: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] ignoredirs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] infile: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] outfile: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] timing: OptValueRef<'_>,
        ) -> Result<Value> {
            let count_on = match count.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => true,
            };
            let trace_val = trace.as_value().cloned().unwrap_or_else(|| Value::int(1));
            let trace_on = _interp.truthy_value(&trace_val)?;
            let countfuncs_on = match countfuncs.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => false,
            };
            let countcallers_on = match countcallers.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => false,
            };
            let imods = ignoremods
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::tuple(Vec::new()));
            let idirs = ignoredirs
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::tuple(Vec::new()));
            let infile_val = infile.as_value().cloned().unwrap_or_else(Value::none);
            let outfile_val = outfile.as_value().cloned().unwrap_or_else(Value::none);
            let timing_on = match timing.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => false,
            };

            let ignore_cls = trace_attr(_interp, "_Ignore")?;
            let ignore_obj = call_pos(_interp, ignore_cls, &[imods, idirs])?;

            _interp.assign_attr(self_val.0.clone(), "infile", infile_val)?;
            _interp.assign_attr(self_val.0.clone(), "outfile", outfile_val)?;
            _interp.assign_attr(self_val.0.clone(), "ignore", ignore_obj)?;
            _interp.assign_attr(self_val.0.clone(), "counts", Value::dict(PyDict::default()))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "pathtobasename",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "donothing", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "trace", trace_val)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_calledfuncs",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_callers",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_caller_cache",
                Value::dict(PyDict::default()),
            )?;

            set_state(
                self_val.0,
                TraceTiming {
                    start: timing_on.then(Instant::now),
                },
            )?;
            if timing_on {
                let time_mod = _interp.load_module("time")?;
                let mono = _interp.get_attr(&time_mod, "monotonic")?;
                let st = call_pos(_interp, mono, &[])?;
                _interp.assign_attr(self_val.0.clone(), "start_time", st)?;
            } else {
                _interp.assign_attr(self_val.0.clone(), "start_time", Value::none())?;
            }

            if countcallers_on {
                let gt = _interp.get_attr(self_val.0, "globaltrace_trackcallers")?;
                _interp.assign_attr(self_val.0.clone(), "globaltrace", gt)?;
            } else if countfuncs_on {
                let gt = _interp.get_attr(self_val.0, "globaltrace_countfuncs")?;
                _interp.assign_attr(self_val.0.clone(), "globaltrace", gt)?;
            } else if trace_on && count_on {
                let gt = _interp.get_attr(self_val.0, "globaltrace_lt")?;
                let lt = _interp.get_attr(self_val.0, "localtrace_trace_and_count")?;
                _interp.assign_attr(self_val.0.clone(), "globaltrace", gt)?;
                _interp.assign_attr(self_val.0.clone(), "localtrace", lt)?;
            } else if trace_on {
                let gt = _interp.get_attr(self_val.0, "globaltrace_lt")?;
                let lt = _interp.get_attr(self_val.0, "localtrace_trace")?;
                _interp.assign_attr(self_val.0.clone(), "globaltrace", gt)?;
                _interp.assign_attr(self_val.0.clone(), "localtrace", lt)?;
            } else if count_on {
                let gt = _interp.get_attr(self_val.0, "globaltrace_lt")?;
                let lt = _interp.get_attr(self_val.0, "localtrace_count")?;
                _interp.assign_attr(self_val.0.clone(), "globaltrace", gt)?;
                _interp.assign_attr(self_val.0.clone(), "localtrace", lt)?;
            } else {
                _interp.assign_attr(self_val.0.clone(), "donothing", Value::int(1))?;
            }
            Ok(Value::none())
        }

        fn run(#[positional_only] self_val: PyValueRef<'_>, cmd: PyValue) -> Result<Value> {
            let main_mod = _interp.load_module("__main__")?;
            let dict = _interp.get_attr(&main_mod, "__dict__")?;
            call_method(_interp, self_val.0, "runctx", &[cmd.0, dict.clone(), dict])
        }

        fn runctx(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyValue,
            #[default(OptionalArg::Omitted)] globals: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] locals: OptValueRef<'_>,
        ) -> Result<Value> {
            let g = match globals.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => Value::dict(PyDict::default()),
            };
            let l = match locals.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => Value::dict(PyDict::default()),
            };
            let donothing = _interp.get_attr(self_val.0, "donothing")?;
            let active = !_interp.truthy_value(&donothing)?;
            if active {
                let gt = _interp.get_attr(self_val.0, "globaltrace")?;
                set_sys_and_threading_trace(_interp, gt, true)?;
            }
            let res = call_pos(
                _interp,
                Value::builtin_function("exec"),
                &[cmd.0, g, l],
            );
            if active {
                let _ = set_sys_and_threading_trace(_interp, Value::none(), true);
            }
            res?;
            Ok(Value::none())
        }

        fn runfunc(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] func: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let donothing = _interp.get_attr(self_val.0, "donothing")?;
            let active = !_interp.truthy_value(&donothing)?;
            if active {
                let gt = _interp.get_attr(self_val.0, "globaltrace")?;
                set_sys_and_threading_trace(_interp, gt, false)?;
            }
            let mut expanded: Vec<ExpandedCallArg> = args
                .iter()
                .cloned()
                .map(|value| ExpandedCallArg { name: None, value })
                .collect();
            for (k, v) in kw {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            let res = _interp.call_function_expanded(func.0, &expanded);
            if active {
                let _ = set_sys_and_threading_trace(_interp, Value::none(), false);
            }
            res
        }

        fn file_module_function_of(
            #[positional_only] _self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            let code = _interp.get_attr(&frame.0, "f_code")?;
            let filename_val = _interp.get_attr(&code, "co_filename")?;
            let modulename = match filename_val.as_str() {
                Some(s) if !s.is_empty() => Value::string(modname_rs(s)),
                _ => Value::none(),
            };
            let co_name = _interp
                .get_attr(&code, "co_name")?
                .as_str()
                .unwrap_or("")
                .to_string();
            let qualname = _interp
                .get_attr(&code, "co_qualname")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| co_name.clone());
            // Match CPython's `ClassName.method` formatting when the function
            // is a class method without `<locals>` nesting.
            let funcname = if !qualname.contains("<locals>")
                && qualname != co_name
                && qualname.ends_with(&format!(".{co_name}"))
            {
                let prefix = &qualname[..qualname.len() - co_name.len() - 1];
                let cls = prefix.rsplit('.').next().unwrap_or(prefix);
                format!("{cls}.{co_name}")
            } else {
                co_name
            };
            Ok(Value::tuple(vec![
                filename_val,
                modulename,
                Value::string(funcname),
            ]))
        }

        fn globaltrace_trackcallers(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            why: PyStr<'_>,
            _arg: PyValue,
        ) -> Result<Value> {
            if why.0 == "call" {
                let this_func = call_method(
                    _interp,
                    self_val.0,
                    "file_module_function_of",
                    std::slice::from_ref(&frame.0),
                )?;
                let f_back = _interp.get_attr(&frame.0, "f_back")?;
                if !f_back.is_none() {
                    let parent_func =
                        call_method(_interp, self_val.0, "file_module_function_of", &[f_back])?;
                    let callers = _interp.get_attr(self_val.0, "_callers")?;
                    call_method(
                        _interp,
                        &callers,
                        "__setitem__",
                        &[Value::tuple(vec![parent_func, this_func]), Value::int(1)],
                    )?;
                }
            }
            Ok(Value::none())
        }

        fn globaltrace_countfuncs(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            why: PyStr<'_>,
            _arg: PyValue,
        ) -> Result<Value> {
            if why.0 == "call" {
                let this_func =
                    call_method(_interp, self_val.0, "file_module_function_of", &[frame.0])?;
                let calledfuncs = _interp.get_attr(self_val.0, "_calledfuncs")?;
                call_method(_interp, &calledfuncs, "__setitem__", &[this_func, Value::int(1)])?;
            }
            Ok(Value::none())
        }

        fn globaltrace_lt(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            why: PyStr<'_>,
            _arg: PyValue,
        ) -> Result<Value> {
            if why.0 == "call" {
                let code = _interp.get_attr(&frame.0, "f_code")?;
                let f_globals = _interp.get_attr(&frame.0, "f_globals")?;
                let filename = call_method(
                    _interp,
                    &f_globals,
                    "get",
                    &[Value::string("__file__"), Value::none()],
                )?;
                if let Some(fname_str) = filename.as_str()
                    && !fname_str.is_empty()
                {
                    let modulename = modname_rs(fname_str);
                    let ignore = _interp.get_attr(self_val.0, "ignore")?;
                    let ignore_it = call_method(
                        _interp,
                        &ignore,
                        "names",
                        &[filename, Value::string(&modulename)],
                    )?;
                    if !_interp.truthy_value(&ignore_it)? {
                        let trace_flag = _interp.get_attr(self_val.0, "trace")?;
                        if _interp.truthy_value(&trace_flag)? {
                            let co_name = _interp.get_attr(&code, "co_name")?;
                            let co_name_s = _interp.render_value_as_str(&co_name)?;
                            write_stdout(
                                _interp,
                                &format!(" --- modulename: {modulename}, funcname: {co_name_s}\n"),
                            )?;
                        }
                        return _interp.get_attr(self_val.0, "localtrace");
                    }
                }
            }
            Ok(Value::none())
        }

        fn localtrace_trace_and_count(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            why: PyStr<'_>,
            _arg: PyValue,
        ) -> Result<Value> {
            if why.0 == "line" {
                let code = _interp.get_attr(&frame.0, "f_code")?;
                let filename = _interp.get_attr(&code, "co_filename")?;
                let lineno = _interp.get_attr(&frame.0, "f_lineno")?;
                let key = Value::tuple(vec![filename.clone(), lineno.clone()]);
                let counts = _interp.get_attr(self_val.0, "counts")?;
                let prev = call_method(_interp, &counts, "get", &[key.clone(), Value::int(0)])?;
                let next_cnt = prev.as_int().unwrap_or(0) + 1;
                call_method(_interp, &counts, "__setitem__", &[key, Value::int(next_cnt)])?;

                let timing_elapsed = with_state(self_val.0, |st| {
                    Ok(st.start.map(|s| s.elapsed().as_secs_f64()))
                })?;
                if let Some(secs) = timing_elapsed {
                    write_stdout(_interp, &format!("{secs:.2} "))?;
                }
                let fname_s = filename.as_str().unwrap_or("");
                let bname = Path::new(fname_s)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(fname_s)
                    .to_string();
                let ln = lineno.as_int().unwrap_or(0);
                let lc = _interp.load_module("linecache")?;
                let getline = _interp.get_attr(&lc, "getline")?;
                let line_val = call_pos(_interp, getline, &[filename, lineno])?;
                let line_s = line_val.as_str().unwrap_or("");
                if !line_s.is_empty() {
                    write_stdout(_interp, &format!("{bname}({ln}): {line_s}"))?;
                } else {
                    write_stdout(_interp, &format!("{bname}({ln})\n"))?;
                }
            }
            _interp.get_attr(self_val.0, "localtrace")
        }

        fn localtrace_trace(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            why: PyStr<'_>,
            _arg: PyValue,
        ) -> Result<Value> {
            if why.0 == "line" {
                let code = _interp.get_attr(&frame.0, "f_code")?;
                let filename = _interp.get_attr(&code, "co_filename")?;
                let lineno = _interp.get_attr(&frame.0, "f_lineno")?;

                let timing_elapsed = with_state(self_val.0, |st| {
                    Ok(st.start.map(|s| s.elapsed().as_secs_f64()))
                })?;
                if let Some(secs) = timing_elapsed {
                    write_stdout(_interp, &format!("{secs:.2} "))?;
                }
                let fname_s = filename.as_str().unwrap_or("");
                let bname = Path::new(fname_s)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(fname_s)
                    .to_string();
                let ln = lineno.as_int().unwrap_or(0);
                let lc = _interp.load_module("linecache")?;
                let getline = _interp.get_attr(&lc, "getline")?;
                let line_val = call_pos(_interp, getline, &[filename, lineno])?;
                let line_s = line_val.as_str().unwrap_or("");
                if !line_s.is_empty() {
                    write_stdout(_interp, &format!("{bname}({ln}): {line_s}"))?;
                } else {
                    write_stdout(_interp, &format!("{bname}({ln})\n"))?;
                }
            }
            _interp.get_attr(self_val.0, "localtrace")
        }

        fn localtrace_count(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            why: PyStr<'_>,
            _arg: PyValue,
        ) -> Result<Value> {
            if why.0 == "line" {
                let code = _interp.get_attr(&frame.0, "f_code")?;
                let filename = _interp.get_attr(&code, "co_filename")?;
                let lineno = _interp.get_attr(&frame.0, "f_lineno")?;
                let key = Value::tuple(vec![filename, lineno]);
                let counts = _interp.get_attr(self_val.0, "counts")?;
                let prev = call_method(_interp, &counts, "get", &[key.clone(), Value::int(0)])?;
                let next_cnt = prev.as_int().unwrap_or(0) + 1;
                call_method(_interp, &counts, "__setitem__", &[key, Value::int(next_cnt)])?;
            }
            _interp.get_attr(self_val.0, "localtrace")
        }

        fn results(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let counts = _interp.get_attr(self_val.0, "counts")?;
            let infile = _interp.get_attr(self_val.0, "infile")?;
            let outfile = _interp.get_attr(self_val.0, "outfile")?;
            let calledfuncs = _interp.get_attr(self_val.0, "_calledfuncs")?;
            let callers = _interp.get_attr(self_val.0, "_callers")?;
            let cr_cls = trace_attr(_interp, "CoverageResults")?;
            call_kw(
                _interp,
                cr_cls,
                &[counts],
                &[
                    ("infile", infile),
                    ("outfile", outfile),
                    ("calledfuncs", calledfuncs),
                    ("callers", callers),
                ],
            )
        }
    }
}
