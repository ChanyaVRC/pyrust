// `modulefinder` module — 100% native Rust implementation of Python 3.12's
// `modulefinder` module (issue #3249).
//
// Analyzes Python script and module import dependencies by parsing source via
// PyRust's `ast` parser, building `Module` descriptors, tracking `modules` and
// `badmodules`, and reporting missing / possibly-missing imports.
//
// Reference: <https://docs.python.org/3.12/library/modulefinder.html>

use std::path::{Path, PathBuf};

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::{PyIndex, PyPath, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn write_stdout(interp: &mut Interpreter, text: &str) {
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(stdout) = interp.get_attr(&sys_mod, "stdout")
        && !stdout.is_none()
        && let Ok(write_fn) = interp.get_attr(&stdout, "write")
    {
        let _ = call_pos(interp, write_fn, &[Value::string(text)]);
    }
}

fn ast_class_name(val: &Value) -> Option<String> {
    match val.kind() {
        ValueKind::PyInstance(inst) => Some(inst.borrow().class.borrow().name.clone()),
        _ => None,
    }
}

fn ast_field(val: &Value, field: &str) -> Option<Value> {
    match val.kind() {
        ValueKind::PyInstance(inst) => inst.borrow().attrs.get(field).cloned(),
        _ => None,
    }
}

fn ast_str(val: &Value, field: &str) -> Option<String> {
    ast_field(val, field).and_then(|v| v.as_str().map(str::to_owned))
}

fn ast_int(val: &Value, field: &str) -> i64 {
    ast_field(val, field)
        .and_then(|v| match v.kind() {
            ValueKind::Int(n) => Some(n),
            _ => None,
        })
        .unwrap_or(0)
}

fn ast_list(val: &Value, field: &str) -> Vec<Value> {
    ast_field(val, field)
        .map(|v| match v.kind() {
            ValueKind::List(items) => items.clone(),
            ValueKind::Tuple(items) => items.to_vec(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

struct ScannedImport {
    name: String,
    fromlist: Option<Vec<String>>,
    level: i64,
}

struct ScannedModuleAst {
    imports: Vec<ScannedImport>,
    globals: Vec<String>,
    star_imports: Vec<String>,
}

fn collect_target_names(node: &Value, out: &mut Vec<String>) {
    let Some(kind) = ast_class_name(node) else {
        return;
    };
    match kind.as_str() {
        "Name" => {
            if let Some(id) = ast_str(node, "id") {
                out.push(id);
            }
        }
        "Tuple" | "List" => {
            for elt in ast_list(node, "elts") {
                collect_target_names(&elt, out);
            }
        }
        "Starred" => {
            if let Some(v) = ast_field(node, "value") {
                collect_target_names(&v, out);
            }
        }
        _ => {}
    }
}

fn scan_stmts(stmts: &[Value], top_level: bool, out: &mut ScannedModuleAst) {
    for stmt in stmts {
        let Some(kind) = ast_class_name(stmt) else {
            continue;
        };
        match kind.as_str() {
            "Import" => {
                for alias in ast_list(stmt, "names") {
                    if let Some(name) = ast_str(&alias, "name") {
                        out.imports.push(ScannedImport {
                            name: name.clone(),
                            fromlist: None,
                            level: 0,
                        });
                        if top_level {
                            let bound = ast_str(&alias, "asname").unwrap_or_else(|| {
                                name.split('.').next().unwrap_or(&name).to_string()
                            });
                            out.globals.push(bound);
                        }
                    }
                }
            }
            "ImportFrom" => {
                let module = ast_str(stmt, "module").unwrap_or_default();
                let level = ast_int(stmt, "level");
                let mut froms = Vec::new();
                for alias in ast_list(stmt, "names") {
                    if let Some(name) = ast_str(&alias, "name") {
                        if name == "*" {
                            out.star_imports.push(module.clone());
                        } else if top_level {
                            let bound = ast_str(&alias, "asname").unwrap_or_else(|| name.clone());
                            out.globals.push(bound);
                        }
                        froms.push(name);
                    }
                }
                out.imports.push(ScannedImport {
                    name: module,
                    fromlist: Some(froms),
                    level,
                });
            }
            "FunctionDef" | "AsyncFunctionDef" => {
                if top_level && let Some(name) = ast_str(stmt, "name") {
                    out.globals.push(name);
                }
                scan_stmts(&ast_list(stmt, "body"), false, out);
            }
            "ClassDef" => {
                if top_level && let Some(name) = ast_str(stmt, "name") {
                    out.globals.push(name);
                }
                scan_stmts(&ast_list(stmt, "body"), false, out);
            }
            "Assign" => {
                if top_level {
                    for t in ast_list(stmt, "targets") {
                        collect_target_names(&t, &mut out.globals);
                    }
                }
            }
            "AnnAssign" => {
                if top_level && let Some(t) = ast_field(stmt, "target") {
                    collect_target_names(&t, &mut out.globals);
                }
            }
            "If" | "While" | "For" | "AsyncFor" => {
                scan_stmts(&ast_list(stmt, "body"), top_level, out);
                scan_stmts(&ast_list(stmt, "orelse"), top_level, out);
            }
            "With" | "AsyncWith" => {
                scan_stmts(&ast_list(stmt, "body"), top_level, out);
            }
            "Try" | "TryStar" => {
                scan_stmts(&ast_list(stmt, "body"), top_level, out);
                for h in ast_list(stmt, "handlers") {
                    scan_stmts(&ast_list(&h, "body"), top_level, out);
                }
                scan_stmts(&ast_list(stmt, "orelse"), top_level, out);
                scan_stmts(&ast_list(stmt, "finalbody"), top_level, out);
            }
            "Match" => {
                for case in ast_list(stmt, "cases") {
                    scan_stmts(&ast_list(&case, "body"), top_level, out);
                }
            }
            _ => {}
        }
    }
}

fn scan_source_ast(
    interp: &mut Interpreter,
    source: &str,
    filename: &str,
) -> Result<ScannedModuleAst> {
    let ast_mod = interp.load_module("ast")?;
    let parse_fn = interp.get_attr(&ast_mod, "parse")?;
    let tree = interp.call_function_expanded(
        parse_fn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string(source),
            },
            ExpandedCallArg {
                name: Some("filename".to_string()),
                value: Value::string(filename),
            },
        ],
    )?;
    let mut out = ScannedModuleAst {
        imports: Vec::new(),
        globals: Vec::new(),
        star_imports: Vec::new(),
    };
    scan_stmts(&ast_list(&tree, "body"), true, &mut out);
    Ok(out)
}

enum FoundModule {
    Builtin,
    Package { init_file: String, pkg_dir: String },
    SourceFile { file: String },
}

fn find_on_search_paths(
    name: &str,
    search_paths: &[String],
    is_top_level: bool,
) -> Option<FoundModule> {
    if is_top_level && crate::builtin_modules::is_builtin_module(name) {
        return Some(FoundModule::Builtin);
    }
    let tail = name.rsplit('.').next().unwrap_or(name);
    for dir in search_paths {
        let base = if dir.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(dir)
        };
        let pkg_dir = base.join(tail);
        let init_py = pkg_dir.join("__init__.py");
        if init_py.is_file() {
            return Some(FoundModule::Package {
                init_file: init_py.to_string_lossy().into_owned(),
                pkg_dir: pkg_dir.to_string_lossy().into_owned(),
            });
        }
        let mod_py = base.join(format!("{tail}.py"));
        if mod_py.is_file() {
            return Some(FoundModule::SourceFile {
                file: mod_py.to_string_lossy().into_owned(),
            });
        }
    }
    if crate::builtin_modules::is_builtin_module(name) {
        return Some(FoundModule::Builtin);
    }
    None
}

fn mf_get_modules_dict(interp: &mut Interpreter, self_val: &Value) -> Result<Value> {
    interp.get_attr(self_val, "modules")
}

fn mf_get_badmodules_dict(interp: &mut Interpreter, self_val: &Value) -> Result<Value> {
    interp.get_attr(self_val, "badmodules")
}

fn mf_get_excludes(interp: &mut Interpreter, self_val: &Value) -> Result<Vec<String>> {
    let exc_val = interp.get_attr(self_val, "excludes")?;
    let items = interp.collect_iterable(&exc_val)?;
    Ok(items
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect())
}

fn mf_get_paths(interp: &mut Interpreter, self_val: &Value) -> Result<Vec<String>> {
    let p_val = interp.get_attr(self_val, "path")?;
    let items = interp.collect_iterable(&p_val)?;
    Ok(items
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect())
}

fn mf_add_badmodule(
    interp: &mut Interpreter,
    self_val: &Value,
    name: &str,
    caller_name: Option<&str>,
) -> Result<()> {
    let bad = mf_get_badmodules_dict(interp, self_val)?;
    let key = PyKey::str_from(name);
    let existing = bad.dict_with(|d| d.get(&key).cloned()).flatten();
    let callers_dict = match existing {
        Some(d) => d,
        None => {
            let d = Value::dict(PyDict::default());
            bad.dict_insert(key, d.clone())?;
            d
        }
    };
    if let Some(c) = caller_name {
        callers_dict.dict_insert(PyKey::str_from(c), Value::int(1))?;
    }
    Ok(())
}

fn mf_add_or_get_module(
    interp: &mut Interpreter,
    self_val: &Value,
    fqname: &str,
    file: Option<&str>,
    path: Option<Vec<String>>,
) -> Result<(Value, bool)> {
    let modules = mf_get_modules_dict(interp, self_val)?;
    let key = PyKey::str_from(fqname);
    if let Some(existing) = modules.dict_with(|d| d.get(&key).cloned()).flatten() {
        return Ok((existing, false));
    }
    let mf_mod = interp.load_module("modulefinder")?;
    let mod_cls = interp.get_attr(&mf_mod, "Module")?;
    let file_val = file.map(Value::string).unwrap_or_else(Value::none);
    let path_val = path
        .map(|ps| Value::list(ps.into_iter().map(Value::string).collect()))
        .unwrap_or_else(Value::none);
    let m = call_pos(
        interp,
        mod_cls,
        &[Value::string(fqname), file_val, path_val],
    )?;
    modules.dict_insert(key, m.clone())?;
    Ok((m, true))
}

fn mf_load_and_scan_file(
    interp: &mut Interpreter,
    self_val: &Value,
    fqname: &str,
    file_path: &str,
    pkg_dir: Option<String>,
) -> Result<Value> {
    let (mod_obj, _is_new) = mf_add_or_get_module(
        interp,
        self_val,
        fqname,
        Some(file_path),
        pkg_dir.map(|d| vec![d]),
    )?;
    // Update __file__ if it was None.
    let _ = interp.assign_attr(mod_obj.clone(), "__file__", Value::string(file_path));

    let src = match std::fs::read_to_string(file_path) {
        Ok(s) => s,
        Err(_) => return Ok(mod_obj),
    };
    // Compile code object for `__code__`.
    if let Ok(code) = interp.call_function_expanded(
        Value::builtin_function("compile"),
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string(&src),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string(file_path),
            },
            ExpandedCallArg {
                name: None,
                value: Value::string("exec"),
            },
            ExpandedCallArg {
                name: Some("dont_inherit".to_string()),
                value: Value::bool_(true),
            },
        ],
    ) {
        let _ = interp.assign_attr(mod_obj.clone(), "__code__", code);
    }

    let Ok(scanned) = scan_source_ast(interp, &src, file_path) else {
        return Ok(mod_obj);
    };

    if let Ok(gdict) = interp.get_attr(&mod_obj, "globalnames") {
        for g in &scanned.globals {
            let _ = gdict.dict_insert(PyKey::str_from(g), Value::int(1));
        }
    }
    if let Ok(sdict) = interp.get_attr(&mod_obj, "starimports") {
        for s in &scanned.star_imports {
            let _ = sdict.dict_insert(PyKey::str_from(s), Value::int(1));
        }
    }

    for imp in scanned.imports {
        let _ = mf_import_hook_impl(
            interp,
            self_val,
            &imp.name,
            Some(&mod_obj),
            imp.fromlist.as_deref(),
            imp.level,
        );
    }
    Ok(mod_obj)
}

