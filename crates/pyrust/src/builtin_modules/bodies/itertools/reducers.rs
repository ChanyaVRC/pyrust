// Predicate-filtering, mapping, zipping, and accumulating `itertools`
// iterators: `takewhile`, `dropwhile`, `starmap`, `accumulate`, `compress`,
// `zip_longest`, and `filterfalse`.

use super::helpers::{
    attr_bool, attr_cloned, attr_i64, check_not_exhausted, expect_self, init_binary_func_iter,
    internal, is_stop_iteration, make_iter,
};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

pub(super) fn takewhile_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    init_binary_func_iter(
        interp,
        args,
        fn_name,
        "takewhile",
        "_pred",
        Some(("_done", false)),
    )
}

pub(super) fn takewhile_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    check_not_exhausted(&inst, "_done")?;
    let pred = attr_cloned(&inst, "_pred", fn_name)?;
    let iter = attr_cloned(&inst, "_iter", fn_name)?;
    let item = interp.call_next(&iter, None)?;
    let verdict = interp.call_function_expanded(
        pred,
        &[ExpandedCallArg {
            name: None,
            value: item.clone(),
        }],
    )?;
    if interp.truthy_value(&verdict)? {
        Ok(item)
    } else {
        inst.borrow_mut().attrs.insert("_done", Value::bool_(true));
        Err(PyError::named("StopIteration", String::new()))
    }
}

pub(super) fn dropwhile_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    init_binary_func_iter(
        interp,
        args,
        fn_name,
        "dropwhile",
        "_pred",
        Some(("_started", false)),
    )
}

pub(super) fn dropwhile_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let iter = attr_cloned(&inst, "_iter", fn_name)?;
    if attr_bool(&inst, "_started") {
        return interp.call_next(&iter, None);
    }
    let pred = attr_cloned(&inst, "_pred", fn_name)?;
    loop {
        let item = interp.call_next(&iter, None)?;
        let verdict = interp.call_function_expanded(
            pred.clone(),
            &[ExpandedCallArg {
                name: None,
                value: item.clone(),
            }],
        )?;
        if !interp.truthy_value(&verdict)? {
            inst.borrow_mut()
                .attrs
                .insert("_started", Value::bool_(true));
            return Ok(item);
        }
    }
}

pub(super) fn starmap_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    init_binary_func_iter(interp, args, fn_name, "starmap", "_func", None)
}

pub(super) fn starmap_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let func = attr_cloned(&inst, "_func", fn_name)?;
    let iter = attr_cloned(&inst, "_iter", fn_name)?;
    let pack = interp.call_next(&iter, None)?;
    let unpacked: Vec<ExpandedCallArg> = interp
        .collect_iterable(&pack)?
        .into_iter()
        .map(|v| ExpandedCallArg {
            name: None,
            value: v,
        })
        .collect();
    interp.call_function_expanded(func, &unpacked)
}

pub(super) fn accumulate_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut positional: Vec<Value> = Vec::new();
    let mut iterable_kw: Option<Value> = None;
    let mut func_kw: Option<Value> = None;
    let mut initial: Option<Value> = None;
    for a in &args[1..] {
        match a.name.as_deref() {
            Some("iterable") => iterable_kw = Some(a.value.clone()),
            Some("func") => func_kw = Some(a.value.clone()),
            Some("initial") => {
                if !a.value.is_none() {
                    initial = Some(a.value.clone());
                }
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for accumulate()"),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    if positional.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "accumulate() takes at most 2 positional arguments ({} given)",
                positional.len()
            ),
        ));
    }
    if !positional.is_empty() && iterable_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for accumulate() given by name ('iterable') and position (1)".to_string(),
        ));
    }
    if positional.len() >= 2 && func_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for accumulate() given by name ('func') and position (2)".to_string(),
        ));
    }
    let iterable_val = iterable_kw
        .or_else(|| positional.first().cloned())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "accumulate() missing required argument 'iterable' (pos 1)".to_string(),
            )
        })?;
    let iter = make_iter(interp, &iterable_val)?;
    let func = func_kw
        .or_else(|| positional.get(1).cloned())
        .unwrap_or_else(Value::none);
    let use_initial = initial.is_some();
    let mut a = inst.borrow_mut();
    a.attrs.insert("_func", func);
    a.attrs.insert("_iter", iter);
    a.attrs.insert("_acc", initial.unwrap_or_else(Value::none));
    a.attrs.insert("_use_initial", Value::bool_(use_initial));
    a.attrs.insert("_started", Value::bool_(false));
    Ok(Value::none())
}

