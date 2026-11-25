// `reprlib` module — 100% native Rust implementation of Python 3.12's
// `reprlib` module (issue #3248).
//
// Provides `Repr` (with Python 3.12 `indent` parameter and full subclass
// dispatch to `repr_<typename>` methods), `aRepr`, `repr`, and the
// `recursive_repr` decorator.
//
// Reference: <https://docs.python.org/3/library/reprlib.html>

use std::collections::HashSet;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyIndex, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, primitive_class_by_name, render_value_repr, value_class,
    value_type_name_str,
};
use crate::value::{PyDict, PyKey, Value, ValueKind};

fn py_isinstance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> Result<bool> {
    let res = call_positional(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls.clone()],
    )?;
    interp.truthy_value(&res)
}

fn lookup_dict() -> Value {
    let mut d = PyDict::default();
    for (typename, module) in [
        ("tuple", "builtins"),
        ("list", "builtins"),
        ("array", "array"),
        ("set", "builtins"),
        ("frozenset", "builtins"),
        ("deque", "collections"),
        ("dict", "builtins"),
        ("str", "builtins"),
        ("int", "builtins"),
    ] {
        d.insert(PyKey::str_from(typename), Value::string(module));
    }
    Value::dict(d)
}

fn call_positional(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn py_len(interp: &mut Interpreter, val: &Value) -> Result<usize> {
    let len_fn = Value::builtin_function("len");
    let res = call_positional(interp, len_fn, std::slice::from_ref(val))?;
    let n = res
        .as_int()
        .ok_or_else(|| PyError::named("TypeError", "len() did not return an integer"))?;
    Ok(n.max(0) as usize)
}

fn iter_take(interp: &mut Interpreter, source: &Value, limit: usize) -> Result<Vec<Value>> {
    let iter_fn = Value::builtin_function("iter");
    let next_fn = Value::builtin_function("next");
    let it = call_positional(interp, iter_fn, std::slice::from_ref(source))?;
    let mut out = Vec::with_capacity(limit.min(64));
    for _ in 0..limit {
        match call_positional(interp, next_fn.clone(), std::slice::from_ref(&it)) {
            Ok(item) => out.push(item),
            Err(e) if e.class_name_is("StopIteration") => break,
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

fn possibly_sorted(interp: &mut Interpreter, x: &Value) -> Result<Value> {
    let sorted_fn = Value::builtin_function("sorted");
    match call_positional(interp, sorted_fn, std::slice::from_ref(x)) {
        Ok(v) => Ok(v),
        Err(e) if is_exception_err(interp, &e) => {
            let list_cls = primitive_class_by_name("list")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("list"));
            call_positional(interp, list_cls, std::slice::from_ref(x))
        }
        Err(e) => Err(e),
    }
}

fn is_exception_err(interp: &mut Interpreter, err: &PyError) -> bool {
    let Ok(exc_val) = interp.materialize_pyerror(err.clone()) else {
        return false;
    };
    let Some(exc_cls) = crate::interpreter::lookup_exc_class("Exception") else {
        return false;
    };
    py_isinstance(interp, &exc_val, &Value::py_class(exc_cls)).unwrap_or(false)
}

fn attr_isize(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<isize> {
    let val = interp.get_attr(obj, name)?;
    match val.as_int() {
        Some(n) => Ok(n as isize),
        None => {
            let idx = PyIndex::try_from_value_with_interp(interp, &val, name, name)?;
            idx.expect_isize(name, name)
        }
    }
}

fn attr_string(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<String> {
    let val = interp.get_attr(obj, name)?;
    val.as_str().map(str::to_owned).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "expected str for '{}', got '{}'",
                name,
                value_type_name_str(&val)
            ),
        )
    })
}

fn repr_truncate(s: &str, max_len: isize, fillvalue: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if (chars.len() as isize) <= max_len {
        return s.to_string();
    }
    let i = (max_len - 3).div_euclid(2).max(0) as usize;
    let j = (max_len - 3 - i as isize).max(0) as usize;
    let prefix: String = chars.iter().take(i).collect();
    let suffix: String = if j == 0 {
        String::new()
    } else {
        chars[chars.len().saturating_sub(j)..].iter().collect()
    };
    format!("{prefix}{fillvalue}{suffix}")
}

fn copy_wrapped_metadata(interp: &mut Interpreter, wrapper: &Value, user_fn: &Value) -> Result<()> {
    for attr in ["__module__", "__doc__", "__name__", "__qualname__"] {
        let val = interp.get_attr(user_fn, attr)?;
        interp.assign_attr(wrapper.clone(), attr, val)?;
    }
    let annotations = match interp.get_attr(user_fn, "__annotations__") {
        Ok(v) => v,
        Err(ref e) if e.class_name_is("AttributeError") => Value::dict(PyDict::default()),
        Err(e) => return Err(e),
    };
    interp.assign_attr(wrapper.clone(), "__annotations__", annotations)?;
    let type_params = match interp.get_attr(user_fn, "__type_params__") {
        Ok(v) => v,
        Err(ref e) if e.class_name_is("AttributeError") => Value::tuple(Vec::new()),
        Err(e) => return Err(e),
    };
    interp.assign_attr(wrapper.clone(), "__type_params__", type_params)?;
    Ok(())
}

// ── Native state for recursive_repr decorator & wrapper ──────────────────────

struct DecoratorState {
    fillvalue: Value,
}

struct WrapperState {
    user_function: Value,
    fillvalue: Value,
    running: HashSet<i64>,
}

struct BoundWrapperState {
    wrapper: Value,
    instance: Value,
}

fn object_id_key(val: &Value) -> i64 {
    val.object_id().as_int().unwrap_or(0)
}

fn reprlib_class_from_modules(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let mod_val = interp.load_module("reprlib")?;
    interp.get_attr(&mod_val, name)
}

/// Install `aRepr = Repr()` and `repr = aRepr.repr` on a freshly loaded
/// `reprlib` module instance.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let repr_cls = mod_rc.borrow().attrs.get("Repr").cloned();
    let Some(repr_cls) = repr_cls else {
        return;
    };
    let Ok(a_repr) = interp.call_function_expanded(repr_cls, &[]) else {
        return;
    };
    let Ok(repr_method) = interp.get_attr(&a_repr, "repr") else {
        return;
    };
    let mut m = mod_rc.borrow_mut();
    m.insert_attr("aRepr".to_string(), a_repr);
    m.insert_attr("repr".to_string(), repr_method);
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("Repr"),
            Value::string("repr"),
            Value::string("recursive_repr"),
        ]),
    }

    fn recursive_repr(
        #[default(OptionalArg::Omitted)] fillvalue: OptValueRef<'_>,
    ) -> Result<Value> {
        let fill = fillvalue
            .as_value()
            .cloned()
            .unwrap_or_else(|| Value::string("..."));
        let dec_cls = reprlib_class_from_modules(_interp, "_RecursiveReprDecorator")?;
        call_positional(_interp, dec_cls, &[fill])
    }

    #[native_state(DecoratorState)]
    class _RecursiveReprDecorator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fillvalue: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                DecoratorState {
                    fillvalue: fillvalue.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            user_function: PyValue,
        ) -> Result<Value> {
            let fillvalue = with_state(self_val.0, |st| Ok(st.fillvalue.clone()))?;
            let wrapper_cls = reprlib_class_from_modules(_interp, "_RecursiveReprWrapper")?;
            let wrapper = call_positional(
                _interp,
                wrapper_cls,
                &[user_function.0.clone(), fillvalue],
            )?;
            copy_wrapped_metadata(_interp, &wrapper, &user_function.0)?;
            Ok(wrapper)
        }
    }

    #[native_state(WrapperState)]
    class _RecursiveReprWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            user_function: PyValue,
            fillvalue: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                WrapperState {
                    user_function: user_function.0,
                    fillvalue: fillvalue.0,
                    running: HashSet::new(),
                },
            )?;
            Ok(Value::none())
        }

        fn __get__(
            #[positional_only] self_val: PyValueRef<'_>,
            obj: PyValue,
            #[default(OptionalArg::Omitted)] _objtype: OptValueRef<'_>,
        ) -> Result<Value> {
            if obj.0.is_none() {
                return Ok(self_val.0.clone());
            }
            let bound_cls = reprlib_class_from_modules(_interp, "_BoundRecursiveRepr")?;
            call_positional(_interp, bound_cls, &[self_val.0.clone(), obj.0])
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            target: PyValue,
        ) -> Result<Value> {
            let key = object_id_key(&target.0);
            let (already_running, user_fn, fillvalue) = with_state_mut(self_val.0, |st| {
                if st.running.contains(&key) {
                    return Ok((true, Value::none(), st.fillvalue.clone()));
                }
                st.running.insert(key);
                Ok((false, st.user_function.clone(), Value::none()))
            })?;
            if already_running {
                return Ok(fillvalue);
            }
            let res = call_positional(_interp, user_fn, &[target.0]);
            let _ = with_state_mut(self_val.0, |st| {
                st.running.remove(&key);
                Ok(())
            });
            res
        }
    }

    #[native_state(BoundWrapperState)]
    class _BoundRecursiveRepr {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            wrapper: PyValue,
            instance: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "__func__", wrapper.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "__self__", instance.0.clone())?;
            set_state(
                self_val.0,
                BoundWrapperState {
                    wrapper: wrapper.0,
                    instance: instance.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (wrapper, instance) =
                with_state(self_val.0, |st| Ok((st.wrapper.clone(), st.instance.clone())))?;
            call_positional(_interp, wrapper, &[instance])
        }
    }

    class Repr {
        constants {
            "_lookup" => lookup_dict(),
        }

        #[allow(clippy::too_many_arguments)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxlevel: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxtuple: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxlist: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxarray: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxdict: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxset: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxfrozenset: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxdeque: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxstring: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxlong: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] maxother: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] fillvalue: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] indent: OptValueRef<'_>,
        ) -> Result<Value> {
            let defaults: [(&str, OptValueRef<'_>, Value); 13] = [
                ("maxlevel", maxlevel, Value::int(6)),
                ("maxtuple", maxtuple, Value::int(6)),
                ("maxlist", maxlist, Value::int(6)),
                ("maxarray", maxarray, Value::int(5)),
                ("maxdict", maxdict, Value::int(4)),
                ("maxset", maxset, Value::int(6)),
                ("maxfrozenset", maxfrozenset, Value::int(6)),
                ("maxdeque", maxdeque, Value::int(6)),
                ("maxstring", maxstring, Value::int(30)),
                ("maxlong", maxlong, Value::int(40)),
                ("maxother", maxother, Value::int(30)),
                ("fillvalue", fillvalue, Value::string("...")),
                ("indent", indent, Value::none()),
            ];
            for (name, opt, default_val) in defaults {
                let val = opt.as_value().cloned().unwrap_or(default_val);
                _interp.assign_attr(self_val.0.clone(), name, val)?;
            }
            Ok(Value::none())
        }

        fn repr(#[positional_only] self_val: PyValueRef<'_>, x: PyValue) -> Result<Value> {
            let maxlevel = _interp.get_attr(self_val.0, "maxlevel")?;
            let repr1 = _interp.get_attr(self_val.0, "repr1")?;
            call_positional(_interp, repr1, &[x.0, maxlevel])
        }

        fn repr1(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let cls = value_class(&x.0);
            let raw_typename = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| value_type_name_str(&x.0).to_string());
            let typename = if raw_typename.contains(' ') {
                raw_typename
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join("_")
            } else {
                raw_typename
            };
            let method_name = format!("repr_{typename}");
            let method = match _interp.get_attr(self_val.0, &method_name) {
                Ok(v) => Some(v),
                Err(ref e) if e.class_name_is("AttributeError") => None,
                Err(e) => return Err(e),
            };
            if let Some(method) = method
                && _interp.truthy_value(&method)?
            {
                let lookup = _interp.get_attr(self_val.0, "_lookup")?;
                let expected_mod = lookup
                    .as_dict()
                    .and_then(|d| d.get(&PyKey::str_from(&typename)).cloned());
                match expected_mod {
                    None => return call_positional(_interp, method, &[x.0, level.0]),
                    Some(exp) => {
                        let cls_mod = match _interp.get_attr(&cls, "__module__") {
                            Ok(v) => v,
                            Err(ref e) if e.class_name_is("AttributeError") => Value::none(),
                            Err(e) => return Err(e),
                        };
                        let eq = _interp.eval_binary(
                            cls_mod,
                            crate::ast::BinaryOp::Eq,
                            exp,
                        )?;
                        if _interp.truthy_value(&eq)? {
                            return call_positional(_interp, method, &[x.0, level.0]);
                        }
                    }
                }
            }
            let repr_inst = _interp.get_attr(self_val.0, "repr_instance")?;
            call_positional(_interp, repr_inst, &[x.0, level.0])
        }

        fn _join(
            #[positional_only] self_val: PyValueRef<'_>,
            pieces: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let piece_vals = _interp.collect_iterable(&pieces.0)?;
            let mut piece_strs = Vec::with_capacity(piece_vals.len());
            for p in &piece_vals {
                let Some(s) = p.as_str() else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "sequence item: expected str instance, {} found",
                            value_type_name_str(p)
                        ),
                    ));
                };
                piece_strs.push(s.to_owned());
            }
            let indent_val = _interp.get_attr(self_val.0, "indent")?;
            if indent_val.is_none() {
                return Ok(Value::string(piece_strs.join(", ")));
            }
            if piece_strs.is_empty() {
                return Ok(Value::string(""));
            }
            let int_cls = primitive_class_by_name("int").map(Value::py_class);
            let indent_str = if let Some(int_cls) = int_cls
                && py_isinstance(_interp, &indent_val, &int_cls)?
            {
                let n = indent_val.as_int().unwrap_or(0);
                if n < 0 {
                    let r = render_value_repr(_interp, &indent_val)?;
                    return Err(PyError::named(
                        "ValueError",
                        format!("Repr.indent cannot be negative int (was {r})"),
                    ));
                }
                " ".repeat(n as usize)
            } else if let Some(s) = indent_val.as_str() {
                s.to_owned()
            } else {
                let t_repr = render_value_repr(_interp, &value_class(&indent_val))?;
                return Err(PyError::named(
                    "TypeError",
                    format!("Repr.indent must be a str, int or None, not {t_repr}"),
                ));
            };

            let maxlevel = attr_isize(_interp, self_val.0, "maxlevel")?;
            let level_n = match level.0.as_int() {
                Some(n) => n as isize,
                None => PyIndex::try_from_value_with_interp(_interp, &level.0, "_join", "level")?
                    .expect_isize("_join", "level")?,
            };
            let repeat_n = (maxlevel - level_n + 1).max(0) as usize;
            let sep = format!(",\n{}", indent_str.repeat(repeat_n));
            let mut joined = String::new();
            joined.push_str(&sep);
            for p in &piece_strs {
                joined.push_str(p);
                joined.push_str(&sep);
            }
            let chars: Vec<char> = joined.chars().collect();
            let indent_chars = indent_str.chars().count();
            let end = if indent_chars == 0 {
                chars.len()
            } else {
                chars.len().saturating_sub(indent_chars)
            };
            let sliced: String = if end >= 1 {
                chars[1..end].iter().collect()
            } else {
                String::new()
            };
            Ok(Value::string(sliced))
        }

        fn _repr_iterable(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
            left: PyValue,
            right: PyValue,
            maxiter: PyValue,
            #[default(OptionalArg::Omitted)] trail: OptValueRef<'_>,
        ) -> Result<Value> {
            let left_s = left.0.as_str().unwrap_or("").to_owned();
            let mut right_s = right.0.as_str().unwrap_or("").to_owned();
            let trail_s = trail
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let n = py_len(_interp, &x.0)?;
            let level_n = match level.0.as_int() {
                Some(v) => v as isize,
                None => {
                    PyIndex::try_from_value_with_interp(_interp, &level.0, "_repr_iterable", "level")?
                        .expect_isize("_repr_iterable", "level")?
                }
            };
            let maxiter_n = match maxiter.0.as_int() {
                Some(v) => v.max(0) as usize,
                None => PyIndex::try_from_value_with_interp(
                    _interp,
                    &maxiter.0,
                    "_repr_iterable",
                    "maxiter",
                )?
                .expect_isize("_repr_iterable", "maxiter")?
                .max(0) as usize,
            };
            let s = if level_n <= 0 && n > 0 {
                attr_string(_interp, self_val.0, "fillvalue")?
            } else {
                let newlevel = Value::int((level_n - 1) as i64);
                let repr1 = _interp.get_attr(self_val.0, "repr1")?;
                let elems = iter_take(_interp, &x.0, maxiter_n)?;
                let mut pieces = Vec::with_capacity(elems.len() + 1);
                for elem in elems {
                    pieces.push(call_positional(
                        _interp,
                        repr1.clone(),
                        &[elem, newlevel.clone()],
                    )?);
                }
                if n > maxiter_n {
                    pieces.push(_interp.get_attr(self_val.0, "fillvalue")?);
                }
                let join_fn = _interp.get_attr(self_val.0, "_join")?;
                let joined = call_positional(_interp, join_fn, &[Value::list(pieces), level.0])?;
                let indent_is_none = _interp.get_attr(self_val.0, "indent")?.is_none();
                if n == 1 && !trail_s.is_empty() && indent_is_none {
                    right_s = format!("{trail_s}{right_s}");
                }
                joined.as_str().unwrap_or("").to_owned()
            };
            Ok(Value::string(format!("{left_s}{s}{right_s}")))
        }

        fn repr_tuple(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let maxtuple = _interp.get_attr(self_val.0, "maxtuple")?;
            let f = _interp.get_attr(self_val.0, "_repr_iterable")?;
            call_positional(
                _interp,
                f,
                &[
                    x.0,
                    level.0,
                    Value::string("("),
                    Value::string(")"),
                    maxtuple,
                    Value::string(","),
                ],
            )
        }

        fn repr_list(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let maxlist = _interp.get_attr(self_val.0, "maxlist")?;
            let f = _interp.get_attr(self_val.0, "_repr_iterable")?;
            call_positional(
                _interp,
                f,
                &[x.0, level.0, Value::string("["), Value::string("]"), maxlist],
            )
        }

        fn repr_array(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let typecode = _interp.get_attr(&x.0, "typecode")?;
            let tc = typecode.as_str().unwrap_or("").to_owned();
            if !_interp.truthy_value(&x.0)? {
                return Ok(Value::string(format!("array('{tc}')")));
            }
            let header = format!("array('{tc}', [");
            let maxarray = _interp.get_attr(self_val.0, "maxarray")?;
            let f = _interp.get_attr(self_val.0, "_repr_iterable")?;
            call_positional(
                _interp,
                f,
                &[
                    x.0,
                    level.0,
                    Value::string(header),
                    Value::string("])"),
                    maxarray,
                ],
            )
        }

        fn repr_set(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            if !_interp.truthy_value(&x.0)? {
                return Ok(Value::string("set()"));
            }
            let sorted = possibly_sorted(_interp, &x.0)?;
            let maxset = _interp.get_attr(self_val.0, "maxset")?;
            let f = _interp.get_attr(self_val.0, "_repr_iterable")?;
            call_positional(
                _interp,
                f,
                &[
                    sorted,
                    level.0,
                    Value::string("{"),
                    Value::string("}"),
                    maxset,
                ],
            )
        }

        fn repr_frozenset(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            if !_interp.truthy_value(&x.0)? {
                return Ok(Value::string("frozenset()"));
            }
            let sorted = possibly_sorted(_interp, &x.0)?;
            let maxfrozenset = _interp.get_attr(self_val.0, "maxfrozenset")?;
            let f = _interp.get_attr(self_val.0, "_repr_iterable")?;
            call_positional(
                _interp,
                f,
                &[
                    sorted,
                    level.0,
                    Value::string("frozenset({"),
                    Value::string("})"),
                    maxfrozenset,
                ],
            )
        }

        fn repr_deque(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let maxdeque = _interp.get_attr(self_val.0, "maxdeque")?;
            let f = _interp.get_attr(self_val.0, "_repr_iterable")?;
            call_positional(
                _interp,
                f,
                &[
                    x.0,
                    level.0,
                    Value::string("deque(["),
                    Value::string("])"),
                    maxdeque,
                ],
            )
        }

        fn repr_dict(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let n = py_len(_interp, &x.0)?;
            if n == 0 {
                return Ok(Value::string("{}"));
            }
            let level_n = match level.0.as_int() {
                Some(v) => v as isize,
                None => PyIndex::try_from_value_with_interp(_interp, &level.0, "repr_dict", "level")?
                    .expect_isize("repr_dict", "level")?,
            };
            if level_n <= 0 {
                let fill = attr_string(_interp, self_val.0, "fillvalue")?;
                return Ok(Value::string(format!("{{{fill}}}")));
            }
            let newlevel = Value::int((level_n - 1) as i64);
            let maxdict = attr_isize(_interp, self_val.0, "maxdict")?.max(0) as usize;
            let repr1 = _interp.get_attr(self_val.0, "repr1")?;
            let keys_sorted = possibly_sorted(_interp, &x.0)?;
            let keys = iter_take(_interp, &keys_sorted, maxdict)?;
            let mut pieces = Vec::with_capacity(keys.len() + 1);
            for key in keys {
                let keyrepr = call_positional(_interp, repr1.clone(), &[key.clone(), newlevel.clone()])?;
                let val = _interp.eval_index(&x.0, key)?;
                let valrepr = call_positional(_interp, repr1.clone(), &[val, newlevel.clone()])?;
                let ks = keyrepr.as_str().unwrap_or("");
                let vs = valrepr.as_str().unwrap_or("");
                pieces.push(Value::string(format!("{ks}: {vs}")));
            }
            if n > maxdict {
                pieces.push(_interp.get_attr(self_val.0, "fillvalue")?);
            }
            let join_fn = _interp.get_attr(self_val.0, "_join")?;
            let joined = call_positional(_interp, join_fn, &[Value::list(pieces), level.0])?;
            let js = joined.as_str().unwrap_or("");
            Ok(Value::string(format!("{{{js}}}")))
        }

        fn repr_str(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            _level: PyValue,
        ) -> Result<Value> {
            let maxstring = attr_isize(_interp, self_val.0, "maxstring")?;
            let fill = attr_string(_interp, self_val.0, "fillvalue")?;
            let Some(raw) = x.0.as_str() else {
                return Ok(Value::string(render_value_repr(_interp, &x.0)?));
            };
            let chars: Vec<char> = raw.chars().collect();
            let take_n = maxstring.max(0) as usize;
            let prefix_s: String = chars.iter().take(take_n).collect();
            let mut s = render_value_repr(_interp, &Value::string(prefix_s))?;
            if (s.chars().count() as isize) > maxstring {
                let i = (maxstring - 3).div_euclid(2).max(0) as usize;
                let j = (maxstring - 3 - i as isize).max(0) as usize;
                let mut combined: String = chars.iter().take(i).collect();
                if j > 0 {
                    let start = chars.len().saturating_sub(j);
                    combined.extend(chars[start..].iter());
                }
                let s2 = render_value_repr(_interp, &Value::string(combined))?;
                let s2_chars: Vec<char> = s2.chars().collect();
                let left: String = s2_chars.iter().take(i).collect();
                let right: String = if j == 0 {
                    String::new()
                } else {
                    s2_chars[s2_chars.len().saturating_sub(j)..]
                        .iter()
                        .collect()
                };
                s = format!("{left}{fill}{right}");
            }
            Ok(Value::string(s))
        }

        fn repr_int(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            _level: PyValue,
        ) -> Result<Value> {
            let maxlong = attr_isize(_interp, self_val.0, "maxlong")?;
            let fill = attr_string(_interp, self_val.0, "fillvalue")?;
            let s = render_value_repr(_interp, &x.0)?;
            Ok(Value::string(repr_truncate(&s, maxlong, &fill)))
        }

        fn repr_instance(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            _level: PyValue,
        ) -> Result<Value> {
            let s = match render_value_repr(_interp, &x.0) {
                Ok(r) => r,
                Err(e) if is_exception_err(_interp, &e) => {
                    let cls = value_class(&x.0);
                    let cls_name = _interp
                        .get_attr(&cls, "__name__")
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| value_type_name_str(&x.0).to_string());
                    let id_val = x.0.object_id().as_int().unwrap_or(0) as u64;
                    return Ok(Value::string(format!("<{cls_name} instance at {id_val:#x}>")));
                }
                Err(e) => return Err(e),
            };
            let maxother = attr_isize(_interp, self_val.0, "maxother")?;
            let fill = attr_string(_interp, self_val.0, "fillvalue")?;
            Ok(Value::string(repr_truncate(&s, maxother, &fill)))
        }
    }
}
