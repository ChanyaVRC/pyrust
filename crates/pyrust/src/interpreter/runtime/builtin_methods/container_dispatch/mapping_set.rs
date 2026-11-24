// Built-in dict and set container method dispatch + live dict view builders.

impl Interpreter {
    fn dispatch_dict_container_method(
        &mut self,
        receiver: Value,
        method: &str,
        pos: Vec<Value>,
        kw: &PyDict,
        ordered_mapping: bool,
    ) -> Result<Value> {
        let Some(spec) = pyrust_builtins::dict::method_spec(method) else {
            return pyrust_builtins::dict::call(method, &receiver, pos, kw);
        };
        spec.validate_keywords(!kw.is_empty())?;
        spec.validate_positional_arity(pos.len())?;
        let resolved = spec.method();
        if spec.view_method().is_some() {
            // Live-view construction belongs to the interpreter adapter:
            // it owns both the backing Rc and OrderedDict provenance.
            return Self::dict_view_for_backing(&receiver, resolved.name(), ordered_mapping);
        }
        if resolved == pyrust_builtins::dict::Method::Clear
            && ordered_mapping
            && let Some(id) = receiver.value_id()
        {
            let prelen = receiver.dict_with(|d| d.len()).unwrap_or(0);
            pyrust_builtins::ordered_mapping::note_clear(id, prelen);
        }
        self.call_dict_method_resolved(resolved, receiver, pos, kw)
    }

    fn dispatch_set_container_method(
        &mut self,
        receiver: Value,
        method: &str,
        pos: Vec<Value>,
        kw: &PyDict,
    ) -> Result<Value> {
        let Some(spec) = pyrust_builtins::set::method_spec(method) else {
            return pyrust_builtins::set::call(method, &receiver, pos);
        };
        spec.validate_keywords(!kw.is_empty())?;
        spec.validate_positional_arity(pos.len())?;
        self.call_set_method_resolved(spec.method(), receiver, pos)
    }

    /// Build a live, `Rc`-shared `dict_views` view (`keys`/`values`/`items`)
    /// from a dict-subclass instance's backing dict (issue #2436).  Sharing the
    /// backing `IndexMap` `Rc` — instead of the snapshot `list` that
    /// `dict::call` materialises when it cannot see the `Rc` — keeps the view
    /// live and routes its iteration through the size-mutation guard.  `ordered`
    /// tags OrderedDict-backed views so the guard reports the OrderedDict
    /// wording.  ONE decision shared by the slow `BkKind::Dict` dispatch and the
    /// `dispatch_backing_primitive_method` inline-cache fast path (the two were
    /// the #2324 duplicate-decision drift that left cached subclass views
    /// unguarded).
    pub(crate) fn dict_view_for_backing(
        backing: &Value,
        method: &str,
        ordered: bool,
    ) -> Result<Value> {
        let rc = backing
            .get_dict_rc()
            .ok_or_else(|| PyError::Runtime("internal: expected dict backing".to_string()))?
            .clone();
        Ok(match method {
            "keys" => pyrust_builtins::dict_views::dict_keys_tagged(rc, ordered),
            "values" => pyrust_builtins::dict_views::dict_values_tagged(rc, ordered),
            _ => pyrust_builtins::dict_views::dict_items_tagged(rc, ordered),
        })
    }

    /// Like [`dict_view_for_backing`], preserving the dict-subclass owner so
    /// `.mapping` wraps that owner rather than stripping it to a plain dict (#3019).
    pub(crate) fn dict_view_for_owner(
        backing: &Value,
        method: &str,
        ordered: bool,
        owner: Value,
    ) -> Result<Value> {
        let rc = backing
            .get_dict_rc()
            .ok_or_else(|| PyError::Runtime("internal: expected dict backing".to_string()))?
            .clone();
        Ok(match method {
            "keys" => pyrust_builtins::dict_views::dict_keys_for_owner(rc, ordered, owner),
            "values" => pyrust_builtins::dict_views::dict_values_for_owner(rc, ordered, owner),
            _ => pyrust_builtins::dict_views::dict_items_for_owner(rc, ordered, owner),
        })
    }
}
