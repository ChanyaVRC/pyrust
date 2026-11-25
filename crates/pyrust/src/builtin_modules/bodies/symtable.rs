// `symtable` module — 100% native Rust implementation of Python 3.12's
// `symtable` module (issue #3249).
//
// Wraps `_symtable`'s analyzed `RawSymTable` tree in CPython-compatible
// `SymbolTable`, `Function(SymbolTable)`, `Class(SymbolTable)`, `Symbol`, and
// `SymbolTableFactory` classes using PyRust's expanded macro toolkit.
//
// Reference: <https://docs.python.org/3.12/library/symtable.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use super::_symtable::{
    CELL, DEF_ANNOT, DEF_BOUND, DEF_GLOBAL, DEF_IMPORT, DEF_LOCAL, DEF_NONLOCAL, DEF_PARAM, FREE,
    GLOBAL_EXPLICIT, GLOBAL_IMPLICIT, LOCAL, RawSymTable, SCOPE_MASK, SCOPE_OFF, TYPE_CLASS,
    TYPE_FUNCTION, TYPE_MODULE, USE, analyze_source, extract_raw_entry,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyPath, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, Value, ValueKind};

struct SymTableState {
    raw: Rc<RawSymTable>,
    filename: String,
}

struct SymbolState {
    name: String,
    flags: i64,
    scope: i64,
    namespaces: Vec<Value>,
    module_scope: bool,
}

fn make_symtable_obj(
    interp: &mut Interpreter,
    raw: Rc<RawSymTable>,
    filename: &str,
) -> Result<Value> {
    let mod_val = interp.load_module("symtable")?;
    let cls_name = match raw.block_type {
        TYPE_FUNCTION => "Function",
        TYPE_CLASS => "Class",
        _ => "SymbolTable",
    };
    let cls = interp.get_attr(&mod_val, cls_name)?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::runtime(format!("corrupt symtable.{cls_name}")));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    crate::builtin_modules::macros::native_state::set_native_state(
        &inst,
        SymTableState {
            raw,
            filename: filename.to_string(),
        },
    )?;
    Ok(inst)
}

fn make_symbol_obj(
    interp: &mut Interpreter,
    name: String,
    flags: i64,
    namespaces: Vec<Value>,
    module_scope: bool,
) -> Result<Value> {
    let mod_val = interp.load_module("symtable")?;
    let cls = interp.get_attr(&mod_val, "Symbol")?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::runtime("corrupt symtable.Symbol".into()));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    let scope = (flags >> SCOPE_OFF) & SCOPE_MASK;
    crate::builtin_modules::macros::native_state::set_native_state(
        &inst,
        SymbolState {
            name,
            flags,
            scope,
            namespaces,
            module_scope,
        },
    )?;
    Ok(inst)
}

fn with_st_state<R>(self_val: &Value, f: impl FnOnce(&SymTableState) -> Result<R>) -> Result<R> {
    crate::builtin_modules::macros::native_state::with_native_state::<SymTableState, _>(
        self_val,
        "SymbolTable",
        f,
    )
}

fn lookup_symbol_in_table(
    interp: &mut Interpreter,
    raw: &Rc<RawSymTable>,
    filename: &str,
    name: &str,
) -> Result<Value> {
    let Some(&flags) = raw.symbols.get(name) else {
        return Err(PyError::key_error(Value::string(name)));
    };
    let mut namespaces = Vec::new();
    for child in &raw.children {
        if child.name == name {
            namespaces.push(make_symtable_obj(interp, Rc::clone(child), filename)?);
        }
    }
    let module_scope = raw.block_type == TYPE_MODULE;
    make_symbol_obj(interp, name.to_string(), flags, namespaces, module_scope)
}

