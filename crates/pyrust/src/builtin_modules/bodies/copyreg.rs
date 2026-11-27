// `copyreg` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/copyreg.py` (issues #3208, #3459).
//
// `dispatch_table`, `_extension_registry`, `_inverted_registry` and
// `_extension_cache` are the live, mutable module dicts shared with `copy` and
// `pickle`.  Every function resolves them (and `constructor`, `_reconstructor`,
// `_HEAPTYPE`, `_new_type`) through the module attributes on each call, exactly
// as the CPython source resolves its globals, so code that mutates, inspects or
// rebinds them is seen immediately.
//
// `_reconstructor` and `__newobj__` are the very callables
// `object.__reduce__()` / `object.__reduce_ex__()` hand back (looked up by
// registered name in `interpreter/runtime/builtin_methods/container_dispatch/
// object_reduce.rs`), so `copyreg._reconstructor is obj.__reduce__()[0]`.
//
// Reference: <https://docs.python.org/3.12/library/copyreg.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::copy::bind_python_args;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, construct_primitive_backing, find_immutable_primitive_base,
    find_mutable_primitive_base, find_scalar_primitive_base, is_primitive_class, lookup_class_attr,
    object_class_singleton,
};
use crate::value::{InstanceAttrs, PyClass, PyDict, PyInstance, PyModule, Value, ValueKind};

const MODULE_DOC: &str = "Helper to provide extensibility for pickle.\n\n\
This is only useful to add pickle support for extension types defined in\n\
C, not for instances of user-defined classes.\n";

/// `_HEAPTYPE = 1<<9`: `Py_TPFLAGS_HEAPTYPE`.
const HEAPTYPE: i64 = 1 << 9;

fn all_list() -> Value {
    Value::list(
        [
            "pickle",
            "constructor",
            "add_extension",
            "remove_extension",
            "clear_extension_cache",
        ]
        .into_iter()
        .map(Value::string)
        .collect(),
    )
}

fn type_error(message: impl Into<String>) -> PyError {
    PyError::named("TypeError", message.into())
}

fn value_error(message: impl Into<String>) -> PyError {
    PyError::named("ValueError", message.into())
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_builtin(interp: &mut Interpreter, name: &'static str, args: &[Value]) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), args)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, args)
}

/// A module global of `copyreg`, resolved live.
fn module_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("copyreg")?;
    interp.get_attr(&module, name)
}

fn binop(interp: &mut Interpreter, left: Value, op: BinaryOp, right: Value) -> Result<bool> {
    let result = interp.eval_binary(left, op, right)?;
    interp.truthy_value(&result)
}

fn is_same(interp: &mut Interpreter, left: &Value, right: &Value) -> Result<bool> {
    binop(interp, left.clone(), BinaryOp::Is, right.clone())
}

fn is_callable(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    let result = call_builtin(interp, "callable", std::slice::from_ref(value))?;
    interp.truthy_value(&result)
}

/// `hasattr(obj, name)`: only `AttributeError` counts as "absent".
fn has_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    match interp.get_attr(obj, name) {
        Ok(_) => Ok(true),
        Err(ref err) if err.class_name_is("AttributeError") => Ok(false),
        Err(err) => Err(err),
    }
}

/// `str(value)` as a Rust string.
fn str_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let text = call_builtin(interp, "str", std::slice::from_ref(value))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

/// `repr(value)` as a Rust string.
fn repr_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let text = call_builtin(interp, "repr", std::slice::from_ref(value))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

/// A name from `builtins` (`complex`, `int`, `str`, ...).
fn builtin_value(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let builtins = interp.load_module("builtins")?;
    interp.get_attr(&builtins, name)
}

/// CPython 3.12 rewords the generic `'T' object is not iterable` refusal of a
/// `*args` call expansion to `Value after * must be an iterable, not T`.
fn star_list_extend_error(error: PyError) -> PyError {
    let type_name = match error.kind() {
        pyrust_core::PyErrorKind::Named(class_name, message)
            if class_name.as_ref() == "TypeError" =>
        {
            message
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix("' object is not iterable"))
                .map(str::to_owned)
        }
        _ => None,
    };
    match type_name {
        Some(type_name) => type_error(format!(
            "Value after * must be an iterable, not {type_name}"
        )),
        None => error,
    }
}

