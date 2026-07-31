// Interpreter-aware __format__ and field-access dispatch.
///
/// Implements `str.format()`.  Parses `{...}` replacement fields in `template`
/// and substitutes positional or keyword arguments, optionally formatted by
/// a `:spec` and/or converted by `!r`/`!s`/`!a`.  Supports `{{` / `}}` for
/// literal braces and `{0.attr}` / `{0[key]}` field accessors.
impl Interpreter {
    /// Fast path for an f-string interpolation with no `!r/!s/!a` conversion and
    /// no format spec: equivalent to `format(value, "")` but without the
    /// `format` global lookup or the generic call frame (issue #1926, mirrors
    /// CPython's FORMAT_VALUE).
    ///
    /// For a non-`PyInstance` value, `format(value, "")` is exactly
    /// `apply_format_spec(value, "")`, i.e. `str(value)` — computed inline here.
    /// For the rarer `PyInstance` case (which may define a custom
    /// `__format__`/`__str__`), we delegate to the real `format` builtin so the
    /// dispatch is byte-for-byte identical to the call-based lowering.
    pub(crate) fn format_value_default(&mut self, value: &Value) -> Result<Value> {
        // Fast path (#alloc): a bare `{i}` field for an int (no format spec) is
        // `str(i)` — format the digits directly into the string Value, one
        // allocation instead of the int→`String`→`Value::string` pair the
        // generic `apply_format_spec("")` path takes.
        if let ValueKind::Int(n) = value.kind() {
            return Ok(Value::int_string(n));
        }
        if matches!(value.kind(), ValueKind::PyInstance(_)) {
            return self.call_function_expanded(
                Value::builtin_function("format"),
                &[
                    ExpandedCallArg {
                        name: None,
                        value: value.clone(),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: Value::string(""),
                    },
                ],
            );
        }
        // Issue #2936: `str(mappingproxy)` is `str(proxied)`, which needs
        // interpreter dispatch when the proxied object is a dict subclass
        // instance.  Only owner-carrying proxies are affected, and the
        // `BuiltinObject` test keeps every other kind on the path below.
        if value.is_builtin_object() && pyrust_builtins::mapping_proxy::owner_of(value).is_some() {
            return Ok(Value::string(render_instance_str(self, value)?));
        }
        // Issue #2771: a bare `{cls}` field is `format(cls, "")`, which runs
        // `type(cls).__format__(cls, "")` — a metaclass `__format__` override
        // wins, otherwise the inherited `object.__format__` returns `str(cls)`
        // (honouring a metaclass `__str__`/`__repr__`).  Only classes with a
        // custom metatype can carry such an override, so plain classes keep the
        // fast `apply_format_spec` path below.
        if let ValueKind::PyClass(cls_rc) = value.kind()
            && cls_rc.borrow().metatype.is_some()
        {
            return self.dispatch_dunder_format(value, "");
        }
        // Non-instance: empty spec == str(value).
        apply_format_spec(value, "")
    }

    /// Format an f-string value through the bytecode site's parsed-spec cache.
    ///
    /// Execution supplies only the cache slot. This domain owns the distinction
    /// between user `__format__`, custom-metaclass dispatch, and the
    /// representation-specialized renderer.
    pub(crate) fn format_value_spec_cached(
        &mut self,
        value: &Value,
        spec_value: &Value,
        cache: &RefCell<Vec<FmtSpecCacheEntry>>,
        site: usize,
    ) -> Result<Value> {
        let spec = spec_value.as_str().unwrap_or("");
        let class_with_custom_metaclass =
            matches!(value.kind(), ValueKind::PyClass(class) if class.borrow().metatype.is_some());
        let owner_carrying_mappingproxy = spec.is_empty()
            && value.is_builtin_object()
            && pyrust_builtins::mapping_proxy::owner_of(value).is_some();
        if matches!(value.kind(), ValueKind::PyInstance(_))
            || class_with_custom_metaclass
            || owner_carrying_mappingproxy
        {
            return self.dispatch_dunder_format(value, spec);
        }
        apply_format_spec_cached(value, spec_value, cache, site)
    }

