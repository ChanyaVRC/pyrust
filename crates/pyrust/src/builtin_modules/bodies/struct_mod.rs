// `struct` module — 100% native-Rust Python 3.12 implementation (issue #3309).
//
// This file is the `pyrust_module!` facade: the public `error`, `Struct`, and
// top-level functions.  Format parsing, coercion/binary16/offset validation,
// packing, unpacking, CPython-exact argument diagnostics, and the
// per-interpreter class/format registry live in submodules under
// `bodies/struct_mod/`.
//
// Reference: <https://docs.python.org/3/library/struct.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

#[path = "struct_mod/args.rs"]
mod args;
#[path = "struct_mod/coercion.rs"]
mod coercion;
#[path = "struct_mod/format.rs"]
mod format;
#[path = "struct_mod/pack.rs"]
mod pack;
#[path = "struct_mod/registry.rs"]
mod registry;
#[path = "struct_mod/unpack.rs"]
mod unpack;

use format::CompiledFormat;

const MODULE_DOC: &str = concat!(
    "Functions to convert between Python values and C structs.
",
    "Python bytes objects are used to hold the data representing the C struct
",
    "and also as format strings (explained below) to describe the layout of data
",
    "in the C struct.
",
    "
",
    "The optional first format char indicates byte order, size and alignment:
",
    "  @: native order, size & alignment (default)
",
    "  =: native order, std. size & alignment
",
    "  <: little-endian, std. size & alignment
",
    "  >: big-endian, std. size & alignment
",
    "  !: same as >
",
    "
",
    "The remaining chars indicate types of args and must match exactly;
",
    "these can be preceded by a decimal repeat count:
",
    "  x: pad byte (no data); c:char; b:signed byte; B:unsigned byte;
",
    "  ?: _Bool (requires C99; if not available, char is used instead)
",
    "  h:short; H:unsigned short; i:int; I:unsigned int;
",
    "  l:long; L:unsigned long; f:float; d:double; e:half-float.
",
    "Special cases (preceding decimal count indicates length):
",
    "  s:string (array of char); p: pascal string (with count byte).
",
    "Special cases (only available in native format):
",
    "  n:ssize_t; N:size_t;
",
    "  P:an integer type that is wide enough to hold a pointer.
",
    "Special case (not in native mode unless 'long long' in platform C):
",
    "  q:long long; Q:unsigned long long
",
    "Whitespace between formats is ignored.
",
    "
",
    "The variable struct.error is an exception raised on errors.
",
);

/// Compiled layout attached to every `Struct` instance.
struct StructState {
    compiled: Rc<CompiledFormat>,
}

/// Cursor over a snapshot of the buffer handed to `iter_unpack`.
struct UnpackIterState {
    compiled: Rc<CompiledFormat>,
    data: Vec<u8>,
    pos: usize,
}

fn all_list() -> Value {
    Value::list(
        [
            "calcsize",
            "pack",
            "pack_into",
            "unpack",
            "unpack_from",
            "iter_unpack",
            "Struct",
            "error",
        ]
        .into_iter()
        .map(Value::string)
        .collect(),
    )
}

/// Post-import wiring: share the `error` class across module generations,
/// hide `unpack_iterator`, and give the classes CPython's `_struct` identity.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module) = module.kind() else {
        return;
    };
    for (name, qualified, doc) in [
        (
            "Struct",
            "_struct.Struct",
            Some("Struct(fmt) --> compiled struct object\n\n"),
        ),
        ("unpack_iterator", "_struct.unpack_iterator", None),
    ] {
        let class = module.borrow().attrs.get(name).cloned();
        if let Some(class) = class
            && let ValueKind::PyClass(class) = class.kind()
        {
            let mut class = class.borrow_mut();
            class
                .attrs
                .insert("__module__".to_string(), Value::string("_struct"));
            if let Some(doc) = doc {
                class
                    .attrs
                    .insert("__doc__".to_string(), Value::string(doc));
            }
            class.error_name = Some(qualified);
        }
    }
    registry::adopt_module_classes(module);
    // `class error(Exception): pass` carries `__doc__ = None` in CPython.
    let error = module.borrow().attrs.get("error").cloned();
    if let Some(error) = error
        && let ValueKind::PyClass(error) = error.kind()
    {
        error
            .borrow_mut()
            .attrs
            .entry("__doc__".to_string())
            .or_insert_with(Value::none);
    }
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

