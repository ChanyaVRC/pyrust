use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: str(object='') / str(bytes_or_buffer[, encoding[, errors]]) — string constructor.
    /// <https://docs.python.org/3/library/functions.html#func-str>
    ///
    /// Migrated to the typed-signature dialect (#400): `#[arity_style(clinic)]`
    /// reproduces CPython 3.12's keyword-or-positional binding while granting
    /// positional `str(x)` the vectorcall fast entry.
    /// This can run user code by dispatching `__str__` and
    /// (as fallback) `__repr__` on user-defined objects.
    #[arity_style(clinic)]
    fn str(
        #[default(OptionalArg::Omitted)] object: OptValueRef,
        #[default(OptionalArg::Omitted)] encoding: OptValueRef,
        #[default(OptionalArg::Omitted)] errors: OptValueRef,
    ) -> Result<Value> {
        let object = object.as_value();
        let encoding = encoding.as_value();
        let errors = errors.as_value();

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
            if _interp.cold.runtime_config.bytes_warning > 0 {
                let bytes_warn_msg = match object.kind() {
                    ValueKind::Bytes(_) => Some("str() on a bytes instance"),
                    ValueKind::BuiltinObject { ops, .. } if ops.type_name() == "bytearray" => {
                        Some("str() on a bytearray instance")
                    }
                    _ => None,
                };
                if let Some(msg) = bytes_warn_msg {
                    crate::builtin_modules::warnings::emit_bytes_warning(_interp, msg)?;
                }
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