    /// Dispatch `__format__(spec)` for a value, validating that the result is a
    /// `str`.  Mirrors the logic in the `format()` builtin (#1370).
    ///
    /// For `PyInstance`:
    ///   1. Look up `__format__` in the MRO.  If found and it is a user-defined
    ///      function (not the object builtin), call it and check the return is
    ///      `str`; if not, raise `TypeError`.
    ///   2. No user `__format__`: if there is backing primitive data, apply
    ///      `apply_format_spec` to the backing value.
    ///   3. Pure user class with neither: empty spec → `__str__` via
    ///      `render_value_as_str`; non-empty spec → `TypeError` (matching
    ///      CPython's `object.__format__` behaviour).
    ///
    /// For all other value kinds: delegate straight to `apply_format_spec`.
    pub(crate) fn dispatch_dunder_format(&mut self, value: &Value, spec: &str) -> Result<Value> {
        // Issue #2771: `format(cls, spec)` runs `type(cls).__format__`, which is
        // the inherited `object.__format__`: empty spec returns `str(cls)`
        // (now honouring a metaclass `__str__`/`__repr__`), a non-empty spec
        // raises `TypeError` naming the *metaclass* (`type(cls).__name__`).
        // CPython names the metaclass regardless of whether it overrides
        // `__repr__`/`__str__`, so intercept here for any class carrying a
        // custom metatype.  A plain class (metatype is the built-in `type`)
        // falls through to `apply_format_spec`, which renders the default
        // `<class '...'>` form and already raises `type.__format__`.
        if let ValueKind::PyClass(cls_rc) = value.kind() {
            let has_custom_meta = cls_rc.borrow().metatype.is_some();
            if has_custom_meta {
                let cls_rc = Rc::clone(cls_rc);
                // A metaclass `__format__` override wins outright — CPython runs
                // `type(cls).__format__(cls, spec)` for *any* spec, not just the
                // inherited `object.__format__`.
                if let Some(method_val) =
                    crate::interpreter::metaclass_dunder(&cls_rc, "__format__")
                {
                    let result = invoke_class_method(
                        self,
                        method_val,
                        Value::py_class(Rc::clone(&cls_rc)),
                        &[ExpandedCallArg {
                            name: None,
                            value: Value::string(spec),
                        }],
                    )?;
                    return if is_str_or_str_subclass(&result) {
                        Ok(result)
                    } else {
                        Err(pyrust_core::type_err!(
                            "__format__ must return a str, not {}",
                            value_type_name_str(&result),
                        ))
                    };
                }
                // No metaclass `__format__`: inherited `object.__format__` —
                // empty spec returns `str(cls)` (honouring a metaclass
                // `__str__`/`__repr__`), a non-empty spec raises `TypeError`
                // naming the metaclass regardless of any repr/str override.
                if spec.is_empty() {
                    return Ok(Value::string(self.render_value_as_str(value)?));
                }
                let meta = crate::interpreter::metaclass_of(&cls_rc);
                let meta_name = meta.borrow().name.clone();
                return Err(pyrust_core::type_err!(
                    "unsupported format string passed to {meta_name}.__format__"
                ));
            }
        }
        // Issue #2936: `mappingproxy` inherits `object.__format__`, so an empty
        // spec is `str(proxy)` — which CPython defines as `str(proxied)` and
        // which needs interpreter dispatch for a dict-subclass owner.  A
        // non-empty spec still raises through `apply_format_spec` below.
        if spec.is_empty()
            && value.is_builtin_object()
            && pyrust_builtins::mapping_proxy::owner_of(value).is_some()
        {
            return Ok(Value::string(render_instance_str(self, value)?));
        }
        let ValueKind::PyInstance(inst) = value.kind() else {
            return apply_format_spec(value, spec);
        };
        let inst_rc = Rc::clone(inst);
        let class = Rc::clone(&inst_rc.borrow().class);
        if let Some(method_val) = lookup_class_attr(&class, "__format__") {
            // Only dispatch to user-defined __format__, not the object builtin.
            if !matches!(method_val.kind(), ValueKind::BuiltinFunction(_)) {
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(spec),
                    }],
                )?;
                return if is_str_or_str_subclass(&result) {
                    Ok(result)
                } else {
                    Err(pyrust_core::type_err!(
                        "__format__ must return a str, not {}",
                        value_type_name_str(&result),
                    ))
                };
            }
        }
        // No user __format__ in MRO (or only the object builtin).
        if let Some(backing) = builtin_data_backing(value) {
            // `object.__format__(self, "")` is defined as `str(self)` (#2386).
            // For subclasses whose `str()` differs from the backing's `str()`
            // — set/frozenset/bytearray, which prefix the class name — the
            // *instance* must be rendered, not the backing, so `f"{S({1})}"` ==
            // `str(S({1}))` == "S({1})", and `f"{BA(b'ab')}"` == "BA(b'ab')".
            // For scalar backings (int/str/float/bytes) `str(value)` already
            // equals `str(backing)`, so routing empty specs here is a no-op for
            // them.  A non-empty spec falls through to `apply_format_spec_named`
            // (the format mini-language / unsupported-spec TypeError).
            if spec.is_empty() {
                return Ok(Value::string(self.render_value_as_str(value)?));
            }
            // CPython names the *actual* subclass in an unsupported-spec
            // TypeError, not the backing primitive (`B.__format__`, not
            // `bytes.__format__`), so thread the receiver's own type name
            // through to `apply_format_spec` (issue #2212).
            let owner = value_type_name_str(value);
            return apply_format_spec_named(&backing, spec, Some(&owner));
        }
        // Pure user class with no custom __format__ and no backing data.
        if spec.is_empty() {
            Ok(Value::string(self.render_value_as_str(value)?))
        } else {
            let type_name = value_type_name_str(value);
            Err(pyrust_core::type_err!(
                "unsupported format string passed to {}.__format__",
                type_name
            ))
        }
    }
} // impl Interpreter