fn resolve_relative_base(
    caller: Option<&Value>,
    interp: &mut Interpreter,
    level: i64,
) -> Option<String> {
    if level <= 0 {
        return None;
    }
    let caller_val = caller?;
    let caller_name = interp
        .get_attr(caller_val, "__name__")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))?;
    let is_pkg = interp
        .get_attr(caller_val, "__path__")
        .ok()
        .is_some_and(|v| !v.is_none());
    let mut parts: Vec<&str> = caller_name.split('.').collect();
    let pop_count = if is_pkg {
        (level - 1) as usize
    } else {
        level as usize
    };
    if pop_count > parts.len() {
        return None;
    }
    parts.truncate(parts.len() - pop_count);
    Some(parts.join("."))
}

fn mf_load_dotted_module(
    interp: &mut Interpreter,
    self_val: &Value,
    fqname: &str,
    caller_name: Option<&str>,
) -> Result<Option<Value>> {
    if fqname.is_empty() {
        return Ok(None);
    }
    let excludes = mf_get_excludes(interp, self_val)?;
    if excludes.iter().any(|e| e == fqname) {
        mf_add_badmodule(interp, self_val, fqname, caller_name)?;
        return Ok(None);
    }
    let modules = mf_get_modules_dict(interp, self_val)?;
    if let Some(existing) = modules
        .dict_with(|d| d.get(&PyKey::str_from(fqname)).cloned())
        .flatten()
    {
        return Ok(Some(existing));
    }

    let parts: Vec<&str> = fqname.split('.').collect();
    let mut current_name = String::new();
    let mut search_paths = mf_get_paths(interp, self_val)?;
    let mut last_mod: Option<Value> = None;

    for (idx, part) in parts.iter().enumerate() {
        if idx == 0 {
            current_name = (*part).to_string();
        } else {
            current_name.push('.');
            current_name.push_str(part);
        }

        if excludes.iter().any(|e| e == &current_name) {
            mf_add_badmodule(interp, self_val, &current_name, caller_name)?;
            return Ok(None);
        }

        if let Some(existing) = modules
            .dict_with(|d| d.get(&PyKey::str_from(&current_name)).cloned())
            .flatten()
        {
            if let Ok(p) = interp.get_attr(&existing, "__path__")
                && !p.is_none()
                && let Ok(items) = interp.collect_iterable(&p)
            {
                search_paths = items
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
            } else {
                search_paths = Vec::new();
            }
            last_mod = Some(existing);
            continue;
        }

        match find_on_search_paths(&current_name, &search_paths, idx == 0) {
            Some(FoundModule::Builtin) => {
                let (m, _) = mf_add_or_get_module(interp, self_val, &current_name, None, None)?;
                search_paths = Vec::new();
                last_mod = Some(m);
            }
            Some(FoundModule::Package { init_file, pkg_dir }) => {
                let m = mf_load_and_scan_file(
                    interp,
                    self_val,
                    &current_name,
                    &init_file,
                    Some(pkg_dir.clone()),
                )?;
                search_paths = vec![pkg_dir];
                last_mod = Some(m);
            }
            Some(FoundModule::SourceFile { file }) => {
                let m = mf_load_and_scan_file(interp, self_val, &current_name, &file, None)?;
                search_paths = Vec::new();
                last_mod = Some(m);
            }
            None => {
                mf_add_badmodule(interp, self_val, &current_name, caller_name)?;
                return Ok(None);
            }
        }
    }

    Ok(last_mod)
}

