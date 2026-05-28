// `__future__` module — included into `pub mod future { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`.
//
// Exposes the ten recognised future-feature names as `_Feature` instances with
// the correct CPython 3.12 `optional`, `mandatory`, and `compiler_flag` values,
// plus the CO_xxx integer constants and `all_feature_names`.
//
// Reference: <https://docs.python.org/3/library/__future__.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::{PyClass, PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

// ── Compiler-flag constants (must match CPython's Include/cpython/compile.h) ──

const CO_NESTED: i64 = 0x0010;
const CO_GENERATOR_ALLOWED: i64 = 0;
const CO_FUTURE_DIVISION: i64 = 0x20000;
const CO_FUTURE_ABSOLUTE_IMPORT: i64 = 0x40000;
const CO_FUTURE_WITH_STATEMENT: i64 = 0x80000;
const CO_FUTURE_PRINT_FUNCTION: i64 = 0x100000;
const CO_FUTURE_UNICODE_LITERALS: i64 = 0x200000;
const CO_FUTURE_BARRY_AS_BDFL: i64 = 0x400000;
const CO_FUTURE_GENERATOR_STOP: i64 = 0x800000;
const CO_FUTURE_ANNOTATIONS: i64 = 0x1000000;

// ── _Feature class singleton ──────────────────────────────────────────────────
//
// Built once per thread.  The `__repr__` dunder is wired to the builtin
// registered below so pyrust's standard `repr()` / `print()` dispatch picks it
// up without any per-site plumbing.

thread_local! {
    static FEATURE_CLASS: Rc<RefCell<PyClass>> = {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        attrs.insert(
            "__repr__".to_string(),
            Value::builtin_function("__future__._Feature.__repr__"),
        );
        Rc::new(RefCell::new(PyClass {
            name: "_Feature".to_string(),
            qualname: "_Feature".to_string(),
            base: None,
            extra_bases: vec![],
            attrs,
            mutation_version: std::cell::Cell::new(0),
            subclasses: std::cell::RefCell::new(vec![]),
            metatype: None,
        }))
    };
}