/// Splits a *terminated* replacement field into `(field_name, conversion,
/// format_spec)`, mirroring CPython 3.12's `field_name_split` / `parse_field`.
///
/// Scans for the first `!` or `:` at bracket depth 0 (so `!`/`:` inside a `[…]`
/// subscript stay part of the field name). A `:` starts the format spec with no
/// conversion. A `!` ends the field name and the single following char is the
/// conversion flag; the char after that must be the field end or a `:`.
///
/// Returns `Err(msg)` with CPython's exact `ValueError` text for a malformed
/// conversion (`{x!}`, `{x!ab}`, `{x!r!s}`), so the caller can defer it as a
/// render-time error in field order.
fn split_field_conv_spec(
    field: &str,
) -> std::result::Result<(&str, Option<char>, &str), &'static str> {
    let bytes = field.as_bytes();
    let mut bracket_depth = 0;
    for (idx, b) in bytes.iter().enumerate() {
        match b {
            b'[' => bracket_depth += 1,
            b']' if bracket_depth > 0 => bracket_depth -= 1,
            b':' if bracket_depth == 0 => return Ok((&field[..idx], None, &field[idx + 1..])),
            b'!' if bracket_depth == 0 => {
                let name = &field[..idx];
                // The single char after '!' is the conversion flag. CPython
                // takes whatever byte follows verbatim (even ':'), so we read a
                // full UTF-8 char rather than splitting on ':' first.
                let after = &field[idx + 1..];
                let Some(conv) = after.chars().next() else {
                    // `{x!}` — '!' with no conversion char before the field end.
                    return Err("unmatched '{' in format spec");
                };
                let rest = &after[conv.len_utf8()..];
                // After the conversion char CPython expects end-of-field or ':'.
                if rest.is_empty() {
                    return Ok((name, Some(conv), ""));
                }
                if let Some(spec) = rest.strip_prefix(':') {
                    return Ok((name, Some(conv), spec));
                }
                return Err("expected ':' after conversion specifier");
            }
            _ => {}
        }
    }
    Ok((field, None, ""))
}

/// Splits a field name like `0.x[1].y` into `("0", ".x[1].y")`.
fn split_head_and_accessors(name: &str) -> (&str, &str) {
    let bytes = name.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'.' || *b == b'[' {
            return (&name[..i], &name[i..]);
        }
    }
    (name, "")
}