/// Split the bound `self` off a method's raw argument list.
fn split_self<'a>(
    method: &str,
    args: &'a [ExpandedCallArg],
) -> Result<(&'a Value, &'a [ExpandedCallArg])> {
    match args.split_first() {
        Some((this, rest)) if this.name.is_none() => Ok((&this.value, rest)),
        _ => Err(type_error(format!(
            "descriptor '{method}' of '_struct.Struct' object needs an argument"
        ))),
    }
}

fn struct_compiled(this: &Value) -> Result<Rc<CompiledFormat>> {
    with_native_state::<StructState, _>(this, "Struct", |state| Ok(Rc::clone(&state.compiled)))
}

fn positional_values(args: &[ExpandedCallArg]) -> Vec<Value> {
    args.iter().map(|arg| arg.value.clone()).collect()
}

fn make_unpack_iterator(
    interp: &mut Interpreter,
    compiled: Rc<CompiledFormat>,
    buffer: &Value,
    error_cls: &Value,
) -> Result<Value> {
    let data = unpack::snapshot_for_iter_unpack(interp, &compiled, buffer, error_cls)?;
    let class = registry::unpack_iterator_class(interp)?;
    let iterator = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::default(),
    })));
    set_native_state(
        &iterator,
        UnpackIterState {
            compiled,
            data,
            pos: 0,
        },
    )?;
    Ok(iterator)
}

