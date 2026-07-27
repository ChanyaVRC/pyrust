// Macro adapters shared by combinations and combinations_with_replacement.

/// Decode `permutations(iterable, r)` with CPython 3.12's deliberately
/// narrower rule: `r` must already be an int (including bool/int subclasses).
/// Unlike the other counted itertools constructors, arbitrary `__index__`
/// objects are not accepted and their method is not invoked.
///
/// Slow path for the two integer representations that need more than a tag
/// check. Plain int/bool handling stays in the registration owner.
#[cold]
fn permutations_non_plain_r(interp: &mut crate::Interpreter, value: &Value) -> Result<usize> {
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

fn init_combo_state(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    with_replacement: bool,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded(fn_name, user)?;
    if user.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument 'iterable' (pos 1)"),
        ));
    }
    if user.len() == 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument 'r' (pos 2)"),
        ));
    }
    if user.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at most 2 arguments ({} given)",
                user.len()
            ),
        ));
    }

    let pool = interp.collect_iterable(&user[0].value)?;
    let r = interp.value_to_isize(
        &user[1].value,
        "Python int too large to convert to C ssize_t",
    )?;
    if r < 0 {
        return Err(PyError::named(
            "ValueError",
            "r must be non-negative".to_string(),
        ));
    }
    inst.borrow_mut().attrs.insert(
        "_cursor",
        combinatoric_cursors::combinations_cursor_value(pool, r as usize, with_replacement)?,
    );
    Ok(Value::none())
}

fn advance_combinations(
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
