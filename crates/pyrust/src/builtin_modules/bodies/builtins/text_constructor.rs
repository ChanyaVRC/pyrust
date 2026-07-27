use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: str(object='') — string constructor.
    /// <https://docs.python.org/3/library/functions.html#func-str>
    /// This can run user code by dispatching `__str__` and
    /// (as fallback) `__repr__` on user-defined objects.
    fn str(args) -> Result<Value> {
        // CPython 3.12: str(object='', encoding='utf-8', errors='strict') —
        // all three parameters are keyword-or-positional.
        let bound = bind_constructor_kwargs(
            FN_NAME,
            args,
            &["object", "encoding", "errors"],
            &[true, true, true],
            3,
        )?;
        let object = &bound[0];
        let encoding = &bound[1];
        let errors = &bound[2];

        // No object → empty string, regardless of encoding/errors (CPython:
        // `str(encoding='utf-8') == ''`).
        let Some(object) = object else {
            return Ok(Value::string(String::new()));
        };

        // The decoding form is selected when *either* encoding or errors is
        // supplied; otherwise this is the plain `str(object)` form.
        if encoding.is_none() && errors.is_none() {
            // Scalar fast path (#alloc): `str(int)` formats the digits straight
            // into the string Value via a stack buffer — one allocation instead
            // of the intermediate heap `String` that `render_instance_str`
            // returns before `Value::string` copies it.
            if let ValueKind::Int(n) = object.kind() {
                return Ok(Value::int_string(n));
            }
            return Ok(Value::string(render_instance_str(_interp, object)?));
        }

        // str(object, encoding[, errors]) — bytes-to-string decoding form.
        let bytes = match object.kind() {
            ValueKind::Bytes(rc) => rc.as_slice().to_vec(),
            ValueKind::Str(_) => {
                return Err(PyError::named(
                    "TypeError",
                    "decoding str is not supported".to_string(),
                ));
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decoding to str: need a bytes-like object, {} found",
                        pyrust_core::builtin_type_name(object)
                    ),
                ));
            }
        };
        let encoding = match encoding {
            Some(e) => match e.kind() {
                ValueKind::Str(s) => s.to_owned(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument 2 (encoding) must be a str"),
                    ));
                }
            },
            None => "utf-8".to_owned(),
        };
        let errors = match errors {
            Some(e) => match e.kind() {
                ValueKind::Str(s) => s.to_owned(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument 3 (errors) must be a str"),
                    ));
                }
            },
            None => "strict".to_owned(),
        };
        pyrust_builtins::bytes::decode_bytes(&bytes, &encoding, &errors)
    }

}