fn format_table_repr(raw: &RawSymTable, filename: &str) -> String {
    match raw.block_type {
        TYPE_MODULE => format!("<SymbolTable for module {filename}>"),
        TYPE_FUNCTION => format!("<Function SymbolTable for {} in {filename}>", raw.name),
        TYPE_CLASS => format!("<Class SymbolTable for {} in {filename}>", raw.name),
        _ => format!("<SymbolTable for {} in {filename}>", raw.name),
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("symtable"),
            Value::string("SymbolTable"),
            Value::string("Class"),
            Value::string("Function"),
            Value::string("Symbol"),
        ]),
        "USE" => Value::int(USE),
        "DEF_GLOBAL" => Value::int(DEF_GLOBAL),
        "DEF_NONLOCAL" => Value::int(DEF_NONLOCAL),
        "DEF_LOCAL" => Value::int(DEF_LOCAL),
        "DEF_PARAM" => Value::int(DEF_PARAM),
        "DEF_IMPORT" => Value::int(DEF_IMPORT),
        "DEF_BOUND" => Value::int(DEF_BOUND),
        "DEF_ANNOT" => Value::int(DEF_ANNOT),
        "SCOPE_OFF" => Value::int(SCOPE_OFF),
        "SCOPE_MASK" => Value::int(SCOPE_MASK),
        "FREE" => Value::int(FREE),
        "LOCAL" => Value::int(LOCAL),
        "GLOBAL_IMPLICIT" => Value::int(GLOBAL_IMPLICIT),
        "GLOBAL_EXPLICIT" => Value::int(GLOBAL_EXPLICIT),
        "CELL" => Value::int(CELL),
    }

    #[native_state(SymTableState)]
    class SymbolTable {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            raw_table: PyValue,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let raw = extract_raw_entry(&raw_table.0)?;
            set_state(
                self_val.0,
                SymTableState {
                    raw,
                    filename: filename.0.to_string(),
                },
            )?;
            Ok(Value::none())
        }

        fn __repr__(#[state] st: &SymTableState) -> Result<Value> {
            Ok(Value::string(format_table_repr(&st.raw, &st.filename)))
        }

        fn get_type(#[state] st: &SymTableState) -> Result<Value> {
            Ok(Value::string(st.raw.type_name()))
        }

        fn get_id(#[state] st: &SymTableState) -> Result<Value> {
            Ok(Value::int(st.raw.id))
        }

        fn get_name(#[state] st: &SymTableState) -> Result<Value> {
            Ok(Value::string(&st.raw.name))
        }

        fn get_lineno(#[state] st: &SymTableState) -> Result<Value> {
            Ok(Value::int(st.raw.lineno))
        }

        fn is_optimized(#[state] st: &SymTableState) -> Result<Value> {
            Ok(Value::bool_(st.raw.optimized))
        }

        fn is_nested(#[state] st: &SymTableState) -> Result<Value> {
            Ok(Value::bool_(st.raw.nested))
        }

        fn has_children(#[state] st: &SymTableState) -> Result<Value> {
            Ok(Value::bool_(!st.raw.children.is_empty()))
        }

        fn get_identifiers(#[state] st: &SymTableState) -> Result<Value> {
            let mut d = PyDict::default();
            for (k, &v) in &st.raw.symbols {
                d.insert(PyKey::str_from(k), Value::int(v));
            }
            let dict_val = Value::dict(d);
            let keys_fn = _interp.get_attr(&dict_val, "keys")?;
            _interp.call_function_expanded(keys_fn, &[])
        }

        fn lookup(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let (raw, filename) = with_state(self_val.0, |st| {
                Ok((Rc::clone(&st.raw), st.filename.clone()))
            })?;
            lookup_symbol_in_table(_interp, &raw, &filename, &name.0)
        }

        fn get_symbols(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (raw, filename) = with_state(self_val.0, |st| {
                Ok((Rc::clone(&st.raw), st.filename.clone()))
            })?;
            let mut out = Vec::with_capacity(raw.symbols.len());
            for name in raw.symbols.keys() {
                out.push(lookup_symbol_in_table(_interp, &raw, &filename, name)?);
            }
            Ok(Value::list(out))
        }

        fn get_children(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (raw, filename) = with_state(self_val.0, |st| {
                Ok((Rc::clone(&st.raw), st.filename.clone()))
            })?;
            let mut out = Vec::with_capacity(raw.children.len());
            for child in &raw.children {
                out.push(make_symtable_obj(_interp, Rc::clone(child), &filename)?);
            }
            Ok(Value::list(out))
        }
    }

    class Function("SymbolTable") {
        fn get_parameters(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_st_state(self_val.0, |st| {
                let params: Vec<Value> = st
                    .raw
                    .symbols
                    .iter()
                    .filter(|(_, flags)| (**flags & DEF_PARAM) != 0)
                    .map(|(k, _)| Value::string(k))
                    .collect();
                Ok(Value::tuple(params))
            })
        }

        fn get_locals(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_st_state(self_val.0, |st| {
                let locals: Vec<Value> = st
                    .raw
                    .symbols
                    .iter()
                    .filter(|(_, flags)| {
                        let scope = (**flags >> SCOPE_OFF) & SCOPE_MASK;
                        scope == LOCAL || scope == CELL
                    })
                    .map(|(k, _)| Value::string(k))
                    .collect();
                Ok(Value::tuple(locals))
            })
        }

        fn get_globals(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_st_state(self_val.0, |st| {
                let globals: Vec<Value> = st
                    .raw
                    .symbols
                    .iter()
                    .filter(|(_, flags)| {
                        let scope = (**flags >> SCOPE_OFF) & SCOPE_MASK;
                        scope == GLOBAL_IMPLICIT || scope == GLOBAL_EXPLICIT
                    })
                    .map(|(k, _)| Value::string(k))
                    .collect();
                Ok(Value::tuple(globals))
            })
        }

        fn get_nonlocals(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_st_state(self_val.0, |st| {
                let nonlocals: Vec<Value> = st
                    .raw
                    .symbols
                    .iter()
                    .filter(|(_, flags)| (**flags & DEF_NONLOCAL) != 0)
                    .map(|(k, _)| Value::string(k))
                    .collect();
                Ok(Value::tuple(nonlocals))
            })
        }

        fn get_frees(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_st_state(self_val.0, |st| {
                let frees: Vec<Value> = st
                    .raw
                    .symbols
                    .iter()
                    .filter(|(_, flags)| ((**flags >> SCOPE_OFF) & SCOPE_MASK) == FREE)
                    .map(|(k, _)| Value::string(k))
                    .collect();
                Ok(Value::tuple(frees))
            })
        }
    }

    class Class("SymbolTable") {
        fn get_methods(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_st_state(self_val.0, |st| {
                let mut seen = indexmap::IndexSet::new();
                for child in &st.raw.children {
                    seen.insert(child.name.clone());
                }
                Ok(Value::tuple(seen.into_iter().map(Value::string).collect()))
            })
        }
    }

    #[native_state(SymbolState)]
    class Symbol {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            flags: PyIndex,
            #[default(None)] namespaces: Option<PyValue>,
            #[keyword_only] #[default(PyBool(false))] module_scope: PyBool,
        ) -> Result<Value> {
            let flags_i64 = flags.expect_i64("Symbol", "flags")?;
            let ns_vec = match namespaces {
                Some(v) if !v.0.is_none() => _interp.collect_iterable(&v.0)?,
                _ => Vec::new(),
            };
            let scope = (flags_i64 >> SCOPE_OFF) & SCOPE_MASK;
            set_state(
                self_val.0,
                SymbolState {
                    name: name.0.to_string(),
                    flags: flags_i64,
                    scope,
                    namespaces: ns_vec,
                    module_scope: module_scope.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __repr__(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::string(format!(
                "<symbol {}>",
                Value::string(&sym.name).repr_raw()
            )))
        }

        fn get_name(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::string(&sym.name))
        }

        fn is_referenced(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_((sym.flags & USE) != 0))
        }

        fn is_parameter(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_((sym.flags & DEF_PARAM) != 0))
        }

        fn is_global(#[state] sym: &SymbolState) -> Result<Value> {
            let g = sym.scope == GLOBAL_IMPLICIT
                || sym.scope == GLOBAL_EXPLICIT
                || (sym.module_scope && (sym.flags & DEF_BOUND) != 0);
            Ok(Value::bool_(g))
        }

        fn is_nonlocal(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_((sym.flags & DEF_NONLOCAL) != 0))
        }

        fn is_declared_global(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_(sym.scope == GLOBAL_EXPLICIT))
        }

        fn is_local(#[state] sym: &SymbolState) -> Result<Value> {
            let l = sym.scope == LOCAL
                || sym.scope == CELL
                || (sym.module_scope && (sym.flags & DEF_BOUND) != 0);
            Ok(Value::bool_(l))
        }

        fn is_annotated(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_((sym.flags & DEF_ANNOT) != 0))
        }

        fn is_free(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_(sym.scope == FREE))
        }

        fn is_imported(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_((sym.flags & DEF_IMPORT) != 0))
        }

        fn is_assigned(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_((sym.flags & DEF_LOCAL) != 0))
        }

        fn is_namespace(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::bool_(!sym.namespaces.is_empty()))
        }

        fn get_namespaces(#[state] sym: &SymbolState) -> Result<Value> {
            Ok(Value::list(sym.namespaces.clone()))
        }

        fn get_namespace(#[state] sym: &SymbolState) -> Result<Value> {
            match sym.namespaces.len() {
                0 => Err(PyError::named(
                    "ValueError",
                    "name is not bound to any namespaces".to_string(),
                )),
                1 => Ok(sym.namespaces[0].clone()),
                _ => Err(PyError::named(
                    "ValueError",
                    "name is bound to multiple namespaces".to_string(),
                )),
            }
        }
    }

    class SymbolTableFactory {
        fn __init__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn new(
            #[positional_only] _self: PyValueRef<'_>,
            table: PyValue,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let raw = extract_raw_entry(&table.0)?;
            make_symtable_obj(_interp, raw, &filename.0)
        }

        fn __call__(
            #[positional_only] _self: PyValueRef<'_>,
            table: PyValue,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let raw = extract_raw_entry(&table.0)?;
            make_symtable_obj(_interp, raw, &filename.0)
        }
    }

    fn symtable(
        code: PyValue,
        filename: PyPath,
        compile_type: PyStr<'_>,
    ) -> Result<Value> {
        let fname = filename.as_path().to_string_lossy().into_owned();
        let raw = analyze_source(_interp, &code.0, &fname, &compile_type.0)?;
        make_symtable_obj(_interp, raw, &fname)
    }
}