pub(super) fn accumulate_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if !attr_bool(&inst, "_started") {
        inst.borrow_mut()
            .attrs
            .insert("_started", Value::bool_(true));
        if attr_bool(&inst, "_use_initial") {
            return attr_cloned(&inst, "_acc", fn_name);
        }
        let iter = attr_cloned(&inst, "_iter", fn_name)?;
        let first = interp.call_next(&iter, None)?;
        inst.borrow_mut().attrs.insert("_acc", first.clone());
        return Ok(first);
    }
    let func = attr_cloned(&inst, "_func", fn_name)?;
    let iter = attr_cloned(&inst, "_iter", fn_name)?;
    let acc = attr_cloned(&inst, "_acc", fn_name)?;
    let nxt = interp.call_next(&iter, None)?;
    let new_acc = if func.is_none() {
        interp.eval_binary(acc, crate::ast::BinaryOp::Add, nxt)?
    } else {
        interp.call_function_expanded(
            func,
            &[
                ExpandedCallArg {
                    name: None,
                    value: acc,
                },
                ExpandedCallArg {
                    name: None,
                    value: nxt,
                },
            ],
        )?
    };
    inst.borrow_mut().attrs.insert("_acc", new_acc.clone());
    Ok(new_acc)
}

pub(super) fn compress_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut positional: Vec<Value> = Vec::new();
    let mut data_kw: Option<Value> = None;
    let mut selectors_kw: Option<Value> = None;
    for a in &args[1..] {
        match a.name.as_deref() {
            Some("data") => data_kw = Some(a.value.clone()),
            Some("selectors") => selectors_kw = Some(a.value.clone()),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for compress()"),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    if positional.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "compress() takes at most 2 arguments ({} given)",
                positional.len()
            ),
        ));
    }
    if !positional.is_empty() && data_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for compress() given by name ('data') and position (1)".to_string(),
        ));
    }
    if positional.len() >= 2 && selectors_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for compress() given by name ('selectors') and position (2)".to_string(),
        ));
    }
    let data_val = data_kw
        .or_else(|| positional.first().cloned())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "compress() missing required argument 'data' (pos 1)".to_string(),
            )
        })?;
    let selectors_val = selectors_kw
        .or_else(|| positional.get(1).cloned())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "compress() missing required argument 'selectors' (pos 2)".to_string(),
            )
        })?;
    let data_iter = make_iter(interp, &data_val)?;
    let selectors_iter = make_iter(interp, &selectors_val)?;
    let mut a = inst.borrow_mut();
    a.attrs.insert("_data", data_iter);
    a.attrs.insert("_selectors", selectors_iter);
    Ok(Value::none())
}

pub(super) fn compress_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let data_iter = attr_cloned(&inst, "_data", fn_name)?;
    let sel_iter = attr_cloned(&inst, "_selectors", fn_name)?;
    loop {
        let item = interp.call_next(&data_iter, None)?;
        let selector = interp.call_next(&sel_iter, None)?;
        if interp.truthy_value(&selector)? {
            return Ok(item);
        }
    }
}

