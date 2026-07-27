// Interpreter-aware adapters for `int.to_bytes` / `int.from_bytes`.

/// Validate `int.from_bytes(..., byteorder=...)` before resolving the byte
/// source, preserving CPython's error precedence.
fn check_from_bytes_byteorder(byteorder: Option<&Value>) -> Result<()> {
    let Some(v) = byteorder else { return Ok(()) };
    match v.kind() {
        ValueKind::Str(s) => {
            let s = s.to_string();
            if matches!(s.as_str(), "big" | "little") {
                Ok(())
            } else {
                Err(PyError::named(
                    "ValueError",
                    "byteorder must be either 'little' or 'big'".to_string(),
                ))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "from_bytes() argument 'byteorder' must be str, not {}",
                pyrust_core::builtin_type_name(v)
            ),
        )),
    }
}

impl Interpreter {
    fn materialize_from_bytes_source(&mut self, source: &Value) -> Result<Vec<u8>> {
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(source) {
            return Ok(data);
        }
        match source.kind() {
            ValueKind::Bytes(bytes) => Ok((**bytes).clone()),
            ValueKind::Str(_) | ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "cannot convert '{}' object to bytes",
                        pyrust_core::builtin_type_name(source),
                    ),
                ))
            }
            _ => {
                let type_name = pyrust_core::builtin_type_name(source).into_owned();
                let items = self.collect_iterable(source).map_err(|error| {
                    if error.class_name_is("TypeError") {
                        PyError::named(
                            "TypeError",
                            format!("cannot convert '{type_name}' object to bytes"),
                        )
                    } else {
                        error
                    }
                })?;
                items
                    .iter()
                    .map(|item| self.materialize_from_bytes_element(item))
                    .collect()
            }
        }
    }

    fn materialize_from_bytes_element(&mut self, value: &Value) -> Result<u8> {
        let resolved = self.value_to_index(value, |value| {
            PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    pyrust_core::builtin_type_name(value),
                ),
            )
        })?;
        match resolved.kind() {
            ValueKind::Int(value) if (0..=255).contains(&value) => Ok(value as u8),
            ValueKind::Bool(value) => Ok(value as u8),
            ValueKind::Int(_) | ValueKind::BigInt(_) => Err(PyError::named(
                "ValueError",
                "bytes must be in range(0, 256)".to_string(),
            )),
            _ => unreachable!("value_to_index returns only integer values"),
        }
    }

    /// Resolve the integer `length` argument of `int.to_bytes` through the
    /// `__index__` protocol when it is a `PyInstance` (issue #1929: an int
    /// subclass, e.g. `(255).to_bytes(I(2), "big")`, or a custom `__index__`
    /// object).  Other args (`byteorder` str, `signed` bool) and the
    /// no-instance fast path are left untouched so the receiver-side
    /// `pyrust_builtins::int::to_bytes` validation is unchanged.
    pub(super) fn resolve_to_bytes_length(
        &mut self,
        method: &str,
        pos: &mut [Value],
        kw: &mut PyDict,
    ) -> Result<()> {
        // `(5).from_bytes(src, ...)`: the bound-instance form reaches the
        // receiver-only `int::call`, which can't drive a user `__iter__`.
        // Resolve the bytes-like / iterable source to concrete bytes here
        // (where the interpreter is available), mirroring the class-method arm.
        if method == "from_bytes" {
            return self.resolve_from_bytes_source(pos, kw);
        }
        if method != "to_bytes" {
            return Ok(());
        }
        if let Some(first) = pos.first().cloned()
            && let Some(resolved) = self.try_resolve_index_value(&first)?
        {
            pos[0] = resolved;
        }
        // Keyword `length=` form.
        let length_key = PyKey::Str(Value::string("length"));
        if let Some(v) = kw.get(&length_key).cloned()
            && let Some(resolved) = self.try_resolve_index_value(&v)?
        {
            kw.insert(length_key, resolved);
        }
        Ok(())
    }

    /// Resolve the `bytes` source argument of `int.from_bytes` to concrete
    /// `bytes` in place, accepting any bytes-like object (bytes, bytearray,
    /// memoryview) or any iterable of ints in `0..=255`. The source is the
    /// first positional arg, or the `bytes` keyword. Shared by both the
    /// class-method (`int.from_bytes(...)`) and bound-instance
    /// (`(5).from_bytes(...)`) dispatch paths.
    pub(super) fn resolve_from_bytes_source(
        &mut self,
        pos: &mut [Value],
        kw: &mut PyDict,
    ) -> Result<()> {
        // CPython validates `byteorder` *before* it processes the bytes source,
        // so a bad/non-str byteorder must win over a bad source. Resolving the
        // source first (it can raise for a str / out-of-range element / bad
        // iterable) would otherwise surface the wrong error. Pre-check the
        // byteorder argument here so its error takes precedence, mirroring the
        // receiver-only `int_from_bytes` messages.
        check_from_bytes_byteorder(pos.get(1).or_else(|| kw.get(&PyKey::str_from("byteorder"))))?;
        if let Some(src) = pos.first() {
            if !matches!(src.kind(), ValueKind::Bytes(_)) {
                let src = src.clone();
                let resolved = self.materialize_from_bytes_source(&src)?;
                pos[0] = Value::bytes(resolved);
            }
        } else if let Some(src) = kw.get(&PyKey::str_from("bytes")).cloned()
            && !matches!(src.kind(), ValueKind::Bytes(_))
        {
            let resolved = self.materialize_from_bytes_source(&src)?;
            kw.insert(PyKey::str_from("bytes"), Value::bytes(resolved));
        }
        Ok(())
    }

    /// If `v` is a `PyInstance` resolvable as an integer — either an int/bool
    /// subclass (use the backing int, which wins over any `__index__` since the
    /// object already *is* an int) or an object defining `__index__` (call it)
    /// — return the resolved integer.  Returns `Ok(None)` for any non-instance
    /// value and for instances that are not integer-like, so the caller leaves
    /// the original value in place and the receiver-side validation raises the
    /// canonical TypeError.
    ///
    /// Routes the actual `__index__` dispatch through
    /// [`Interpreter::try_value_to_index`], whose `None` result distinguishes
    /// "this instance isn't integer-like" from a real error raised inside
    /// `__index__`.
    fn try_resolve_index_value(&mut self, v: &Value) -> Result<Option<Value>> {
        if !matches!(v.kind(), ValueKind::PyInstance(_)) {
            return Ok(None);
        }
        self.try_value_to_index(v)
    }
}
