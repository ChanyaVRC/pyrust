impl Interpreter {
    /// Coerce a set-algebra operand to its `(PySet, frozen)` items.
    ///
    /// Recognises `set` / `frozenset` / `PyInstance` subclasses thereof (via
    /// the free `set_items_from_value`) **and** the set-like dict views
    /// `dict_keys` / `dict_items` (issue #1891).  `dict_values` is *not*
    /// set-like, so it returns `None` (caller falls through to TypeError).
    ///
    /// - `None`         — operand is not a set-like type.
    /// - `Some(Ok(..))` — coerced items; `frozen` is `false` for views (CPython
    ///   set ops on views return a `set`).
    /// - `Some(Err(..))`— operand is set-like but coercion failed, e.g. a
    ///   `dict_items` view whose value is unhashable (`unhashable type: 'list'`).
    pub(crate) fn coerce_set_operand(&mut self, v: &Value) -> Option<Result<(PySet, bool)>> {
        if let Some(items) = set_items_from_value(v) {
            return Some(Ok(items));
        }
        match pyrust_builtins::dict_views::view_kind(v) {
            // dict_keys: keys are already `PyKey`s in the backing IndexMap.
            Some(pyrust_builtins::dict_views::DictViewKind::Keys) => {
                let rc = pyrust_builtins::dict_views::as_dict_rc(v)?;
                let keys: PySet = rc.borrow().keys().cloned().collect();
                Some(Ok((keys, false)))
            }
            // dict_items: each pair becomes a `(key, value)` tuple `PyKey`; the
            // value must be hashable (matches CPython, which builds a set).
            Some(pyrust_builtins::dict_views::DictViewKind::Items) => {
                let rc = pyrust_builtins::dict_views::as_dict_rc(v)?;
                let pairs: Vec<(PyKey, Value)> = rc
                    .borrow()
                    .iter()
                    .map(|(k, val)| (k.clone(), val.clone()))
                    .collect();
                let mut out: PySet = PySet::default();
                for (k, val) in pairs {
                    let val_key = match self.value_to_pykey(&val) {
                        Ok(vk) => vk,
                        Err(e) => return Some(Err(e)),
                    };
                    out.insert(PyKey::Tuple(vec![k, val_key]));
                }
                Some(Ok((out, false)))
            }
            // dict_values and non-views: not set-like.
            _ => None,
        }
    }

    /// Coerce an operand of a dict-view set operator (`&`/`|`/`-`/`^`) to its
    /// `(PySet, frozen)` items (issue #1891).
    ///
    /// Unlike [`Self::coerce_set_operand`], when `allow_iterable` is set this
    /// accepts *any* iterable — list, tuple, str, generator, dict, … — exactly
    /// as CPython's `dictviews_and`/`_or`/`_sub`/`_xor` do (they build a set
    /// from the iterable).  Returns `None` only for non-iterable operands, so
    /// the caller falls through and the normal `__and__`/etc. path raises the
    /// `unsupported operand type(s)` TypeError.
    pub(crate) fn coerce_setop_operand(
        &mut self,
        v: &Value,
        allow_iterable: bool,
    ) -> Option<Result<(PySet, bool)>> {
        if let Some(res) = self.coerce_set_operand(v) {
            return Some(res);
        }
        if !allow_iterable {
            return None;
        }
        Some(self.coerce_setop_iterable_operand(v))
    }

    /// Build a plain set from an operand through its iterator protocol.
    ///
    /// This deliberately skips [`Self::coerce_set_operand`].  CPython's
    /// `_PyDictView_Intersect` manually iterates every non-exact-set operand,
    /// including set and frozenset subclasses, so their overridden `__iter__`
    /// method (and any exception it raises) must remain observable (#3006).
    pub(crate) fn coerce_setop_iterable_operand(&mut self, v: &Value) -> Result<(PySet, bool)> {
        // Treat the operand as an arbitrary iterable (CPython builds a set
        // from it).  A non-iterable operand surfaces the iterator protocol's
        // own `'<type>' object is not iterable` TypeError — matching CPython,
        // whose dict-view set operations iterate the operand directly.
        let items = self.collect_iterable(v)?;
        let mut out: PySet = PySet::default();
        for item in items {
            out.insert(self.value_to_pykey(&item)?);
        }
        Ok((out, false))
    }
}