fn unpack_from_with(
    interp: &mut Interpreter,
    compiled: &CompiledFormat,
    parsed: args::UnpackFromArgs<'_>,
    error_cls: &Value,
) -> Result<Value> {
    let zero = Value::int(0);
    let offset = parsed.offset.unwrap_or(&zero);
    unpack::unpack_buffer_from(interp, compiled, parsed.buffer, offset, error_cls)
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
    }

    fn calcsize(args) -> Result<Value> {
        let format = args::exactly_one("_struct.calcsize", args)?;
        let error_cls = registry::error_class(_interp)?;
        let compiled = registry::compile_cached(format, &error_cls)?;
        Ok(Value::int(compiled.size as i64))
    }

    fn iter_unpack(args) -> Result<Value> {
        let (format, buffer) = args::exactly_two("iter_unpack", "_struct.iter_unpack", args)?;
        let error_cls = registry::error_class(_interp)?;
        let compiled = registry::compile_cached(format, &error_cls)?;
        make_unpack_iterator(_interp, compiled, buffer, &error_cls)
    }

    fn pack(args) -> Result<Value> {
        args::no_keywords("_struct.pack", args)?;
        let Some((format, values)) = args.split_first() else {
            return Err(type_error("missing format argument"));
        };
        let error_cls = registry::error_class(_interp)?;
        let compiled = registry::compile_cached(&format.value, &error_cls)?;
        pack::pack_values(_interp, &compiled, &positional_values(values), &error_cls)
    }

    fn pack_into(args) -> Result<Value> {
        args::no_keywords("_struct.pack_into", args)?;
        let Some((format, rest)) = args.split_first() else {
            return Err(type_error("missing format argument"));
        };
        let error_cls = registry::error_class(_interp)?;
        let compiled = registry::compile_cached(&format.value, &error_cls)?;
        let (positional, _) = args::split_args(rest);
        let (buffer, offset) = args::pack_into_head(&positional, &error_cls)?;
        pack::pack_into_buffer(
            _interp,
            &compiled,
            buffer,
            offset,
            &positional_values(&rest[2..]),
            &error_cls,
        )
    }

    fn unpack(args) -> Result<Value> {
        let (format, buffer) = args::exactly_two("unpack", "_struct.unpack", args)?;
        let error_cls = registry::error_class(_interp)?;
        let compiled = registry::compile_cached(format, &error_cls)?;
        unpack::unpack_buffer(_interp, &compiled, buffer, &error_cls)
    }

    fn unpack_from(args) -> Result<Value> {
        let (positional, keywords) = args::split_args(args);
        let parsed = args::parse_unpack_from(&positional, &keywords, 1)?;
        let error_cls = registry::error_class(_interp)?;
        let compiled = registry::compile_cached(positional[0], &error_cls)?;
        unpack_from_with(_interp, &compiled, parsed, &error_cls)
    }

    fn _clearcache(args) -> Result<Value> {
        args::no_arguments("_struct._clearcache", args)?;
        registry::clear_format_cache();
        Ok(Value::none())
    }

    #[native_state(StructState)]
    class Struct {
        fn __init__(args) -> Result<Value> {
            let (this, rest) = split_self("__init__", args)?;
            let (positional, keywords) = args::split_args(rest);
            let format = args::parse_struct_init(&positional, &keywords)?;
            let error_cls = registry::error_class(_interp)?;
            let compiled = registry::compile_cached(format, &error_cls)?;
            set_state(this, StructState { compiled })?;
            Ok(Value::none())
        }

        #[getter]
        fn format(#[state] state: &StructState) -> Result<Value> {
            Ok(Value::string(&state.compiled.format_str))
        }

        #[setter("format")]
        fn set_format(
            #[positional_only] _this: PyValueRef<'_>,
            #[positional_only] _value: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "AttributeError",
                "attribute 'format' of '_struct.Struct' objects is not writable".to_string(),
            ))
        }

        #[getter]
        fn size(#[state] state: &StructState) -> Result<Value> {
            Ok(Value::int(state.compiled.size as i64))
        }

        #[setter("size")]
        fn set_size(
            #[positional_only] _this: PyValueRef<'_>,
            #[positional_only] _value: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "AttributeError",
                "attribute 'size' of '_struct.Struct' objects is not writable".to_string(),
            ))
        }

        fn pack(args) -> Result<Value> {
            let (this, rest) = split_self("pack", args)?;
            args::no_keywords("Struct.pack", rest)?;
            let compiled = struct_compiled(this)?;
            let error_cls = registry::error_class(_interp)?;
            pack::pack_values(_interp, &compiled, &positional_values(rest), &error_cls)
        }

        fn pack_into(args) -> Result<Value> {
            let (this, rest) = split_self("pack_into", args)?;
            args::no_keywords("Struct.pack_into", rest)?;
            let compiled = struct_compiled(this)?;
            let error_cls = registry::error_class(_interp)?;
            let (positional, _) = args::split_args(rest);
            let (buffer, offset) = args::pack_into_head(&positional, &error_cls)?;
            pack::pack_into_buffer(
                _interp,
                &compiled,
                buffer,
                offset,
                &positional_values(&rest[2..]),
                &error_cls,
            )
        }

        fn unpack(args) -> Result<Value> {
            let (this, rest) = split_self("unpack", args)?;
            let buffer = args::exactly_one("Struct.unpack", rest)?;
            let compiled = struct_compiled(this)?;
            let error_cls = registry::error_class(_interp)?;
            unpack::unpack_buffer(_interp, &compiled, buffer, &error_cls)
        }

        fn unpack_from(args) -> Result<Value> {
            let (this, rest) = split_self("unpack_from", args)?;
            let (positional, keywords) = args::split_args(rest);
            let parsed = args::parse_unpack_from(&positional, &keywords, 0)?;
            let compiled = struct_compiled(this)?;
            let error_cls = registry::error_class(_interp)?;
            unpack_from_with(_interp, &compiled, parsed, &error_cls)
        }

        fn iter_unpack(args) -> Result<Value> {
            let (this, rest) = split_self("iter_unpack", args)?;
            let buffer = args::exactly_one("Struct.iter_unpack", rest)?;
            let compiled = struct_compiled(this)?;
            let error_cls = registry::error_class(_interp)?;
            make_unpack_iterator(_interp, compiled, buffer, &error_cls)
        }
    }

    // No docstring: CPython 3.12 `struct.error.__doc__` is `None`.
    exception error;

    // The iterator returned by `iter_unpack`; not exported by `struct`.
    #[native_state(UnpackIterState)]
    class unpack_iterator {
        fn __init__(_args) -> Result<Value> {
            Err(type_error("cannot create '_struct.unpack_iterator' instances"))
        }

        fn __iter__(#[positional_only] this: PyValueRef<'_>) -> Result<Value> {
            Ok(this.0.clone())
        }

        fn __next__(#[state_mut] state: &mut UnpackIterState) -> Result<Value> {
            let size = state.compiled.size;
            if state.pos >= state.data.len() {
                return Err(PyError::named("StopIteration", String::new()));
            }
            let end = state.pos + size;
            let item = unpack::unpack_from_slice(&state.compiled, &state.data[state.pos..end]);
            state.pos = end;
            Ok(item)
        }

        fn __length_hint__(#[state] state: &UnpackIterState) -> Result<Value> {
            let remaining = state.data.len().saturating_sub(state.pos);
            Ok(Value::int((remaining / state.compiled.size.max(1)) as i64))
        }
    }
}
