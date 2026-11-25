// `pyclbr` module — 100% native Rust implementation of Python 3.12's `pyclbr`
// (Python module browser support) module (issue #3249).
//
// Parses Python source files via PyRust's full Python 3.12 `ast` parser to
// build CPython-compatible `Class` and `Function` trees (with `lineno`,
// `end_lineno`, `parent`, `children`, `methods`, `super`, and `is_async`).
//
// Reference: <https://docs.python.org/3.12/library/pyclbr.html>

use std::path::PathBuf;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

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

fn ast_int_opt(val: &Value, field: &str) -> Option<i64> {
    ast_field(val, field).and_then(|v| match v.kind() {
        ValueKind::Int(n) => Some(n),
        _ => None,
    })
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

fn format_base_expr(node: &Value) -> Option<String> {
    let kind = ast_class_name(node)?;
    match kind.as_str() {
        "Name" => ast_str(node, "id"),
        "Attribute" => {
            let base = format_base_expr(&ast_field(node, "value")?)?;
            let attr = ast_str(node, "attr")?;
            Some(format!("{base}.{attr}"))
        }
        "Subscript" => format_base_expr(&ast_field(node, "value")?),
        _ => None,
    }
}

fn find_module_source(
    interp: &mut Interpreter,
    module_name: &str,
    extra_path: Option<&[String]>,
) -> Result<(String, String)> {
    let mut search_paths: Vec<String> = Vec::new();
    if let Some(ep) = extra_path {
        search_paths.extend(ep.iter().cloned());
    }
    if let Ok(sys_mod) = interp.load_module("sys")
        && let Ok(sp) = interp.get_attr(&sys_mod, "path")
        && let Ok(items) = interp.collect_iterable(&sp)
    {
        for item in items {
            if let Some(s) = item.as_str() {
                search_paths.push(s.to_string());
            }
        }
    }

    let parts: Vec<&str> = module_name.split('.').collect();
    for root in &search_paths {
        let mut base = if root.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(root)
        };
        for part in &parts[..parts.len() - 1] {
            base.push(part);
        }
        let leaf = parts[parts.len() - 1];
        let pkg_init = base.join(leaf).join("__init__.py");
        if pkg_init.is_file() {
            let fname = pkg_init.to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&pkg_init)
                .map_err(|e| PyError::from_io_error(&e, Some(&fname)))?;
            return Ok((fname, src));
        }
        let mod_file = base.join(format!("{leaf}.py"));
        if mod_file.is_file() {
            let fname = mod_file.to_string_lossy().into_owned();
            let src = std::fs::read_to_string(&mod_file)
                .map_err(|e| PyError::from_io_error(&e, Some(&fname)))?;
            return Ok((fname, src));
        }
    }

    Err(PyError::import_error(
        "ModuleNotFoundError",
        format!("No module named '{module_name}'"),
        Some(module_name.to_string()),
    ))
}

struct PyclbrCtx<'a> {
    module_name: &'a str,
    filename: &'a str,
    class_cls: Value,
    func_cls: Value,
    top_tree: Value,
    /// Map of `local_name -> (module, orig_name)` for `from X import Y [as Z]`.
    from_imports: IndexMap<String, (String, String)>,
    extra_path: Option<Vec<String>>,
}