/// Build a `_Feature` instance.
///
/// `optional` and `mandatory` are 5-tuples of `(major, minor, micro,
/// releaselevel, serial)`.  `mandatory` may be `None`.
fn make_feature(
    optional: (i64, i64, i64, &'static str, i64),
    mandatory: Option<(i64, i64, i64, &'static str, i64)>,
    compiler_flag: i64,
) -> Value {
    FEATURE_CLASS.with(|class| {
        let opt_tuple = version_tuple(optional);
        let mand_val = match mandatory {
            Some(m) => version_tuple(m),
            None => Value::none(),
        };
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        attrs.insert("optional".to_string(), opt_tuple);
        attrs.insert("mandatory".to_string(), mand_val);
        attrs.insert("compiler_flag".to_string(), Value::int(compiler_flag));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}

/// Build a version 5-tuple `(major, minor, micro, releaselevel, serial)`.
fn version_tuple(v: (i64, i64, i64, &'static str, i64)) -> Value {
    Value::tuple(vec![
        Value::int(v.0),
        Value::int(v.1),
        Value::int(v.2),
        Value::string(v.3),
        Value::int(v.4),
    ])
}

/// Format a version 5-tuple Value in Python repr style:
/// `(3, 7, 0, 'beta', 1)`.
fn repr_version_tuple(val: &Value) -> String {
    match val.kind() {
        ValueKind::Tuple(items) if items.len() == 5 => {
            let items: Vec<String> = items
                .iter()
                .map(|v| match v.kind() {
                    ValueKind::Int(n) => n.to_string(),
                    ValueKind::Str(s) => format!("'{s}'"),
                    _ => format!("{v:?}"),
                })
                .collect();
            format!("({}, {}, {}, {}, {})", items[0], items[1], items[2], items[3], items[4])
        }
        _ => "???".to_string(),
    }
}

pyrust_module! {
    constants {
        // CO_xxx compiler-flag constants.
        "CO_NESTED"                => Value::int(CO_NESTED),
        "CO_GENERATOR_ALLOWED"     => Value::int(CO_GENERATOR_ALLOWED),
        "CO_FUTURE_DIVISION"       => Value::int(CO_FUTURE_DIVISION),
        "CO_FUTURE_ABSOLUTE_IMPORT"=> Value::int(CO_FUTURE_ABSOLUTE_IMPORT),
        "CO_FUTURE_WITH_STATEMENT" => Value::int(CO_FUTURE_WITH_STATEMENT),
        "CO_FUTURE_PRINT_FUNCTION" => Value::int(CO_FUTURE_PRINT_FUNCTION),
        "CO_FUTURE_UNICODE_LITERALS"=> Value::int(CO_FUTURE_UNICODE_LITERALS),
        "CO_FUTURE_BARRY_AS_BDFL"  => Value::int(CO_FUTURE_BARRY_AS_BDFL),
        "CO_FUTURE_GENERATOR_STOP" => Value::int(CO_FUTURE_GENERATOR_STOP),
        "CO_FUTURE_ANNOTATIONS"    => Value::int(CO_FUTURE_ANNOTATIONS),

        // all_feature_names list — CPython exposes this as a module-level list.
        "all_feature_names" => Value::list(vec![
            Value::string("nested_scopes"),
            Value::string("generators"),
            Value::string("division"),
            Value::string("absolute_import"),
            Value::string("with_statement"),
            Value::string("print_function"),
            Value::string("unicode_literals"),
            Value::string("barry_as_FLUFL"),
            Value::string("generator_stop"),
            Value::string("annotations"),
        ]),

        // Ten recognised feature objects — CPython 3.12 values.
        // <https://github.com/python/cpython/blob/3.12/Lib/__future__.py>
        "nested_scopes"    => make_feature((2,1,0,"beta",1),  Some((2,2,0,"alpha",0)), CO_NESTED),
        "generators"       => make_feature((2,2,0,"alpha",1), Some((2,3,0,"final",0)), CO_GENERATOR_ALLOWED),
        "division"         => make_feature((2,2,0,"alpha",2), Some((3,0,0,"alpha",0)), CO_FUTURE_DIVISION),
        "absolute_import"  => make_feature((2,5,0,"alpha",1), Some((3,0,0,"alpha",0)), CO_FUTURE_ABSOLUTE_IMPORT),
        "with_statement"   => make_feature((2,5,0,"alpha",1), Some((2,6,0,"alpha",0)), CO_FUTURE_WITH_STATEMENT),
        "print_function"   => make_feature((2,6,0,"alpha",2), Some((3,0,0,"alpha",0)), CO_FUTURE_PRINT_FUNCTION),
        "unicode_literals" => make_feature((2,6,0,"alpha",2), Some((3,0,0,"alpha",0)), CO_FUTURE_UNICODE_LITERALS),
        "barry_as_FLUFL"   => make_feature((3,1,0,"alpha",2), Some((4,0,0,"alpha",0)), CO_FUTURE_BARRY_AS_BDFL),
        "generator_stop"   => make_feature((3,5,0,"beta",1),  Some((3,7,0,"alpha",0)), CO_FUTURE_GENERATOR_STOP),
        "annotations"      => make_feature((3,7,0,"beta",1),  None,                    CO_FUTURE_ANNOTATIONS),
    }

    /// `repr(_Feature_instance)` — produces the CPython-style representation:
    /// `_Feature((major, minor, micro, 'level', serial), None_or_tuple, flag)`.
    ///
    /// Registered as `"__future__._Feature.__repr__"`.
    #[py_name = "_Feature.__repr__"]
    fn feature_repr(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::Runtime(
                "_Feature.__repr__() missing self".to_string(),
            ));
        }
        let inst = match args[0].value.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "_Feature.__repr__() requires a _Feature instance".to_string(),
                ))
            }
        };
        let borrow = inst.borrow();
        let optional = borrow
            .attrs
            .get("optional")
            .cloned()
            .unwrap_or_else(Value::none);
        let mandatory = borrow
            .attrs
            .get("mandatory")
            .cloned()
            .unwrap_or_else(Value::none);
        let flag = match borrow.attrs.get("compiler_flag").map(|v| v.kind()) {
            Some(ValueKind::Int(n)) => n,
            _ => 0,
        };
        let opt_s = repr_version_tuple(&optional);
        let mand_s = match mandatory.kind() {
            ValueKind::None => "None".to_string(),
            _ => repr_version_tuple(&mandatory),
        };
        Ok(Value::string(format!("_Feature({opt_s}, {mand_s}, {flag})")))
    }
}
