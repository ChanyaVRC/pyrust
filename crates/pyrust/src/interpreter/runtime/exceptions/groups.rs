// PEP 654 ExceptionGroup matching, splitting, and derivation.

impl Interpreter {
    /// PEP 654 `except*` helper: split the `.exceptions` of a `BaseExceptionGroup`
    /// into matched (instances of `kind`) and remaining (non-matching).
    ///
    /// Returns `None` if:
    ///   - `group` is not a `BaseExceptionGroup` instance, OR
    ///   - no contained exception is an instance of `kind`.
    ///
    /// Returns `Some((matched_group, remaining_group))` where:
    ///   - `matched_group`    = new group containing only matching exceptions
    ///   - `remaining_group`  = `Some(group)` with non-matching exceptions,
    ///     or `None` if all exceptions were matched
    pub(super) fn split_exception_group(
        &mut self,
        group_in: &Value,
        kind: &Value,
    ) -> Result<Option<(Value, Option<Value>)>> {
        // PEP 654: `except*` forbids catching a `BaseExceptionGroup` subclass and
        // any non-exception catch type.  CPython validates the whole catch type
        // (a single class or a tuple) up-front: the "does not inherit from
        // BaseException" check wins over the ExceptionGroup check across the
        // tuple, and both fire before any filtering happens.
        validate_except_star_type(kind)?;

        // PEP 654: if the active exception is a plain (non-group) exception,
        // wrap it in an ExceptionGroup before filtering — matching CPython's
        // implicit wrapping behaviour for `except*`.
        let group_owned;
        let group = if let ValueKind::PyInstance(inst_rc) = group_in.kind() {
            let cls = Rc::clone(&inst_rc.borrow().class);
            if !pyrust_core::class_chain_contains_builtin_exception(&cls, "BaseExceptionGroup") {
                let is_exception =
                    pyrust_core::class_chain_contains_builtin_exception(&cls, "Exception");
                let wrap_cls_name = if is_exception {
                    "ExceptionGroup"
                } else {
                    "BaseExceptionGroup"
                };
                let wrap_cls = match lookup_exc_class(wrap_cls_name) {
                    Some(c) => c,
                    None => return Ok(None),
                };
                group_owned = instantiate_exception(
                    wrap_cls,
                    vec![
                        Value::string(String::new()),
                        Value::tuple(vec![group_in.clone()]),
                    ],
                );
                &group_owned
            } else {
                group_in
            }
        } else {
            return Ok(None);
        };

        // Must be an instance of BaseExceptionGroup.
        match group.kind() {
            ValueKind::PyInstance(i)
                if pyrust_core::class_chain_contains_builtin_exception(
                    &i.borrow().class,
                    "BaseExceptionGroup",
                ) => {}
            _ => return Ok(None),
        }

        // PEP 654: recurse the whole tree so a matching leaf at any nesting
        // depth is collected, preserving the nested group structure.  Reuse the
        // recursive `eg_split` from PR #2203 (which backs `ExceptionGroup.split`)
        // instead of a flat direct-children-only scan.
        let matcher = EgMatcher::Type(kind.clone());
        let (matched, remaining) = self.eg_split(group, &matcher)?;
        match matched {
            Some(matched_group) => Ok(Some((matched_group, remaining))),
            None => Ok(None),
        }
    }

