/// `str.encode(encoding='utf-8', errors='strict')`
///
/// Positional args have the same semantics as keyword args; the caller
/// (`str_merge_kwargs`) normalises keyword forms into positional slots
/// before this function is reached.
fn str_encode(s: &str, args: &[Value]) -> Result<Value> {
    if args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("encode() takes at most 2 arguments ({} given)", args.len()),
        ));
    }
    let encoding: &str = match args.first() {
        None => "utf-8",
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "encode() argument 'encoding' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };
    let errors: &str = match args.get(1) {
        None => "strict",
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "encode() argument 'errors' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };
    encode_str_to_bytes(s, encoding, errors)
}
