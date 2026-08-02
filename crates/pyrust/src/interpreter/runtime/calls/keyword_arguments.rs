/// CPython-style callable name used in duplicate-keyword diagnostics.
///
/// This is deliberately part of the call runtime: collection-key code reports
/// the collision, while call code decides how the callee is presented.
pub(super) fn callable_error_name(callee: &Value) -> Option<String> {
    let function = match callee.kind() {
        ValueKind::UserFunction(function) => function.clone(),
        ValueKind::BoundMethod { function, .. } | ValueKind::ClassBoundMethod { function, .. } => {
            function.clone()
        }
        ValueKind::PyClass(class) => {
            let class = class.borrow();
            let qualname = class.qualname.clone();
            let module = class
                .attrs
                .get("__module__")
                .and_then(|value| value.as_str().map(str::to_owned));
            return match module.as_deref() {
                Some(module) if !module.is_empty() && module != "builtins" => {
                    Some(format!("{module}.{qualname}"))
                }
                _ => Some(qualname),
            };
        }
        ValueKind::BuiltinFunction(name) => return Some(name.to_string()),
        _ => return None,
    };

    let qualname = function.effective_qualname();
    match function.module_value().as_str() {
        Some(module) if !module.is_empty() && module != "builtins" => {
            Some(format!("{module}.{qualname}"))
        }
        _ => Some(qualname),
    }
}

/// Build the duplicate-keyword error emitted by call mapping expansion.
pub(super) fn duplicate_keyword_error(func_name: Option<&str>, keyword: &str) -> PyError {
    match func_name {
        Some(name) => pyrust_core::type_err!(
            "{}() got multiple values for keyword argument '{}'",
            name,
            keyword
        ),
        None => pyrust_core::type_err!("got multiple values for keyword argument '{}'", keyword),
    }
}

pub(crate) enum KwCallMappingMerge {
    Complete,
    Duplicate(String),
    NotMapping,
}

/// Build the call-specific error for a non-mapping `**` operand.
pub(super) fn keyword_mapping_operand_error(func_name: Option<&str>, source: &Value) -> PyError {
    match func_name {
        Some(name) => pyrust_core::type_err!(
            "{}() argument after ** must be a mapping, not {}",
            name,
            value_type_name_str(source)
        ),
        None => {
            pyrust_core::type_err!("'{}' object is not a mapping", value_type_name_str(source))
        }
    }
}

/// Replace only the generic non-iterable error from a call's `*` operand.
/// The callee-name callback runs exclusively on that error path.
pub(super) fn star_operand_error(
    source: &Value,
    error: PyError,
    func_name: impl FnOnce() -> Option<String>,
) -> PyError {
    let type_name = value_type_name_str(source);
    let generic_message = format!("'{type_name}' object is not iterable");
    let is_non_iterable = matches!(
        &error,
        PyError::Named(class_name, message)
            if class_name.as_ref() == "TypeError" && message == &generic_message
    );
    let Some(name) = is_non_iterable.then(func_name).flatten() else {
        return error;
    };
    pyrust_core::type_err!(
        "{}() argument after * must be an iterable, not {}",
        name,
        type_name
    )
}

fn keyword_name(key: &PyKey) -> String {
    match key {
        PyKey::Str(value) => value.as_str().unwrap_or_default().to_owned(),
        other => format!("{other:?}"),
    }
}

impl Interpreter {
    /// Materialise a call's `*` operand and add callee context only when the
    /// generic iterator path proves that the operand is not iterable.
    pub(crate) fn collect_call_splat(
        &mut self,
        callee: &Value,
        source: &Value,
    ) -> Result<Vec<Value>> {
        match self.collect_iterable(source) {
            Ok(items) => Ok(items),
            Err(error) => Err(star_operand_error(source, error, || {
                callable_error_name(callee)
            })),
        }
    }

    /// Append a call's `**mapping` entries to the general expanded-argument
    /// buffer. Mapping protocol semantics come from `collection_ops`; this
    /// call-specific adapter owns keyword validation and diagnostics.
    pub(crate) fn expand_kwargs_into(
        &mut self,
        callee: &Value,
        kwargs: &Value,
        buffer: &mut Vec<ExpandedCallArg>,
    ) -> Result<()> {
        let pairs = mapping_entries_for_expansion(self, kwargs)?.ok_or_else(|| {
            let name = callable_error_name(callee);
            keyword_mapping_operand_error(name.as_deref(), kwargs)
        })?;
        for (key, value) in pairs {
            match key {
                PyKey::Str(name) => buffer.push(ExpandedCallArg {
                    name: Some(name.as_str().unwrap_or_default().to_owned()),
                    value,
                }),
                _ => return Err(pyrust_core::type_err!("keywords must be strings")),
            }
        }
        Ok(())
    }

    /// Expand and merge one `**mapping` call operand.
    pub(crate) fn merge_kwcall_mapping(
        &mut self,
        receiver: &Value,
        source: &Value,
    ) -> Result<KwCallMappingMerge> {
        let Some(pairs) = mapping_entries_for_expansion(self, source)? else {
            return Ok(KwCallMappingMerge::NotMapping);
        };
        Ok(match self.dict_merge_kwcall(receiver, pairs)? {
            Some(keyword) => KwCallMappingMerge::Duplicate(keyword),
            None => KwCallMappingMerge::Complete,
        })
    }

    /// Insert one named call operand after validating its internal string key.
    pub(crate) fn set_kwcall_value(
        &mut self,
        receiver: &Value,
        key: &Value,
        value: Value,
    ) -> Result<Option<String>> {
        let key = key.to_key().ok_or_else(|| {
            PyError::Runtime("internal: non-hashable keyword argument key".to_string())
        })?;
        self.dict_setitem_kwcall(receiver, key, value)
    }

    /// Merge a `**mapping` into call kwargs, stopping at the first duplicate.
    pub(crate) fn dict_merge_kwcall(
        &mut self,
        receiver: &Value,
        pairs: Vec<(PyKey, Value)>,
    ) -> Result<Option<String>> {
        for (key, value) in pairs {
            if self.dict_lookup(receiver, &key)?.is_some() {
                return Ok(Some(keyword_name(&key)));
            }
            receiver
                .dict_with_mut(|dict| dict.insert(key, value))
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        }
        Ok(None)
    }

    /// Insert one named argument, reporting a collision without overwriting.
    pub(crate) fn dict_setitem_kwcall(
        &mut self,
        receiver: &Value,
        key: PyKey,
        value: Value,
    ) -> Result<Option<String>> {
        if self.dict_lookup(receiver, &key)?.is_some() {
            return Ok(Some(keyword_name(&key)));
        }
        receiver
            .dict_with_mut(|dict| dict.insert(key, value))
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        Ok(None)
    }
}