impl<'a> PyclbrCtx<'a> {
    fn visit_body(
        &mut self,
        interp: &mut Interpreter,
        stmts: &[Value],
        parent: Option<&Value>,
        parent_is_class: bool,
    ) -> Result<()> {
        for stmt in stmts {
            let Some(kind) = ast_class_name(stmt) else {
                continue;
            };
            match kind.as_str() {
                "FunctionDef" | "AsyncFunctionDef" => {
                    let is_async = kind == "AsyncFunctionDef";
                    let name = ast_str(stmt, "name").unwrap_or_default();
                    let lineno = ast_int_opt(stmt, "lineno").unwrap_or(0);
                    let end_lineno = ast_int_opt(stmt, "end_lineno");
                    let parent_val = parent.cloned().unwrap_or_else(Value::none);
                    let end_val = end_lineno.map(Value::int).unwrap_or_else(Value::none);

                    let func_obj = call_kw(
                        interp,
                        self.func_cls.clone(),
                        &[
                            Value::string(self.module_name),
                            Value::string(&name),
                            Value::string(self.filename),
                            Value::int(lineno),
                            parent_val,
                            Value::bool_(is_async),
                        ],
                        &[("end_lineno", end_val)],
                    )?;

                    if let Some(p) = parent {
                        if let Ok(children) = interp.get_attr(p, "children") {
                            let _ = children.dict_insert(PyKey::str_from(&name), func_obj.clone());
                        }
                        if parent_is_class && let Ok(methods) = interp.get_attr(p, "methods") {
                            let _ = methods.dict_insert(PyKey::str_from(&name), Value::int(lineno));
                        }
                    } else {
                        self.top_tree
                            .dict_insert(PyKey::str_from(&name), func_obj.clone())?;
                    }

                    self.visit_body(interp, &ast_list(stmt, "body"), Some(&func_obj), false)?;
                }
                "ClassDef" => {
                    let name = ast_str(stmt, "name").unwrap_or_default();
                    let lineno = ast_int_opt(stmt, "lineno").unwrap_or(0);
                    let end_lineno = ast_int_opt(stmt, "end_lineno");
                    let parent_val = parent.cloned().unwrap_or_else(Value::none);
                    let end_val = end_lineno.map(Value::int).unwrap_or_else(Value::none);

                    let mut supers: Vec<Value> = Vec::new();
                    for base_expr in ast_list(stmt, "bases") {
                        if let Some(base_name) = format_base_expr(&base_expr) {
                            if base_name == "object" {
                                continue;
                            }
                            let resolved = self.resolve_super(interp, &base_name);
                            supers.push(resolved);
                        }
                    }

                    let cls_obj = call_kw(
                        interp,
                        self.class_cls.clone(),
                        &[
                            Value::string(self.module_name),
                            Value::string(&name),
                            Value::list(supers),
                            Value::string(self.filename),
                            Value::int(lineno),
                            parent_val,
                        ],
                        &[("end_lineno", end_val)],
                    )?;

                    if let Some(p) = parent {
                        if let Ok(children) = interp.get_attr(p, "children") {
                            let _ = children.dict_insert(PyKey::str_from(&name), cls_obj.clone());
                        }
                    } else {
                        self.top_tree
                            .dict_insert(PyKey::str_from(&name), cls_obj.clone())?;
                    }

                    self.visit_body(interp, &ast_list(stmt, "body"), Some(&cls_obj), true)?;
                }
                "ImportFrom" => {
                    if parent.is_none()
                        && let Some(mod_name) = ast_str(stmt, "module")
                    {
                        for alias in ast_list(stmt, "names") {
                            if let Some(orig) = ast_str(&alias, "name") {
                                if orig == "*" {
                                    continue;
                                }
                                let local =
                                    ast_str(&alias, "asname").unwrap_or_else(|| orig.clone());
                                self.from_imports.insert(local, (mod_name.clone(), orig));
                            }
                        }
                    }
                }
                "If" | "Try" | "TryStar" | "With" | "AsyncWith" if parent.is_none() => {
                    self.visit_body(interp, &ast_list(stmt, "body"), parent, parent_is_class)?;
                    self.visit_body(interp, &ast_list(stmt, "orelse"), parent, parent_is_class)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn resolve_super(&self, interp: &mut Interpreter, base_name: &str) -> Value {
        // 1. Check classes already defined in this module.
        if let Some(existing) = self
            .top_tree
            .dict_with(|d| d.get(&PyKey::str_from(base_name)).cloned())
            .flatten()
        {
            return existing;
        }
        // 2. Check `from other_mod import Base` imports.
        if let Some((other_mod, orig_name)) = self.from_imports.get(base_name)
            && other_mod != self.module_name
            && let Ok(other_tree) =
                readmodule_ex_impl(interp, other_mod, self.extra_path.as_deref())
            && let Some(found) = other_tree
                .dict_with(|d| d.get(&PyKey::str_from(orig_name)).cloned())
                .flatten()
        {
            return found;
        }
        Value::string(base_name)
    }
}

fn readmodule_ex_impl(
    interp: &mut Interpreter,
    module_name: &str,
    extra_path: Option<&[String]>,
) -> Result<Value> {
    let (filename, source) = find_module_source(interp, module_name, extra_path)?;

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
                value: Value::string(&filename),
            },
        ],
    )?;

    let pyclbr_mod = interp.load_module("pyclbr")?;
    let class_cls = interp.get_attr(&pyclbr_mod, "Class")?;
    let func_cls = interp.get_attr(&pyclbr_mod, "Function")?;
    let top_tree = Value::dict(PyDict::default());

    let mut ctx = PyclbrCtx {
        module_name,
        filename: &filename,
        class_cls,
        func_cls,
        top_tree: top_tree.clone(),
        from_imports: IndexMap::new(),
        extra_path: extra_path.map(|s| s.to_vec()),
    };

    ctx.visit_body(interp, &ast_list(&tree, "body"), None, false)?;
    Ok(top_tree)
}

fn extract_extra_path(
    interp: &mut Interpreter,
    path: Option<PyValue>,
) -> Result<Option<Vec<String>>> {
    match path {
        None => Ok(None),
        Some(v) if v.0.is_none() => Ok(None),
        Some(v) => {
            let items = interp.collect_iterable(&v.0)?;
            Ok(Some(
                items
                    .into_iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect(),
            ))
        }
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("readmodule"),
            Value::string("readmodule_ex"),
            Value::string("Class"),
            Value::string("Function"),
        ]),
    }

    class Function {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            module: PyStr<'_>,
            name: PyStr<'_>,
            file: PyValue,
            lineno: PyIndex,
            #[default(None)] parent: Option<PyValue>,
            #[default(PyBool(false))] is_async: PyBool,
            #[keyword_only] #[default(None)] end_lineno: Option<PyValue>,
        ) -> Result<Value> {
            let line = lineno.expect_i64("Function", "lineno")?;
            _interp.assign_attr(self_val.0.clone(), "module", Value::string(module.0))?;
            _interp.assign_attr(self_val.0.clone(), "name", Value::string(name.0))?;
            _interp.assign_attr(self_val.0.clone(), "file", file.0)?;
            _interp.assign_attr(self_val.0.clone(), "lineno", Value::int(line))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "end_lineno",
                end_lineno.map(|v| v.0).unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "parent",
                parent.map(|v| v.0).unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "children",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "is_async", Value::bool_(is_async.0))?;
            Ok(Value::none())
        }
    }

    class Class {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            module: PyStr<'_>,
            name: PyStr<'_>,
            super_: PyValue,
            file: PyValue,
            lineno: PyIndex,
            #[default(None)] parent: Option<PyValue>,
            #[keyword_only] #[default(None)] end_lineno: Option<PyValue>,
        ) -> Result<Value> {
            let line = lineno.expect_i64("Class", "lineno")?;
            let super_val = if super_.0.is_none() {
                Value::list(vec![Value::string("object")])
            } else {
                super_.0
            };
            _interp.assign_attr(self_val.0.clone(), "module", Value::string(module.0))?;
            _interp.assign_attr(self_val.0.clone(), "name", Value::string(name.0))?;
            _interp.assign_attr(self_val.0.clone(), "file", file.0)?;
            _interp.assign_attr(self_val.0.clone(), "lineno", Value::int(line))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "end_lineno",
                end_lineno.map(|v| v.0).unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "parent",
                parent.map(|v| v.0).unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "children",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "super", super_val)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "methods",
                Value::dict(PyDict::default()),
            )?;
            Ok(Value::none())
        }

        fn _addmethod(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            lineno: PyIndex,
        ) -> Result<Value> {
            let line = lineno.expect_i64("_addmethod", "lineno")?;
            let methods = _interp.get_attr(self_val.0, "methods")?;
            methods.dict_insert(PyKey::str_from(&name.0), Value::int(line))?;
            Ok(Value::none())
        }
    }

    fn readmodule_ex(
        module: PyStr<'_>,
        #[default(None)] path: Option<PyValue>,
    ) -> Result<Value> {
        let extra = extract_extra_path(_interp, path)?;
        readmodule_ex_impl(_interp, &module.0, extra.as_deref())
    }

    fn readmodule(
        module: PyStr<'_>,
        #[default(None)] path: Option<PyValue>,
    ) -> Result<Value> {
        let extra = extract_extra_path(_interp, path)?;
        let full = readmodule_ex_impl(_interp, &module.0, extra.as_deref())?;
        let pyclbr_mod = _interp.load_module("pyclbr")?;
        let class_cls = _interp.get_attr(&pyclbr_mod, "Class")?;

        let pairs: Vec<(PyKey, Value)> = full
            .dict_with(|d| d.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let filtered = Value::dict(PyDict::default());
        for (k, v) in pairs {
            let is_cls = _interp.call_function_expanded(
                Value::builtin_function("isinstance"),
                &[
                    ExpandedCallArg {
                        name: None,
                        value: v.clone(),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: class_cls.clone(),
                    },
                ],
            )?;
            if _interp.truthy_value(&is_cls)? {
                filtered.dict_insert(k, v)?;
            }
        }
        Ok(filtered)
    }
}
