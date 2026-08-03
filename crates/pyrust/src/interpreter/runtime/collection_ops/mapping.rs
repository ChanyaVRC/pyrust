/// Operand type name for `|` TypeError messages. A `mappingproxy` reports as
/// `dict` because its `__or__` / `__ror__` slots are `dict.__or__` /
/// `dict.__ror__` in CPython 3.12, so a failed merge names the operand `dict`.
pub(super) fn bitor_operand_type_name(v: &Value) -> std::borrow::Cow<'static, str> {
    if is_mapping_proxy(v) {
        std::borrow::Cow::Borrowed("dict")
    } else {
        crate::interpreter::error_type_name_str(v)
    }
}

/// True if `v` is a `mappingproxy` (either class- or dict-backed).
pub(super) fn is_mapping_proxy(v: &Value) -> bool {
    pyrust_builtins::mapping_proxy::is_mapping_proxy(v)
}

/// Extract key-value pairs from a plain `dict`, a dict-backed subclass, an
/// instance `__dict__`, or a `mappingproxy`. Used by PEP 584 dict algebra and
/// descriptor validation.
pub(super) fn dict_entries_from_value(v: &Value) -> Option<Vec<(PyKey, Value)>> {
    if let Some(entries) = v.dict_with(|d| {
        d.iter()
            .map(|(k, val)| (k.clone(), val.clone()))
            .collect::<Vec<_>>()
    }) {
        return Some(entries);
    }
    if let Some(entries) = pyrust_builtins::instance_dict::as_instance_dict_items(v) {
        return Some(entries);
    }
    if let Some(cls_rc) = pyrust_builtins::mapping_proxy::as_class_rc(v) {
        return Some(
            cls_rc
                .borrow()
                .attrs
                .iter()
                .map(|(k, val)| (PyKey::str_from(k), val.clone()))
                .collect(),
        );
    }
    if let Some(dict_rc) = pyrust_builtins::mapping_proxy::as_dict_rc(v) {
        return Some(dict_rc.borrow().clone().into_iter().collect());
    }
    if let Some(backing) = builtin_data_backing(v) {
        return dict_entries_from_value(&backing);
    }
    None
}

/// Clone an existing native dict backing without rebuilding its key table.
/// This is the `PyDict_Copy` starting point required by `dict | other`: a
/// dense copy preserves CPython deletion dummies and their probe order.
pub(super) fn dict_clone_from_value(v: &Value) -> Option<PyDict> {
    if let Some(dict) = v.dict_with(Clone::clone) {
        return Some(dict);
    }
    if let Some(dict_rc) = pyrust_builtins::mapping_proxy::as_dict_rc(v) {
        return Some(dict_rc.borrow().clone());
    }
    if let Some(backing) = builtin_data_backing(v) {
        return dict_clone_from_value(&backing);
    }
    None
}

/// Extract entries for `**mapping` / dict-display expansion. Native mapping
/// representations are decoded here; user objects use the Python mapping
/// protocol. `None` means the value is not a mapping.
pub(super) fn mapping_entries_for_expansion(
    interp: &mut Interpreter,
    value: &Value,
) -> Result<Option<Vec<(PyKey, Value)>>> {
    if let Some(entries) = dict_entries_from_value(value) {
        return Ok(Some(entries));
    }
    if matches!(value.kind(), ValueKind::PyInstance(_)) {
        return mapping_pairs_via_protocol(interp, value);
    }
    Ok(None)
}

impl Interpreter {
    /// Materialise a `**mapping` / dict-display source with canonical mapping
    /// validation and error wording.
    pub(crate) fn mapping_splat_pairs(&mut self, source: &Value) -> Result<Vec<(PyKey, Value)>> {
        mapping_entries_for_expansion(self, source)?.ok_or_else(|| {
            pyrust_core::type_err!("'{}' object is not a mapping", value_type_name_str(source))
        })
    }

    /// Extend a list accumulator from an arbitrary Python iterable.
    pub(crate) fn extend_list_accumulator(
        &mut self,
        receiver: &Value,
        source: &Value,
    ) -> Result<()> {
        let values = self.collect_iterable(source)?;
        receiver.list_extend(values)
    }

    /// Update a live dict accumulator from a mapping expansion source.
    pub(crate) fn update_dict_accumulator(
        &mut self,
        receiver: &Value,
        source: &Value,
    ) -> Result<()> {
        let entries = self.mapping_splat_pairs(source)?;
        self.dict_extend_value_dedup(receiver, entries)
    }
}