fn mf_import_hook_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    name: &str,
    caller: Option<&Value>,
    fromlist: Option<&[String]>,
    level: i64,
) -> Result<()> {
    let caller_name = caller.and_then(|c| {
        interp
            .get_attr(c, "__name__")
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
    });

    let target_name = if level > 0 {
        match resolve_relative_base(caller, interp, level) {
            Some(base) if !base.is_empty() && !name.is_empty() => format!("{base}.{name}"),
            Some(base) if !base.is_empty() => base,
            _ => name.to_string(),
        }
    } else {
        name.to_string()
    };

    let loaded = mf_load_dotted_module(interp, self_val, &target_name, caller_name.as_deref())?;

    if let Some(items) = fromlist {
        let Some(pkg_mod) = loaded else {
            return Ok(());
        };
        let is_pkg = interp
            .get_attr(&pkg_mod, "__path__")
            .ok()
            .is_some_and(|v| !v.is_none());
        let pkg_globals = interp.get_attr(&pkg_mod, "globalnames").ok();
        for sub in items {
            if sub == "*" {
                continue;
            }
            let already_global = pkg_globals
                .as_ref()
                .and_then(|gd| gd.dict_with(|d| d.contains_key(&PyKey::str_from(sub))))
                .unwrap_or(false);
            if already_global {
                continue;
            }
            if is_pkg {
                let sub_fq = format!("{target_name}.{sub}");
                let _ = mf_load_dotted_module(interp, self_val, &sub_fq, caller_name.as_deref())?;
            }
        }
    }
    Ok(())
}

