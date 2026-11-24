use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: sorted(iterable, /, *, key=None, reverse=False) — new sorted list.
    /// <https://docs.python.org/3/library/functions.html#sorted>
    ///
    /// Migrated to the typed-signature dialect (#400): `iterable` is
    /// positional-only, `key` and `reverse` are keyword-only, and
    /// `#[arity_style(expected_got)]` preserves `sorted expected 1 argument, got N`.
    /// This can run user code by dispatching `__lt__` (and related
    /// comparison dunders) when sorting, and may invoke the user-supplied key
    /// function which can execute arbitrary user code.
    #[arity_style(expected_got)]
    fn sorted(
        #[positional_only] iterable: PyValueRef,
        #[keyword_only] #[default(OptionalArg::Omitted)] key: OptValueRef,
        #[keyword_only] #[default(OptionalArg::Omitted)] reverse: OptValueRef,
    ) -> Result<Value> {
        // `reverse=` is dispatched through `__bool__` (with `__len__`
        // fallback and default-truthy for instances without either) —
        // matches CPython 3.12+ which coerces via `bool()`.  An earlier
        // attempt routed through `__index__` based on Python 3.11
        // behaviour; 3.12 changed it (CPython commit history confirms),
        // so the truthy-dispatch path is the cross-version-safe choice
        // for the pyrust matrix.  See #432 review + CI parity failure
        // on `sorted-rev-justbool` / `-nothing` cases under 3.12.
        let reverse = match reverse.as_value() {
            Some(v) => _interp.truthy_value(v)?,
            None => false,
        };
        let key_fn = key.as_value().filter(|v| !v.is_none()).cloned();
        let source = iterable.0;
        let mut items = match _interp.collect_mapping_proxy_with_length_hint(source)? {
            Some(items) => items,
            None => _interp.collect_iterable(source)?,
        };
        if let Some(kfn) = key_fn {
            let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(items.len());
            for v in std::mem::take(&mut items) {
                let k = _interp.call_function_expanded(
                    kfn.clone(),
                    &[ExpandedCallArg { name: None, value: v.clone() }],
                )?;
                keyed.push((k, v));
            }
            // Classify by key (one pass, no alloc): homogeneous all-int / all-str
            // keys sort with a native comparator; a `PyInstance` key routes
            // through the interpreter; every other primitive mix uses
            // `compare_values`.
            match classify_sort(keyed.iter().map(|(k, _)| k)) {
                SortKind::AllInt => keyed.sort_by(|(a, _), (b, _)| {
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    lhs.as_int().unwrap_or(0).cmp(&rhs.as_int().unwrap_or(0))
                }),
                SortKind::AllStr => keyed.sort_by(|(a, _), (b, _)| {
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    lhs.as_str().unwrap_or("").cmp(rhs.as_str().unwrap_or(""))
                }),
                SortKind::HasInstance | SortKind::General => {
                    let mut sort_err: Option<PyError> = None;
                    keyed.sort_by(|(a, _), (b, _)| {
                        if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                        let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                        match _interp.richcmp_order(lhs, rhs) {
                            Ok(ord) => ord,
                            Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                        }
                    });
                    if let Some(e) = sort_err { return Err(e); }
                }
            }
            // Reuse the `items` buffer (now empty after `take`) to avoid
            // a fresh allocation when extracting values from the keyed pairs.
            items.extend(keyed.into_iter().map(|(_, v)| v));
        } else {
            // Classify once: a homogeneous all-int / all-str slice sorts with a
            // native comparator (CPython's `unsafe_long_compare` /
            // `unsafe_latin_compare`) — no per-pair type dispatch, no `Result`,
            // cannot raise.  A `PyInstance` routes through the interpreter (user
            // `__lt__`); every other primitive mix uses `compare_values`.
            match classify_sort(items.iter()) {
                SortKind::AllInt => items.sort_by(|a, b| {
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    lhs.as_int().unwrap_or(0).cmp(&rhs.as_int().unwrap_or(0))
                }),
                SortKind::AllStr => items.sort_by(|a, b| {
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    lhs.as_str().unwrap_or("").cmp(rhs.as_str().unwrap_or(""))
                }),
                SortKind::HasInstance | SortKind::General => {
                    let mut sort_err: Option<PyError> = None;
                    items.sort_by(|a, b| {
                        if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                        let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                        match _interp.richcmp_order(lhs, rhs) {
                            Ok(ord) => ord,
                            Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                        }
                    });
                    if let Some(e) = sort_err { return Err(e); }
                }
            }
        }
        // `reverse=True` is applied by inverting the comparison inside the
        // stable `sort_by` (operand swap above), matching `list.sort`'s
        // `sort_by_cmp`.  A trailing `items.reverse()` would flip equal
        // runs too and break stability (see #1904).
        Ok(Value::list(items))
    }

    /// CPython: min(iterable, /, *, key=None) or min(*args, key=None).
    /// <https://docs.python.org/3/library/functions.html#min>
    /// This can run user code by dispatching `__lt__` (and related
    /// comparison dunders) when comparing elements, and may invoke the
    /// user-supplied key function.
    fn min(args) -> Result<Value> {
        min_max_impl(_interp, args, false, FN_NAME)
    }

    /// CPython: max(iterable, /, *, key=None) or max(*args, key=None).
    /// <https://docs.python.org/3/library/functions.html#max>
    /// This can run user code by dispatching `__gt__` (with `__lt__` as
    /// reflected fallback) when comparing elements, and may invoke the
    /// user-supplied key function.
    fn max(args) -> Result<Value> {
        min_max_impl(_interp, args, true, FN_NAME)
    }
}
