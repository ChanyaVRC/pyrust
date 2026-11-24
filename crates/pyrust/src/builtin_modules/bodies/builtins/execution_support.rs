/// CPython injects `__builtins__` into a caller-supplied globals dict the first
/// time `eval()` or `exec()` is called with it (see `PyEval_EvalCode`).  If the
/// dict does not already contain `"__builtins__"`, insert the builtins module.
///
/// Existing values (including `{}` as a deliberate override) are left alone.
fn inject_builtins_into_globals(globals: &Value) {
    use crate::value::StrKey;
    let already_present = globals
        .dict_with(|d| d.contains_key(&StrKey("__builtins__")))
        .unwrap_or(true); // not a dict — leave it alone
    if !already_present {
        let builtins = crate::interpreter::cached_builtins_module();
        let _ = globals.dict_insert(PyKey::str_from("__builtins__"), builtins);
    }
}