/// Applies a chain of `.attr` / `[key]` accessors to a value.
fn apply_field_accessors(
    interp: &mut Interpreter,
    mut value: Value,
    mut rest: &str,
) -> Result<Value> {
    while !rest.is_empty() {
        let bytes = rest.as_bytes();
        if bytes[0] == b'.' {
            // Find next '.' or '['
            let end = bytes[1..]
                .iter()
                .position(|&b| b == b'.' || b == b'[')
                .map(|i| i + 1)
                .unwrap_or(rest.len());
            let attr = &rest[1..end];
            rest = &rest[end..];
            // Dispatch getattr through the full attribute resolution path so
            // that built-in types (int, float, complex, …) work the same way
            // `getattr(value, attr)` does — not just PyInstance values (#1031).
            value = interp.get_attr(&value, attr)?;
        } else if bytes[0] == b'[' {
            let end = bytes
                .iter()
                .position(|&b| b == b']')
                .ok_or_else(|| pyrust_core::value_err!("expected '}}' before end of string"))?;
            let key_str = &rest[1..end];
            rest = &rest[end + 1..];
            // Per CPython 3.12: a subscript that parses as a non-negative
            // integer is passed as `int` to __getitem__; anything else
            // (non-numeric or negative like "-1") is passed as `str`.
            // CPython rejects numbers > i64::MAX with "Too many decimal
            // digits in format string" (matching CPython's internal
            // Py_ssize_t overflow check in _PyObject_GetMethod).
            let key = if let Ok(idx) = key_str.parse::<u64>() {
                if idx > i64::MAX as u64 {
                    return Err(pyrust_core::value_err!(
                        "Too many decimal digits in format string"
                    ));
                }
                Value::int(idx as i64)
            } else {
                Value::string(key_str)
            };
            value = interp.eval_index(&value, key)?;
        } else {
            return Err(pyrust_core::value_err!(
                "unexpected character in format field: '{}'",
                &rest[..1]
            ));
        }
    }
    Ok(value)
}

/// Expands `{field_name}` references within a format spec string (the part
/// after `:` in a replacement field).  Per PEP 3101, only one level of
/// nesting is allowed — the inner fields cannot have a conversion or a
/// further nested spec.
///
/// `auto_idx` and `saw_manual` are the same counters used by the enclosing
/// `format_str_template` call; the spec fields advance the same auto-number
/// sequence as top-level fields.
fn expand_format_spec_positional(
    spec: &str,
    positional: &[Value],
    keyword: &[(&str, Value)],
    auto_idx: &mut Option<usize>,
    saw_manual: &mut bool,
) -> Result<String> {
    let bytes = spec.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' if i + 1 < bytes.len() && bytes[i + 1] == b'{' => {
                out.push('{');
                i += 2;
            }
            b'}' if i + 1 < bytes.len() && bytes[i + 1] == b'}' => {
                out.push('}');
                i += 2;
            }
            b'{' => {
                // Find the matching '}'.  No further nesting is allowed inside
                // a format spec's inner field.
                let start = i + 1;
                let end = bytes[start..]
                    .iter()
                    .position(|&b| b == b'}')
                    .ok_or_else(|| {
                        pyrust_core::value_err!(
                            "Single '{' encountered in format string".to_string()
                        )
                    })?
                    + start;
                // PEP 3101: inner fields cannot have a nested spec; if the user
                // wrote `{name:spec}` inside a format spec, CPython treats
                // everything before `:` as the field name.
                let inner_raw = &spec[start..end];
                i = end + 1;
                let inner = inner_raw
                    .split_once(':')
                    .map(|(name, _)| name)
                    .unwrap_or(inner_raw);

                // Inner fields do not support '!' conversion or nested ':' spec.
                let value = if inner.is_empty() {
                    // Auto-numbered
                    if *saw_manual {
                        return Err(pyrust_core::value_err!(
                            "cannot switch from manual field specification to automatic field numbering"
                        ));
                    }
                    let Some(idx) = *auto_idx else { unreachable!() };
                    *auto_idx = Some(idx + 1);
                    positional.get(idx).cloned().ok_or_else(|| {
                        pyrust_core::index_err!(
                            "Replacement index {idx} out of range for positional args tuple"
                        )
                    })?
                } else if let Ok(n) = inner.parse::<usize>() {
                    if auto_idx.is_some() && *auto_idx != Some(0) {
                        return Err(pyrust_core::value_err!(
                            "cannot switch from automatic field numbering to manual field specification"
                        ));
                    }
                    *saw_manual = true;
                    *auto_idx = None;
                    positional.get(n).cloned().ok_or_else(|| {
                        pyrust_core::index_err!(
                            "Replacement index {n} out of range for positional args tuple"
                        )
                    })?
                } else {
                    keyword
                        .iter()
                        .find(|(k, _)| *k == inner)
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| PyError::key_error(Value::string(inner)))?
                };
                out.push_str(&value.to_py_str());
            }
            b'}' => {
                return Err(pyrust_core::value_err!(
                    "Single '}' encountered in format string".to_string()
                ));
            }
            _ => {
                let ch_start = i;
                i += 1;
                while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                    i += 1;
                }
                out.push_str(&spec[ch_start..i]);
            }
        }
    }
    Ok(out)
}