pub(super) fn zip_longest_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut positional: Vec<Value> = Vec::new();
    let mut fillvalue = Value::none();
    for a in &args[1..] {
        match a.name.as_deref() {
            Some("fillvalue") => fillvalue = a.value.clone(),
            Some(_) => {
                return Err(PyError::named(
                    "TypeError",
                    "zip_longest() got an unexpected keyword argument".to_string(),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    let iters: Vec<Value> = positional
        .iter()
        .map(|v| make_iter(interp, v))
        .collect::<Result<_>>()?;
    let n = iters.len();
    let mut a = inst.borrow_mut();
    a.attrs.insert("_iters", Value::list(iters));
    a.attrs.insert("_fillvalue", fillvalue);
    a.attrs.insert("_active", Value::int(n as i64));
    a.attrs
        .insert("_done", Value::list(vec![Value::bool_(false); n]));
    Ok(Value::none())
}

pub(super) fn zip_longest_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let active = attr_i64(&inst, "_active", fn_name)?;
    if active == 0 {
        return Err(PyError::named("StopIteration", String::new()));
    }
    let iters = attr_cloned(&inst, "_iters", fn_name)?;
    let fillvalue = attr_cloned(&inst, "_fillvalue", fn_name)?;
    let n = match iters.kind() {
        ValueKind::List(lst) => lst.len(),
        _ => return Err(internal(fn_name)),
    };
    let mut tuple: Vec<Value> = Vec::with_capacity(n);
    let mut new_active = active;
    let done_val = attr_cloned(&inst, "_done", fn_name)?;
    let mut new_done = match done_val.kind() {
        ValueKind::List(lst) => lst.to_vec(),
        _ => return Err(internal(fn_name)),
    };
    if new_done.len() != n {
        return Err(internal(fn_name));
    }
    let mut cleared_indices: Vec<usize> = Vec::new();
    for (i, done) in new_done.iter_mut().enumerate() {
        if matches!(done.kind(), ValueKind::Bool(true)) {
            tuple.push(fillvalue.clone());
        } else {
            let iter = match iters.kind() {
                ValueKind::List(lst) => lst.get(i).cloned().ok_or_else(|| internal(fn_name))?,
                _ => return Err(internal(fn_name)),
            };
            match interp.call_next(&iter, None) {
                Ok(v) => tuple.push(v),
                Err(e) if is_stop_iteration(&e) => {
                    tuple.push(fillvalue.clone());
                    *done = Value::bool_(true);
                    cleared_indices.push(i);
                    new_active -= 1;
                }
                Err(e) => return Err(e),
            }
        }
    }
    if !cleared_indices.is_empty() {
        let mut new_iters = match iters.kind() {
            ValueKind::List(lst) => lst.to_vec(),
            _ => return Err(internal(fn_name)),
        };
        for idx in cleared_indices {
            if let Some(slot) = new_iters.get_mut(idx) {
                *slot = Value::none();
            }
        }
        inst.borrow_mut()
            .attrs
            .insert("_iters", Value::list(new_iters));
    }
    {
        let mut attrs = inst.borrow_mut();
        attrs.attrs.insert("_active", Value::int(new_active));
        attrs.attrs.insert("_done", Value::list(new_done));
    }
    if new_active == 0 {
        return Err(PyError::named("StopIteration", String::new()));
    }
    Ok(Value::tuple(tuple))
}

pub(super) fn filterfalse_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    init_binary_func_iter(interp, args, fn_name, "filterfalse", "_pred", None)
}

pub(super) fn filterfalse_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let pred = attr_cloned(&inst, "_pred", fn_name)?;
    let iter = attr_cloned(&inst, "_iter", fn_name)?;
    loop {
        let item = interp.call_next(&iter, None)?;
        let falsy = if pred.is_none() {
            !interp.truthy_value(&item)?
        } else {
            let verdict = interp.call_function_expanded(
                pred.clone(),
                &[ExpandedCallArg {
                    name: None,
                    value: item.clone(),
                }],
            )?;
            !interp.truthy_value(&verdict)?
        };
        if falsy {
            return Ok(item);
        }
    }
}