    /// PEP 654 `BaseExceptionGroup.derive(excs)` — build a new exception group
    /// with the same `.message` but the supplied `excs` as `.exceptions`.
    /// CPython's default derivation calls `BaseExceptionGroup(self.message, excs)`,
    /// which promotes to `ExceptionGroup` when every exception is an `Exception`
    /// subclass.  `derive` does NOT copy `__traceback__`/`__cause__`/`__context__`/
    /// `__notes__` — the caller (`subgroup`/`split`) copies those onto the result.
    pub(crate) fn exception_group_derive(&mut self, args: &[ExpandedCallArg]) -> Result<Value> {
        // args[0] = self, args[1] = excs (sequence).
        let user_argc = args.len().saturating_sub(1);
        if user_argc != 1 {
            return Err(pyrust_core::type_err!(&format!(
                "function takes exactly 1 argument ({user_argc} given)"
            )));
        }
        let self_val = &args[0].value;
        let inst_rc = match self_val.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => {
                return Err(pyrust_core::type_err!(
                    "descriptor 'derive' for 'BaseExceptionGroup' objects doesn't apply to this object"
                ));
            }
        };
        let message = inst_rc
            .borrow()
            .attrs
            .get_slot("message")
            .cloned()
            .unwrap_or_else(|| Value::string(String::new()));
        let excs: Vec<Value> = if let Some(t) = args[1].value.as_tuple() {
            t.to_vec()
        } else if let Some(l) = args[1].value.as_list() {
            l.to_vec()
        } else {
            return Err(pyrust_core::type_err!(
                "second argument (exceptions) must be a sequence"
            ));
        };
        Ok(self.make_derived_group(message, excs))
    }

    /// Construct a `BaseExceptionGroup`/`ExceptionGroup` value directly,
    /// applying the same Exception-subclass promotion that
    /// `BaseExceptionGroup.__new__` performs.  Used by the default `derive`.
    fn make_derived_group(&self, message: Value, excs: Vec<Value>) -> Value {
        let all_exc = !excs.is_empty()
            && excs.iter().all(|v| {
                if let ValueKind::PyInstance(i) = v.kind() {
                    pyrust_core::class_chain_contains_builtin_exception(
                        &i.borrow().class,
                        "Exception",
                    )
                } else {
                    false
                }
            });
        let cls_name = if all_exc {
            "ExceptionGroup"
        } else {
            "BaseExceptionGroup"
        };
        let cls = lookup_exc_class(cls_name)
            .or_else(|| lookup_exc_class("BaseExceptionGroup"))
            .expect("BaseExceptionGroup class must exist");
        // CPython's default `derive` stores the exceptions as a *list* in
        // `.args` (so `repr` renders `[...]`), while `.exceptions` is
        // normalised to a tuple by `instantiate_exception`.
        instantiate_exception(cls, vec![message, Value::list(excs)])
    }

    /// PEP 654 `subgroup(condition)` / `split(condition)`.  When `want_split`
    /// is false, returns the match subgroup (or `None`); when true, returns a
    /// `(match, rest)` 2-tuple (each element a group or `None`).
    pub(crate) fn exception_group_subgroup_or_split(
        &mut self,
        args: &[ExpandedCallArg],
        want_split: bool,
    ) -> Result<Value> {
        let method = if want_split { "split" } else { "subgroup" };
        let user_argc = args.len().saturating_sub(1);
        if user_argc != 1 {
            return Err(pyrust_core::type_err!(&format!(
                "{method} expected 1 argument, got {user_argc}"
            )));
        }
        let self_val = args[0].value.clone();
        let condition = args[1].value.clone();
        // Validate the condition up-front (CPython raises TypeError before any
        // matching for a non-exception-type / non-callable condition).
        let matcher = self.classify_eg_condition(&condition)?;
        let (m, r) = self.eg_split(&self_val, &matcher)?;
        if want_split {
            Ok(Value::tuple(vec![
                m.unwrap_or_else(Value::none),
                r.unwrap_or_else(Value::none),
            ]))
        } else {
            Ok(m.unwrap_or_else(Value::none))
        }
    }

    /// Classify a `subgroup`/`split` condition into a matcher.
    fn classify_eg_condition(&self, condition: &Value) -> Result<EgMatcher> {
        match condition.kind() {
            // An exception *type*.
            ValueKind::PyClass(cls) => {
                if is_exception_class(cls) {
                    Ok(EgMatcher::Type(condition.clone()))
                } else {
                    Err(pyrust_core::type_err!(
                        "expected a function, exception type or tuple of exception types"
                    ))
                }
            }
            // A tuple of exception types.
            ValueKind::Tuple(items) => {
                for item in items.iter() {
                    match item.kind() {
                        ValueKind::PyClass(cls) if is_exception_class(cls) => {}
                        _ => {
                            return Err(pyrust_core::type_err!(
                                "expected a function, exception type or tuple of exception types"
                            ));
                        }
                    }
                }
                Ok(EgMatcher::Type(condition.clone()))
            }
            // Otherwise: must be callable (a predicate).
            ValueKind::UserFunction(_)
            | ValueKind::BuiltinFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. } => Ok(EgMatcher::Predicate(condition.clone())),
            ValueKind::PyInstance(inst) => {
                // A callable instance (defines __call__).
                if lookup_class_attr(&inst.borrow().class, "__call__").is_some() {
                    Ok(EgMatcher::Predicate(condition.clone()))
                } else {
                    Err(pyrust_core::type_err!(
                        "expected a function, exception type or tuple of exception types"
                    ))
                }
            }
            _ => Err(pyrust_core::type_err!(
                "expected a function, exception type or tuple of exception types"
            )),
        }
    }

    /// Apply a matcher to a single exception value.
    fn eg_matches(&mut self, exc: &Value, matcher: &EgMatcher) -> Result<bool> {
        match matcher {
            EgMatcher::Type(kind) => self.exception_matches(exc, kind),
            EgMatcher::Predicate(func) => {
                let res = self.call_function_expanded(
                    func.clone(),
                    &[ExpandedCallArg {
                        name: None,
                        value: exc.clone(),
                    }],
                )?;
                self.truthy_value(&res)
            }
        }
    }

    /// Recursive PEP 654 split.  Returns `(match, rest)` where each is `Some`
    /// group or `None`.  If the matcher matches the group itself, the whole
    /// group is returned (by identity) as the match.
    fn eg_split(
        &mut self,
        group: &Value,
        matcher: &EgMatcher,
    ) -> Result<(Option<Value>, Option<Value>)> {
        // If the condition matches the group as a whole, the entire group is
        // the match and there is no rest (CPython returns the same object).
        if self.eg_matches(group, matcher)? {
            return Ok((Some(group.clone()), None));
        }
        let inst_rc = match group.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok((None, None)),
        };
        let items: Vec<Value> = {
            let borrowed = inst_rc.borrow();
            match borrowed.attrs.get_slot("exceptions") {
                Some(v) => v
                    .as_tuple()
                    .map(|t| t.to_vec())
                    .or_else(|| v.as_list().map(|l| l.to_vec()))
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        };
        let mut match_excs: Vec<Value> = Vec::new();
        let mut rest_excs: Vec<Value> = Vec::new();
        for exc in &items {
            let is_group = matches!(exc.kind(), ValueKind::PyInstance(i)
            if pyrust_core::class_chain_contains_builtin_exception(
                &i.borrow().class,
                "BaseExceptionGroup",
            ));
            if is_group {
                let (m, r) = self.eg_split(exc, matcher)?;
                if let Some(m) = m {
                    match_excs.push(m);
                }
                if let Some(r) = r {
                    rest_excs.push(r);
                }
            } else if self.eg_matches(exc, matcher)? {
                match_excs.push(exc.clone());
            } else {
                rest_excs.push(exc.clone());
            }
        }
        let m = if match_excs.is_empty() {
            None
        } else {
            Some(self.eg_derive_with_metadata(group, match_excs)?)
        };
        let r = if rest_excs.is_empty() {
            None
        } else {
            Some(self.eg_derive_with_metadata(group, rest_excs)?)
        };
        Ok((m, r))
    }

    /// Build a sub-group from `group` via its (possibly user-overridden)
    /// `derive` method, then copy `__traceback__`/`__cause__`/`__context__`/
    /// `__notes__` from the source group onto the result (matching CPython's
    /// `exceptiongroup_subset`).
    fn eg_derive_with_metadata(&mut self, group: &Value, excs: Vec<Value>) -> Result<Value> {
        // Look up `derive` via the MRO so a subclass override is honoured.
        let derive_fn = match group.kind() {
            ValueKind::PyInstance(i) => lookup_class_attr(&i.borrow().class, "derive"),
            _ => None,
        };
        let derived = match derive_fn {
            Some(f) => self.call_function_expanded(
                f,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: group.clone(),
                    },
                    ExpandedCallArg {
                        name: None,
                        value: Value::list(excs),
                    },
                ],
            )?,
            None => {
                // No derive in MRO (shouldn't happen): fall back to default.
                let message = match group.kind() {
                    ValueKind::PyInstance(i) => i
                        .borrow()
                        .attrs
                        .get_slot("message")
                        .cloned()
                        .unwrap_or_else(|| Value::string(String::new())),
                    _ => Value::string(String::new()),
                };
                self.make_derived_group(message, excs)
            }
        };
        // Copy metadata from the source group onto the derived group.  Skip the
        // copy when a misbehaving user `derive` returns the source object itself
        // (same RefCell) — the values are already present and re-borrowing
        // mutably while holding the shared borrow would panic.
        if let (ValueKind::PyInstance(src), ValueKind::PyInstance(dst)) =
            (group.kind(), derived.kind())
            && !Rc::ptr_eq(src, dst)
        {
            let src_b = src.borrow();
            let mut dst_b = dst.borrow_mut();
            // `__traceback__` / `__cause__` / `__context__` are real C-level
            // slots in CPython and in pyrust's separate slot backing, so they
            // survive a `__dict__` swap without consulting that mapping.
            for key in ["__traceback__", "__cause__", "__context__"] {
                if let Some(v) = src_b.attrs.get_cloned_or_slot(key) {
                    dst_b.attrs.insert_slot(key, v);
                }
            }
            // CPython's `exceptiongroup_subset` builds every *derived*
            // subgroup with `suppress_context = true` unconditionally — it
            // does NOT copy the source group's flag.  A `.split()` / `.subgroup()`
            // result (and the `except*` residual once an outer handler
            // re-splits it, #2755) therefore surfaces with
            // `__suppress_context__ is True` even when the source group's was
            // the `False` default.  Only the whole-group match returns the
            // source object unchanged (handled in `eg_split` before reaching
            // here), so forcing the flag on the derived path is safe.
            dst_b
                .attrs
                .insert_slot("__suppress_context__", Value::bool_(true));
            // `__notes__`, by contrast, is an ordinary `__dict__` attribute in
            // CPython (`add_note` stores it there), so a `__dict__` swap drops
            // it and the derived group must not inherit the stale pre-swap notes.
            // Read dict-only so behaviour matches CPython for dict-backed groups.
            if let Some(v) = src_b.attrs.get_cloned("__notes__") {
                dst_b.attrs.insert("__notes__", v);
            }
        }
        Ok(derived)
    }

    pub(super) fn exception_matches(&self, exception: &Value, kind: &Value) -> Result<bool> {
        let instance = match exception.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok(false),
        };

        let raised_class = Rc::clone(&instance.borrow().class);
        match kind.kind() {
            ValueKind::PyClass(expected) => {
                let expected = Rc::clone(expected);
                if !is_exception_class(&expected) {
                    return Err(pyrust_core::type_err!(
                        "catching classes that do not inherit from BaseException is not allowed"
                    ));
                }
                Ok(class_is_subclass_of(&raised_class, &expected))
            }
            ValueKind::Tuple(items) => {
                // CPython validates all tuple elements before matching — raise TypeError
                // for any non-exception-class element, even if an earlier element matches.
                let mut matched = false;
                for item in items {
                    match item.kind() {
                        ValueKind::PyClass(expected) => {
                            let expected = Rc::clone(expected);
                            if !is_exception_class(&expected) {
                                return Err(pyrust_core::type_err!(
                                    "catching classes that do not inherit from BaseException is not allowed"
                                ));
                            }
                            if class_is_subclass_of(&raised_class, &expected) {
                                matched = true;
                            }
                        }
                        _ => {
                            return Err(pyrust_core::type_err!(
                                "catching classes that do not inherit from BaseException is not allowed"
                            ));
                        }
                    }
                }
                Ok(matched)
            }
            _ => Err(pyrust_core::type_err!(
                "catching classes that do not inherit from BaseException is not allowed"
            )),
        }
    }
}

