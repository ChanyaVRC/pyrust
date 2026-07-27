/// Validate attribute names for `getattr` / `setattr` / `hasattr` /
/// `delattr`, including `str` subclasses and CPython's shared error wording.
fn attr_name_arg(name: &Value) -> Result<String> {
    if is_str_or_str_subclass(name) {
        Ok(extract_str_value(name))
    } else {
        let type_name = value_type_name_str(name);
        Err(PyError::named(
            "TypeError",
            format!("attribute name must be string, not '{type_name}'"),
        ))
    }
}
