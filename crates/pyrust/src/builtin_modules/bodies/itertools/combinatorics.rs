// Combinatoric `itertools` iterator entry points: `product`, `combinations`,
// `combinations_with_replacement`, and `permutations`.

use super::combinatoric_cursors;
use super::helpers::expect_self;
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

pub(super) fn product_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut positional: Vec<Value> = Vec::new();
    let mut repeat: i64 = 1;
    for a in &args[1..] {
        match a.name.as_deref() {
            Some("repeat") => {
                repeat = interp
                    .value_to_isize(&a.value, "Python int too large to convert to C ssize_t")?;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for product()"),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    if repeat < 0 {
        return Err(PyError::named(
            "ValueError",
            "repeat argument cannot be negative".to_string(),
        ));
    }
    let repeat =
        usize::try_from(repeat).map_err(|_| PyError::named("MemoryError", String::new()))?;
    let mut pools = combinatoric_cursors::reserve_product_pools(positional.len(), repeat)?;
    let single_pass: Vec<Value> = if repeat == 0 || positional.is_empty() {
        Vec::new()
    } else {
        let mut materialized = combinatoric_cursors::reserve_distinct_pools(positional.len())?;
        for value in &positional {
            materialized.push(Value::list(interp.collect_iterable(value)?));
        }
        materialized
    };
    if !single_pass.is_empty() {
        for _ in 0..repeat {
            pools.extend(single_pass.iter().cloned());
        }
    }
    inst.borrow_mut().attrs.insert_slot(
        "_cursor",
        combinatoric_cursors::product_cursor_value(pools)?,
    );
    Ok(Value::none())
}

pub(super) fn product_next(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    match combinatoric_cursors::next_product(&inst, fn_name)? {
        Some(tuple) => Ok(Value::tuple(tuple)),
        None => Err(PyError::named("StopIteration", String::new())),
    }
}

pub(super) fn init_combo_state(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    with_replacement: bool,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut positional: Vec<Value> = Vec::new();
    let mut iterable_kw: Option<Value> = None;
    let mut r_kw: Option<Value> = None;
    for a in &args[1..] {
        match a.name.as_deref() {
            Some("iterable") => iterable_kw = Some(a.value.clone()),
            Some("r") => r_kw = Some(a.value.clone()),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for {fn_name}()"),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    if positional.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at most 2 arguments ({} given)",
                positional.len()
            ),
        ));
    }
    if !positional.is_empty() && iterable_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            format!("argument for {fn_name}() given by name ('iterable') and position (1)"),
        ));
    }
    if positional.len() >= 2 && r_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            format!("argument for {fn_name}() given by name ('r') and position (2)"),
        ));
    }
    let iterable_val = iterable_kw
        .or_else(|| positional.first().cloned())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!("{fn_name}() missing required argument 'iterable' (pos 1)"),
            )
        })?;
    let r_val = r_kw.or_else(|| positional.get(1).cloned()).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument 'r' (pos 2)"),
        )
    })?;

    let pool = interp.collect_iterable(&iterable_val)?;
    let r = interp.value_to_isize(&r_val, "Python int too large to convert to C ssize_t")?;
    if r < 0 {
        return Err(PyError::named(
            "ValueError",
            "r must be non-negative".to_string(),
        ));
    }
    inst.borrow_mut().attrs.insert_slot(
        "_cursor",
        combinatoric_cursors::combinations_cursor_value(pool, r as usize, with_replacement)?,
    );
    Ok(Value::none())
}

pub(super) fn advance_combinations(
    args: &[ExpandedCallArg],
    fn_name: &str,
    with_replacement: bool,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    match combinatoric_cursors::next_combinations(&inst, fn_name, with_replacement)? {
        Some(tuple) => Ok(Value::tuple(tuple)),
        None => Err(PyError::named("StopIteration", String::new())),
    }
}

/// Slow path for `permutations(iterable, r)` when `r` is a BigInt or int subclass.
#[cold]
fn permutations_non_plain_r(interp: &mut Interpreter, value: &Value) -> Result<usize> {
    let non_negative = |r: i64| {
        if r < 0 {
            Err(PyError::named(
                "ValueError",
                "r must be non-negative".to_string(),
            ))
        } else {
            Ok(r as usize)
        }
    };
    match value.kind() {
        ValueKind::BigInt(_) => {
            let r = interp.value_to_isize(value, "Python int too large to convert to C ssize_t")?;
            non_negative(r)
        }
        ValueKind::PyInstance(_) => {
            let backing = crate::interpreter::coerce_subclass_backing(value, &[])
                .ok_or_else(|| PyError::named("TypeError", "Expected int as r".to_string()))?;
            match backing.kind() {
                ValueKind::Int(r) => non_negative(r),
                ValueKind::Bool(r) => Ok(r as usize),
                ValueKind::BigInt(_) => {
                    let r = interp
                        .value_to_isize(&backing, "Python int too large to convert to C ssize_t")?;
                    non_negative(r)
                }
                _ => Err(PyError::named("TypeError", "Expected int as r".to_string())),
            }
        }
        _ => Err(PyError::named("TypeError", "Expected int as r".to_string())),
    }
}

pub(super) fn permutations_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut positional: Vec<Value> = Vec::new();
    let mut iterable_kw: Option<Value> = None;
    let mut r_kw: Option<Value> = None;
    for a in &args[1..] {
        match a.name.as_deref() {
            Some("iterable") => iterable_kw = Some(a.value.clone()),
            Some("r") => r_kw = Some(a.value.clone()),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for permutations()"),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    if positional.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "permutations() takes at most 2 arguments ({} given)",
                positional.len()
            ),
        ));
    }
    if !positional.is_empty() && iterable_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for permutations() given by name ('iterable') and position (1)".to_string(),
        ));
    }
    if positional.len() >= 2 && r_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for permutations() given by name ('r') and position (2)".to_string(),
        ));
    }
    let iterable_val = iterable_kw
        .or_else(|| positional.first().cloned())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "permutations() missing required argument 'iterable' (pos 1)".to_string(),
            )
        })?;
    let r_opt = r_kw.or_else(|| positional.get(1).cloned());
    let pool: Vec<Value> = interp.collect_iterable(&iterable_val)?;
    let r = match r_opt.as_ref().map(|v| v.kind()) {
        None | Some(ValueKind::None) => pool.len(),
        Some(ValueKind::Int(r)) if r >= 0 => r as usize,
        Some(ValueKind::Int(_)) => {
            return Err(PyError::named(
                "ValueError",
                "r must be non-negative".to_string(),
            ));
        }
        Some(ValueKind::Bool(r)) => r as usize,
        Some(ValueKind::BigInt(_) | ValueKind::PyInstance(_)) => {
            permutations_non_plain_r(interp, r_opt.as_ref().unwrap())?
        }
        _ => {
            return Err(PyError::named("TypeError", "Expected int as r".to_string()));
        }
    };
    inst.borrow_mut().attrs.insert_slot(
        "_cursor",
        combinatoric_cursors::permutations_cursor_value(pool, r)?,
    );
    Ok(Value::none())
}

pub(super) fn permutations_next(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    match combinatoric_cursors::next_permutations(&inst, fn_name)? {
        Some(tuple) => Ok(Value::tuple(tuple)),
        None => Err(PyError::named("StopIteration", String::new())),
    }
}