/// A PEP 654 `subgroup`/`split` condition, classified into either an
/// exception type / tuple-of-types (matched with `isinstance` semantics) or a
/// callable predicate `(exc) -> bool`.
enum EgMatcher {
    Type(Value),
    Predicate(Value),
}

/// Validate an `except*` catch type (a single exception class or a tuple of
/// them), matching CPython's `check_except_star_type_valid`.
///
/// Two passes over the whole catch type, in CPython's precedence order:
///   1. every element must inherit from `BaseException`, else
///      "catching classes that do not inherit from BaseException is not allowed"
///      (this wins over the ExceptionGroup check, even when a later element is
///      itself an exception-group subclass);
///   2. no element may be a `BaseExceptionGroup` subclass, else
///      "catching ExceptionGroup with except* is not allowed. Use except instead."
fn validate_except_star_type(kind: &Value) -> Result<()> {
    let classes: Vec<&Rc<RefCell<PyClass>>> = match kind.kind() {
        ValueKind::PyClass(cls) => vec![cls],
        ValueKind::Tuple(items) => {
            let mut v = Vec::with_capacity(items.len());
            for item in items {
                match item.kind() {
                    ValueKind::PyClass(cls) => v.push(cls),
                    _ => {
                        return Err(pyrust_core::type_err!(
                            "catching classes that do not inherit from BaseException is not allowed"
                        ));
                    }
                }
            }
            v
        }
        _ => {
            return Err(pyrust_core::type_err!(
                "catching classes that do not inherit from BaseException is not allowed"
            ));
        }
    };
    for cls in &classes {
        if !is_exception_class(cls) {
            return Err(pyrust_core::type_err!(
                "catching classes that do not inherit from BaseException is not allowed"
            ));
        }
    }
    for cls in &classes {
        if pyrust_core::class_chain_contains_builtin_exception(cls, "BaseExceptionGroup") {
            return Err(pyrust_core::type_err!(
                "catching ExceptionGroup with except* is not allowed. Use except instead."
            ));
        }
    }
    Ok(())
}
