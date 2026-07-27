/// Acquire and drain an iterator under the error policy shared by
/// `str.join`, `bytes.join`, and `bytearray.join`.
///
/// CPython rewrites any TypeError raised while obtaining/validating the
/// iterator to "can only join an iterable". Once acquisition succeeds,
/// exceptions from `__next__` must pass through unchanged.
fn collect_join_iterable(interp: &mut Interpreter, iterable: &Value) -> Result<Vec<Value>> {
    let iterator = make_iterator(interp, iterable).map_err(|error| {
        if error.class_name_is("TypeError") {
            pyrust_core::type_err!("can only join an iterable")
        } else {
            error
        }
    })?;
    interp.collect_iterator(&iterator)
}
