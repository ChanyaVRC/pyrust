// Comparison adapter used by `cmp_to_key`.

fn cmp_key_compare(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    op: BinaryOp,
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name} expected 1 argument, got {}", user.len()),
        ));
    }
    let other_obj = match user[0].value.kind() {
        ValueKind::PyInstance(other) => other.borrow().attrs.get("obj").cloned(),
        _ => None,
    };
    let Some(other_obj) = other_obj else {
        return Ok(Value::not_implemented());
    };
    let (cmp, self_obj) = {
        let borrow = inst.borrow();
        (
            borrow
                .attrs
                .get("_cmp")
                .cloned()
                .ok_or_else(|| internal(fn_name))?,
            borrow
                .attrs
                .get("obj")
                .cloned()
                .ok_or_else(|| internal(fn_name))?,
        )
    };
    let result = interp.call_function_expanded(
        cmp,
        &[
            ExpandedCallArg {
                name: None,
                value: self_obj,
            },
            ExpandedCallArg {
                name: None,
                value: other_obj,
            },
        ],
    )?;
    let comparison = interp.eval_binary(result, op, Value::int(0))?;
    // CPython's KeyWrapper uses PyObject_RichCompare, whose result is an
    // arbitrary Python object.  Direct rich comparisons expose that object;
    // only a consuming boolean context decides whether to truth-test it.
    Ok(comparison)
}