fn compute_missing_maybe(
    interp: &mut Interpreter,
    self_val: &Value,
) -> Result<(Vec<String>, Vec<String>)> {
    let bad = mf_get_badmodules_dict(interp, self_val)?;
    let modules = mf_get_modules_dict(interp, self_val)?;
    let excludes = mf_get_excludes(interp, self_val)?;

    let mut bad_names: Vec<String> = bad
        .dict_with(|d| {
            d.keys()
                .filter_map(|k| match k {
                    PyKey::Str(s) => Some(s.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    bad_names.sort();

    let mut missing = Vec::new();
    let mut maybe = Vec::new();

    for name in bad_names {
        if excludes.iter().any(|e| e == &name) {
            continue;
        }
        let in_modules = modules
            .dict_with(|d| d.contains_key(&PyKey::str_from(&name)))
            .unwrap_or(false);
        if in_modules {
            continue;
        }
        if let Some((pkgname, subname)) = name.rsplit_once('.')
            && let Some(pkg_mod) = modules
                .dict_with(|d| d.get(&PyKey::str_from(pkgname)).cloned())
                .flatten()
        {
            let in_globals = interp
                .get_attr(&pkg_mod, "globalnames")
                .ok()
                .and_then(|gd| gd.dict_with(|d| d.contains_key(&PyKey::str_from(subname))))
                .unwrap_or(false);
            if in_globals {
                continue;
            }
            let has_star = interp
                .get_attr(&pkg_mod, "starimports")
                .ok()
                .and_then(|sd| sd.dict_with(|d| !d.is_empty()))
                .unwrap_or(false);
            if has_star {
                maybe.push(name);
                continue;
            }
        }
        missing.push(name);
    }

    missing.sort();
    maybe.sort();
    Ok((missing, maybe))
}

pyrust_module! {
    constants {
        "packagePathMap" => Value::dict(PyDict::default()),
        "replacePackageMap" => Value::dict(PyDict::default()),
    }

    fn AddPackagePath(packagename: PyStr<'_>, path: PyStr<'_>) -> Result<Value> {
        let m = _interp.load_module("modulefinder")?;
        let map = _interp.get_attr(&m, "packagePathMap")?;
        let key = PyKey::str_from(&packagename.0);
        let existing = map.dict_with(|d| d.get(&key).cloned()).flatten();
        match existing {
            Some(list) => {
                list.list_push(Value::string(&path.0))?;
            }
            None => {
                map.dict_insert(key, Value::list(vec![Value::string(&path.0)]))?;
            }
        }
        Ok(Value::none())
    }

    fn ReplacePackage(oldname: PyStr<'_>, newname: PyStr<'_>) -> Result<Value> {
        let m = _interp.load_module("modulefinder")?;
        let map = _interp.get_attr(&m, "replacePackageMap")?;
        map.dict_insert(PyKey::str_from(&oldname.0), Value::string(&newname.0))?;
        Ok(Value::none())
    }

    class Module {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[default(None)] file: Option<PyValue>,
            #[default(None)] path: Option<PyValue>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "__name__", Value::string(name.0))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "__file__",
                file.map(|v| v.0).unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "__path__",
                path.map(|v| v.0).unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(self_val.0.clone(), "__code__", Value::none())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "globalnames",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "starimports",
                Value::dict(PyDict::default()),
            )?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let name = _interp
                .get_attr(self_val.0, "__name__")
                .unwrap_or_else(|_| Value::none());
            let file = _interp
                .get_attr(self_val.0, "__file__")
                .unwrap_or_else(|_| Value::none());
            let path = _interp
                .get_attr(self_val.0, "__path__")
                .unwrap_or_else(|_| Value::none());
            let mut s = format!("Module({}", name.repr_raw());
            if !file.is_none() {
                s.push_str(", ");
                s.push_str(&file.repr_raw());
            }
            if !path.is_none() {
                s.push_str(", ");
                s.push_str(&path.repr_raw());
            }
            s.push(')');
            Ok(Value::string(s))
        }
    }

    class ModuleFinder {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(None)] path: Option<PyValue>,
            #[default(PyIndex::from(0i64))] debug: PyIndex,
            #[default(None)] excludes: Option<PyValue>,
            #[default(None)] replace_paths: Option<PyValue>,
        ) -> Result<Value> {
            let path_val = match path {
                Some(p) if !p.0.is_none() => Value::list(_interp.collect_iterable(&p.0)?),
                _ => {
                    let sys_mod = _interp.load_module("sys")?;
                    let sp = _interp.get_attr(&sys_mod, "path")?;
                    Value::list(_interp.collect_iterable(&sp)?)
                }
            };
            let exc_val = match excludes {
                Some(e) if !e.0.is_none() => Value::list(_interp.collect_iterable(&e.0)?),
                _ => Value::list(Vec::new()),
            };
            let rep_val = match replace_paths {
                Some(r) if !r.0.is_none() => Value::list(_interp.collect_iterable(&r.0)?),
                _ => Value::list(Vec::new()),
            };
            let dbg = debug.expect_i64("ModuleFinder", "debug")?;

            _interp.assign_attr(self_val.0.clone(), "path", path_val)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "modules",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "badmodules",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "debug", Value::int(dbg))?;
            _interp.assign_attr(self_val.0.clone(), "indent", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "excludes", exc_val)?;
            _interp.assign_attr(self_val.0.clone(), "replace_paths", rep_val)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "processed_paths",
                Value::list(Vec::new()),
            )?;
            Ok(Value::none())
        }

        fn add_module(
            #[positional_only] self_val: PyValueRef<'_>,
            fqname: PyStr<'_>,
        ) -> Result<Value> {
            let (m, _) = mf_add_or_get_module(_interp, self_val.0, &fqname.0, None, None)?;
            Ok(m)
        }

        fn run_script(
            #[positional_only] self_val: PyValueRef<'_>,
            pathname: PyPath,
        ) -> Result<Value> {
            let path_str = pathname.as_path().to_string_lossy().into_owned();
            // Ensure script's directory is searched when scanning script imports.
            let orig_path = _interp.get_attr(self_val.0, "path")?;
            let script_dir = Path::new(&path_str)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut tmp_paths = vec![Value::string(script_dir)];
            tmp_paths.extend(_interp.collect_iterable(&orig_path)?);
            let _ = _interp.assign_attr(self_val.0.clone(), "path", Value::list(tmp_paths));
            let res = mf_load_and_scan_file(_interp, self_val.0, "__main__", &path_str, None);
            let _ = _interp.assign_attr(self_val.0.clone(), "path", orig_path);
            res?;
            Ok(Value::none())
        }

        fn load_file(
            #[positional_only] self_val: PyValueRef<'_>,
            pathname: PyPath,
        ) -> Result<Value> {
            let path_str = pathname.as_path().to_string_lossy().into_owned();
            let stem = Path::new(&path_str)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("__main__")
                .to_string();
            mf_load_and_scan_file(_interp, self_val.0, &stem, &path_str, None)?;
            Ok(Value::none())
        }

        fn import_hook(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[default(None)] caller: Option<PyValue>,
            #[default(None)] fromlist: Option<PyValue>,
            #[default(PyIndex::from(i64::from(-1)))] level: PyIndex,
        ) -> Result<Value> {
            let lv = level.expect_i64("import_hook", "level")?;
            let froms: Option<Vec<String>> = match fromlist {
                Some(v) if !v.0.is_none() => Some(
                    _interp
                        .collect_iterable(&v.0)?
                        .into_iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect(),
                ),
                _ => None,
            };
            mf_import_hook_impl(
                _interp,
                self_val.0,
                &name.0,
                caller.as_ref().map(|v| &v.0),
                froms.as_deref(),
                lv,
            )?;
            Ok(Value::none())
        }

        fn any_missing(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (missing, maybe) = compute_missing_maybe(_interp, self_val.0)?;
            let mut all = missing;
            all.extend(maybe);
            all.sort();
            all.dedup();
            Ok(Value::list(all.into_iter().map(Value::string).collect()))
        }

        fn any_missing_maybe(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (missing, maybe) = compute_missing_maybe(_interp, self_val.0)?;
            Ok(Value::tuple(vec![
                Value::list(missing.into_iter().map(Value::string).collect()),
                Value::list(maybe.into_iter().map(Value::string).collect()),
            ]))
        }

        fn report(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            write_stdout(_interp, "\n");
            write_stdout(_interp, &format!("  {:<25} {}\n", "Name", "File"));
            write_stdout(_interp, &format!("  {:<25} {}\n", "----", "----"));
            let modules = mf_get_modules_dict(_interp, self_val.0)?;
            let mut entries: Vec<(String, Value)> = modules
                .dict_with(|d| {
                    d.iter()
                        .filter_map(|(k, v)| match k {
                            PyKey::Str(s) => Some((s.to_string(), v.clone())),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            entries.sort_by(|a, b| a.0.cmp(&b.0));

            for (name, m) in entries {
                let is_pkg = _interp
                    .get_attr(&m, "__path__")
                    .ok()
                    .is_some_and(|v| !v.is_none());
                let kind = if is_pkg { "P" } else { "m" };
                let file = _interp
                    .get_attr(&m, "__file__")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                write_stdout(_interp, &format!("{kind} {name:<25} {file}\n"));
            }

            let (missing, maybe) = compute_missing_maybe(_interp, self_val.0)?;
            let bad = mf_get_badmodules_dict(_interp, self_val.0)?;
            if !missing.is_empty() {
                write_stdout(_interp, "\nMissing modules:\n");
                for name in &missing {
                    let callers = bad
                        .dict_with(|d| d.get(&PyKey::str_from(name)).cloned())
                        .flatten()
                        .and_then(|cd| {
                            cd.dict_with(|d| {
                                let mut ks: Vec<String> = d
                                    .keys()
                                    .filter_map(|k| match k {
                                        PyKey::Str(s) => Some(s.to_string()),
                                        _ => None,
                                    })
                                    .collect();
                                ks.sort();
                                ks
                            })
                        })
                        .unwrap_or_default();
                    write_stdout(
                        _interp,
                        &format!("? {name} imported from {}\n", callers.join(", ")),
                    );
                }
            }
            if !maybe.is_empty() {
                write_stdout(
                    _interp,
                    "\nSubmodules that appear to be missing, but could also be global names in the parent package:\n",
                );
                for name in &maybe {
                    write_stdout(_interp, &format!("? {name}\n"));
                }
            }
            Ok(Value::none())
        }

        fn msg(
            #[positional_only] self_val: PyValueRef<'_>,
            level: PyIndex,
            s: PyStr<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let lv = level.expect_i64("msg", "level")?;
            let dbg = _interp
                .get_attr(self_val.0, "debug")
                .ok()
                .and_then(|v| match v.kind() {
                    ValueKind::Int(n) => Some(n),
                    _ => None,
                })
                .unwrap_or(0);
            if lv <= dbg {
                let mut line = s.0.to_string();
                for a in args {
                    line.push(' ');
                    line.push_str(&a.repr_raw());
                }
                line.push('\n');
                write_stdout(_interp, &line);
            }
            Ok(Value::none())
        }
    }
}