/// Whether `class` is one of the interpreter's built-in (non-heap) types:
/// `object` or a primitive-backed type such as `int` / `list` / `dict`.
fn is_static_builtin_type(class: &Value) -> bool {
    let ValueKind::PyClass(rc) = class.kind() else {
        return false;
    };
    Rc::ptr_eq(rc, &object_class_singleton())
        || rc.borrow().canonical_tag.is_some()
        || is_primitive_class(rc)
}

fn object_value() -> Value {
    Value::py_class(object_class_singleton())
}

/// `getattr(obj, name, None)` truthiness: `bool(getattr(obj, name, None))`.
fn truthy_attr_or_none(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    let found = call_builtin(
        interp,
        "getattr",
        &[obj.clone(), Value::string(name), Value::none()],
    )?;
    interp.truthy_value(&found)
}

// ---------------------------------------------------------------------------
// `__newobj__` / `__newobj_ex__` / `_reconstructor`
// ---------------------------------------------------------------------------

fn instance_of(class: Rc<RefCell<PyClass>>, attrs: InstanceAttrs) -> Value {
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// `cls.__new__(cls, *args)`, with the interpreter's native fast paths for
/// primitive-backed classes.
fn newobj_impl(interp: &mut Interpreter, cls: &Value, rest: &[Value]) -> Result<Value> {
    let ValueKind::PyClass(class_rc) = cls.kind() else {
        let new = interp.get_attr(cls, "__new__")?;
        let mut call_args = vec![cls.clone()];
        call_args.extend_from_slice(rest);
        return call_pos(interp, new, &call_args);
    };
    let class_rc = Rc::clone(class_rc);
    let rest_expanded: Vec<ExpandedCallArg> = rest
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    if class_rc.borrow().canonical_tag == Some(pyrust_core::CanonicalClassTag::NoneType) {
        return Ok(Value::none());
    }
    if is_primitive_class(&class_rc) {
        return interp.call_class_expanded(class_rc, &rest_expanded);
    }
    let mutable_base = find_mutable_primitive_base(&class_rc);
    if let Some(base_kind) = mutable_base
        .or_else(|| find_immutable_primitive_base(&class_rc))
        .or_else(|| find_scalar_primitive_base(&class_rc))
        && let Some(tag) = pyrust_core::CanonicalClassTag::from_primitive_name(base_kind)
    {
        // `list.__new__` / `dict.__new__` / `set.__new__` / `bytearray.__new__`
        // ignore their arguments (they are consumed by `__init__`).
        let ctor_args: &[ExpandedCallArg] = if mutable_base.is_some() {
            &[]
        } else {
            &rest_expanded
        };
        let backing = construct_primitive_backing(interp, tag, ctor_args)?;
        let mut attrs = InstanceAttrs::new();
        attrs.set_builtin_data(backing);
        return Ok(instance_of(class_rc, attrs));
    }
    if let Some(new_fn) = lookup_class_attr(&class_rc, "__new__") {
        let mut new_args = Vec::with_capacity(rest_expanded.len() + 1);
        new_args.push(ExpandedCallArg {
            name: None,
            value: Value::py_class(class_rc),
        });
        new_args.extend(rest_expanded);
        return interp.call_function_expanded(new_fn, &new_args);
    }
    Ok(instance_of(class_rc, InstanceAttrs::new()))
}

/// `_reconstructor(cls, base, state)`.
fn reconstructor_impl(
    interp: &mut Interpreter,
    cls: &Value,
    base: &Value,
    state: &Value,
) -> Result<Value> {
    if let (ValueKind::PyClass(class_rc), ValueKind::PyClass(base_rc)) = (cls.kind(), base.kind()) {
        if Rc::ptr_eq(base_rc, &object_class_singleton()) {
            return Ok(instance_of(Rc::clone(class_rc), InstanceAttrs::new()));
        }
        let tag = base_rc.borrow().canonical_tag;
        if let Some(tag) = tag {
            let ctor_args = if state.is_none() {
                Vec::new()
            } else {
                vec![ExpandedCallArg {
                    name: None,
                    value: state.clone(),
                }]
            };
            let backing = construct_primitive_backing(interp, tag, &ctor_args)?;
            let mut attrs = InstanceAttrs::new();
            attrs.set_builtin_data(backing);
            return Ok(instance_of(Rc::clone(class_rc), attrs));
        }
    }
    let object = object_value();
    if is_same(interp, base, &object)? {
        let new = interp.get_attr(&object, "__new__")?;
        return call_pos(interp, new, std::slice::from_ref(cls));
    }
    let new = interp.get_attr(base, "__new__")?;
    let obj = call_pos(interp, new, &[cls.clone(), state.clone()])?;
    let base_init = interp.get_attr(base, "__init__")?;
    let object_init = interp.get_attr(&object, "__init__")?;
    if binop(interp, base_init.clone(), BinaryOp::Ne, object_init)? {
        call_pos(interp, base_init, &[obj.clone(), state.clone()])?;
    }
    Ok(obj)
}

// ---------------------------------------------------------------------------
// `_reduce_ex` (protocols 0 and 1)
// ---------------------------------------------------------------------------

fn reduce_ex_impl(interp: &mut Interpreter, this: &Value, proto: &Value) -> Result<Value> {
    if !binop(interp, proto.clone(), BinaryOp::Lt, Value::int(2))? {
        return Err(PyError::named("AssertionError", String::new()));
    }
    let cls = interp.get_attr(this, "__class__")?;
    let mro = interp.get_attr(&cls, "__mro__")?;
    let heaptype = module_attr(interp, "_HEAPTYPE")?;
    let new_type = module_attr(interp, "_new_type")?;
    let mut found: Option<Value> = None;
    for base in interp.collect_iterable(&mro)? {
        if has_attr(interp, &base, "__flags__")? {
            let flags = interp.get_attr(&base, "__flags__")?;
            if !binop(interp, flags, BinaryOp::BitAnd, heaptype.clone())? {
                found = Some(base);
                break;
            }
        } else if is_static_builtin_type(&base) {
            // Classes carry no `__flags__` here: the interpreter's built-in
            // types are exactly CPython's non-heap (static) types.
            found = Some(base);
            break;
        }
        let new = interp.get_attr(&base, "__new__")?;
        let is_new_type = call_builtin(interp, "isinstance", &[new.clone(), new_type.clone()])?;
        if interp.truthy_value(&is_new_type)? {
            let owner = call_builtin(
                interp,
                "getattr",
                &[new.clone(), Value::string("__self__"), Value::none()],
            )?;
            if is_same(interp, &owner, &base)? {
                found = Some(base);
                break;
            }
        }
    }
    let object = object_value();
    let base = found.unwrap_or_else(|| object.clone());
    let state = if is_same(interp, &base, &object)? {
        Value::none()
    } else {
        if is_same(interp, &base, &cls)? {
            let name = interp.get_attr(&cls, "__name__")?;
            let name = repr_of(interp, &name)?;
            return Err(type_error(format!("cannot pickle {name} object")));
        }
        call_pos(interp, base.clone(), std::slice::from_ref(this))?
    };
    let args = Value::tuple(vec![cls.clone(), base, state]);
    let dict = match interp.get_attr(this, "__getstate__") {
        Err(err) if err.class_name_is("AttributeError") => {
            if truthy_attr_or_none(interp, this, "__slots__")? {
                let name = interp.get_attr(&cls, "__name__")?;
                let name = repr_of(interp, &name)?;
                return Err(type_error(format!(
                    "cannot pickle {name} object: a class that defines __slots__ without \
                     defining __getstate__ cannot be pickled with protocol {}",
                    str_of(interp, proto)?
                )));
            }
            match interp.get_attr(this, "__dict__") {
                Ok(dict) => dict,
                Err(err) if err.class_name_is("AttributeError") => Value::none(),
                Err(err) => return Err(err),
            }
        }
        Err(err) => return Err(err),
        Ok(getstate) => {
            let this_type = call_builtin(interp, "type", std::slice::from_ref(this))?;
            let type_getstate = interp.get_attr(&this_type, "__getstate__")?;
            let object_getstate = interp.get_attr(&object, "__getstate__")?;
            if is_same(interp, &type_getstate, &object_getstate)?
                && truthy_attr_or_none(interp, this, "__slots__")?
            {
                return Err(type_error(
                    "a class that defines __slots__ without defining __getstate__ cannot be pickled",
                ));
            }
            call_pos(interp, getstate, &[])?
        }
    };
    let reconstructor = module_attr(interp, "_reconstructor")?;
    if interp.truthy_value(&dict)? {
        Ok(Value::tuple(vec![reconstructor, args, dict]))
    } else {
        Ok(Value::tuple(vec![reconstructor, args]))
    }
}

// ---------------------------------------------------------------------------
// `_slotnames`
// ---------------------------------------------------------------------------

fn slotnames_impl(interp: &mut Interpreter, cls: &Value) -> Result<Value> {
    let class_dict = interp.get_attr(cls, "__dict__")?;
    let cached = call_method(
        interp,
        &class_dict,
        "get",
        &[Value::string("__slotnames__")],
    )?;
    if !cached.is_none() {
        return Ok(cached);
    }
    let mut names: Vec<Value> = Vec::new();
    if has_attr(interp, cls, "__slots__")? {
        let mro = interp.get_attr(cls, "__mro__")?;
        for c in interp.collect_iterable(&mro)? {
            let c_dict = interp.get_attr(&c, "__dict__")?;
            if !binop(
                interp,
                Value::string("__slots__"),
                BinaryOp::In,
                c_dict.clone(),
            )? {
                continue;
            }
            let mut slots = crate::builtin_modules::operator::getitem_value(
                interp,
                c_dict,
                Value::string("__slots__"),
            )?;
            let str_cls = builtin_value(interp, "str")?;
            let is_str = call_builtin(interp, "isinstance", &[slots.clone(), str_cls])?;
            if interp.truthy_value(&is_str)? {
                slots = Value::tuple(vec![slots]);
            }
            for name in interp.collect_iterable(&slots)? {
                if binop(
                    interp,
                    name.clone(),
                    BinaryOp::In,
                    Value::tuple(vec![
                        Value::string("__dict__"),
                        Value::string("__weakref__"),
                    ]),
                )? {
                    continue;
                }
                let starts = call_method(interp, &name, "startswith", &[Value::string("__")])?;
                let mangled = if interp.truthy_value(&starts)? {
                    let ends = call_method(interp, &name, "endswith", &[Value::string("__")])?;
                    !interp.truthy_value(&ends)?
                } else {
                    false
                };
                if !mangled {
                    names.push(name);
                    continue;
                }
                let class_name = interp.get_attr(&c, "__name__")?;
                let stripped = call_method(interp, &class_name, "lstrip", &[Value::string("_")])?;
                if interp.truthy_value(&stripped)? {
                    let stripped = str_of(interp, &stripped)?;
                    let name = str_of(interp, &name)?;
                    names.push(Value::string(format!("_{stripped}{name}")));
                } else {
                    names.push(name);
                }
            }
        }
    }
    let names = Value::list(names);
    // Cache the outcome in the class if at all possible; never die if we can't.
    let _ = call_builtin(
        interp,
        "setattr",
        &[cls.clone(), Value::string("__slotnames__"), names.clone()],
    );
    Ok(names)
}

// ---------------------------------------------------------------------------
// Extension registry
// ---------------------------------------------------------------------------

fn add_extension_impl(
    interp: &mut Interpreter,
    module: Value,
    name: Value,
    code: Value,
) -> Result<Value> {
    let code = call_builtin(interp, "int", &[code])?;
    let in_range = binop(interp, Value::int(1), BinaryOp::Le, code.clone())?
        && binop(interp, code.clone(), BinaryOp::Le, Value::int(0x7fff_ffff))?;
    if !in_range {
        return Err(value_error("code out of range"));
    }
    let key = Value::tuple(vec![module, name]);
    let registry = module_attr(interp, "_extension_registry")?;
    let inverted = module_attr(interp, "_inverted_registry")?;
    let registered_code = call_method(interp, &registry, "get", std::slice::from_ref(&key))?;
    if binop(interp, registered_code, BinaryOp::Eq, code.clone())? {
        let registered_key = call_method(interp, &inverted, "get", std::slice::from_ref(&code))?;
        if binop(interp, registered_key, BinaryOp::Eq, key.clone())? {
            return Ok(Value::none()); // Redundant registrations are benign
        }
    }
    if binop(interp, key.clone(), BinaryOp::In, registry.clone())? {
        let existing =
            crate::builtin_modules::operator::getitem_value(interp, registry, key.clone())?;
        return Err(value_error(format!(
            "key {} is already registered with code {}",
            str_of(interp, &key)?,
            str_of(interp, &existing)?
        )));
    }
    if binop(interp, code.clone(), BinaryOp::In, inverted.clone())? {
        let existing =
            crate::builtin_modules::operator::getitem_value(interp, inverted, code.clone())?;
        return Err(value_error(format!(
            "code {} is already in use for key {}",
            str_of(interp, &code)?,
            str_of(interp, &existing)?
        )));
    }
    crate::builtin_modules::operator::setitem_value(interp, registry, key.clone(), code.clone())?;
    crate::builtin_modules::operator::setitem_value(interp, inverted, code, key)?;
    Ok(Value::none())
}

fn remove_extension_impl(
    interp: &mut Interpreter,
    module: Value,
    name: Value,
    code: Value,
) -> Result<Value> {
    let key = Value::tuple(vec![module, name]);
    let registry = module_attr(interp, "_extension_registry")?;
    let inverted = module_attr(interp, "_inverted_registry")?;
    let registered_code = call_method(interp, &registry, "get", std::slice::from_ref(&key))?;
    let mismatch = if binop(interp, registered_code, BinaryOp::Ne, code.clone())? {
        true
    } else {
        let registered_key = call_method(interp, &inverted, "get", std::slice::from_ref(&code))?;
        binop(interp, registered_key, BinaryOp::Ne, key.clone())?
    };
    if mismatch {
        return Err(value_error(format!(
            "key {} is not registered with code {}",
            str_of(interp, &key)?,
            str_of(interp, &code)?
        )));
    }
    crate::builtin_modules::operator::delitem_value(interp, registry, key)?;
    crate::builtin_modules::operator::delitem_value(interp, inverted, code.clone())?;
    let cache = module_attr(interp, "_extension_cache")?;
    if binop(interp, code.clone(), BinaryOp::In, cache.clone())? {
        crate::builtin_modules::operator::delitem_value(interp, cache, code)?;
    }
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// Post-import wiring
// ---------------------------------------------------------------------------

/// Post-import wiring for the module-level statements CPython runs at import:
/// `pickle(complex, pickle_complex, complex)`, `pickle(type(int | str),
/// pickle_union)`, `_HEAPTYPE` / `_new_type`.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let complex_cls = builtin_value(interp, "complex")?;
    let int_cls = builtin_value(interp, "int")?;
    let int_new = interp.get_attr(&int_cls, "__new__")?;
    let new_type = call_builtin(interp, "type", &[int_new])?;
    let types = interp.load_module("types")?;
    let union_type = interp.get_attr(&types, "UnionType")?;
    let (pickle_complex, pickle_union) = {
        let module = module.borrow();
        (
            module.get_attr_value("pickle_complex"),
            module.get_attr_value("pickle_union"),
        )
    };
    let (Some(pickle_complex), Some(pickle_union)) = (pickle_complex, pickle_union) else {
        return Err(PyError::runtime(
            "copyreg: pickle_complex/pickle_union missing".into(),
        ));
    };
    let dispatch_table = module
        .borrow()
        .get_attr_value("dispatch_table")
        .ok_or_else(|| PyError::runtime("copyreg: dispatch_table missing".into()))?;
    {
        // `object.__reduce__()` / `__reduce_ex__(2)` return the interned
        // builtin callables, so expose those very objects.
        let mut module = module.borrow_mut();
        module.insert_attr("_new_type".to_string(), new_type);
        module.insert_attr(
            "_reconstructor".to_string(),
            Value::builtin_function("copyreg._reconstructor"),
        );
        module.insert_attr(
            "__newobj__".to_string(),
            Value::builtin_function("copyreg.__newobj__"),
        );
    }
    let setitem = crate::builtin_modules::operator::setitem_value;
    setitem(interp, dispatch_table.clone(), complex_cls, pickle_complex)?;
    setitem(interp, dispatch_table, union_type, pickle_union)?;
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
        "dispatch_table" => Value::dict(PyDict::default()),
        "_HEAPTYPE" => Value::int(HEAPTYPE),
        "_extension_registry" => Value::dict(PyDict::default()),
        "_inverted_registry" => Value::dict(PyDict::default()),
        "_extension_cache" => Value::dict(PyDict::default()),
    }

    fn pickle(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [ob_type, pickle_function, constructor_ob] = bind_python_args(
            "pickle",
            &["ob_type", "pickle_function", "constructor_ob"],
            2,
            args,
            &kwargs,
        )?;
        let (ob_type, pickle_function) = (
            ob_type.expect("required"),
            pickle_function.expect("required"),
        );
        if !is_callable(_interp, &pickle_function)? {
            return Err(type_error("reduction functions must be callable"));
        }
        let table = module_attr(_interp, "dispatch_table")?;
        crate::builtin_modules::operator::setitem_value(_interp, table, ob_type, pickle_function)?;
        // The constructor_ob function is a vestige of safe for unpickling.
        // There is no reason for the caller to pass it anymore.
        if let Some(constructor_ob) = constructor_ob
            && !constructor_ob.is_none()
        {
            let constructor = module_attr(_interp, "constructor")?;
            call_pos(_interp, constructor, &[constructor_ob])?;
        }
        Ok(Value::none())
    }

    fn constructor(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [object] = bind_python_args("constructor", &["object"], 1, args, &kwargs)?;
        if !is_callable(_interp, &object.expect("required"))? {
            return Err(type_error("constructors must be callable"));
        }
        Ok(Value::none())
    }

    fn pickle_complex(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [c] = bind_python_args("pickle_complex", &["c"], 1, args, &kwargs)?;
        let c = c.expect("required");
        let real = _interp.get_attr(&c, "real")?;
        let imag = _interp.get_attr(&c, "imag")?;
        let complex_cls = builtin_value(_interp, "complex")?;
        Ok(Value::tuple(vec![complex_cls, Value::tuple(vec![real, imag])]))
    }

    fn pickle_union(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [obj] = bind_python_args("pickle_union", &["obj"], 1, args, &kwargs)?;
        let obj = obj.expect("required");
        let functools = _interp.load_module("functools")?;
        let operator = _interp.load_module("operator")?;
        let reduce = _interp.get_attr(&functools, "reduce")?;
        let or_ = _interp.get_attr(&operator, "or_")?;
        let union_args = _interp.get_attr(&obj, "__args__")?;
        Ok(Value::tuple(vec![reduce, Value::tuple(vec![or_, union_args])]))
    }

    fn _reconstructor(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [cls, base, state] =
            bind_python_args("_reconstructor", &["cls", "base", "state"], 3, args, &kwargs)?;
        reconstructor_impl(
            _interp,
            &cls.expect("required"),
            &base.expect("required"),
            &state.expect("required"),
        )
    }

    fn _reduce_ex(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [this, proto] = bind_python_args("_reduce_ex", &["self", "proto"], 2, args, &kwargs)?;
        reduce_ex_impl(_interp, &this.expect("required"), &proto.expect("required"))
    }

    fn __newobj__(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        if let Some(key) = kwargs.keys().next() {
            return Err(type_error(format!(
                "__newobj__() got an unexpected keyword argument '{key}'"
            )));
        }
        let Some((cls, rest)) = args.split_first() else {
            return Err(type_error(
                "__newobj__() missing 1 required positional argument: 'cls'",
            ));
        };
        newobj_impl(_interp, cls, rest)
    }

    /// Used by pickle protocol 4, instead of __newobj__ to allow classes with
    /// keyword-only arguments to be pickled correctly.
    fn __newobj_ex__(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [cls, new_args, new_kwargs] = bind_python_args(
            "__newobj_ex__",
            &["cls", "args", "kwargs"],
            3,
            args,
            &kwargs,
        )?;
        let (cls, new_args, new_kwargs) = (
            cls.expect("required"),
            new_args.expect("required"),
            new_kwargs.expect("required"),
        );
        let positional = _interp
            .collect_iterable(&new_args)
            .map_err(star_list_extend_error)?;
        let keys = match _interp.get_attr(&new_kwargs, "keys") {
            Ok(keys_fn) => call_pos(_interp, keys_fn, &[])?,
            Err(err) if err.class_name_is("AttributeError") => {
                let type_name = call_builtin(_interp, "type", std::slice::from_ref(&new_kwargs))?;
                let type_name = _interp.get_attr(&type_name, "__name__")?;
                return Err(type_error(format!(
                    "copyreg.__newobj_ex__() argument after ** must be a mapping, not {}",
                    str_of(_interp, &type_name)?
                )));
            }
            Err(err) => return Err(err),
        };
        let keys = _interp.collect_iterable(&keys)?;
        if keys.is_empty() {
            return newobj_impl(_interp, &cls, &positional);
        }
        let mut call_args = Vec::with_capacity(positional.len() + keys.len() + 1);
        call_args.push(ExpandedCallArg {
            name: None,
            value: cls.clone(),
        });
        call_args.extend(
            positional
                .into_iter()
                .map(|value| ExpandedCallArg { name: None, value }),
        );
        for key in keys {
            let Some(name) = key.as_str().map(str::to_string) else {
                let type_name = call_builtin(_interp, "type", std::slice::from_ref(&key))?;
                let type_name = _interp.get_attr(&type_name, "__name__")?;
                return Err(type_error(format!(
                    "keywords must be strings, not {}",
                    str_of(_interp, &type_name)?
                )));
            };
            let value =
                crate::builtin_modules::operator::getitem_value(_interp, new_kwargs.clone(), key)?;
            call_args.push(ExpandedCallArg {
                name: Some(name),
                value,
            });
        }
        let new = _interp.get_attr(&cls, "__new__")?;
        _interp.call_function_expanded(new, &call_args)
    }

    /// Return a list of slot names for a given class.
    fn _slotnames(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [cls] = bind_python_args("_slotnames", &["cls"], 1, args, &kwargs)?;
        slotnames_impl(_interp, &cls.expect("required"))
    }

    /// Register an extension code.
    fn add_extension(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [module, name, code] = bind_python_args(
            "add_extension",
            &["module", "name", "code"],
            3,
            args,
            &kwargs,
        )?;
        add_extension_impl(
            _interp,
            module.expect("required"),
            name.expect("required"),
            code.expect("required"),
        )
    }

    /// Unregister an extension code.  For testing only.
    fn remove_extension(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [module, name, code] = bind_python_args(
            "remove_extension",
            &["module", "name", "code"],
            3,
            args,
            &kwargs,
        )?;
        remove_extension_impl(
            _interp,
            module.expect("required"),
            name.expect("required"),
            code.expect("required"),
        )
    }

    fn clear_extension_cache(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        bind_python_args::<0>("clear_extension_cache", &[], 0, args, &kwargs)?;
        let cache = module_attr(_interp, "_extension_cache")?;
        call_method(_interp, &cache, "clear", &[])?;
        Ok(Value::none())
    }
}
