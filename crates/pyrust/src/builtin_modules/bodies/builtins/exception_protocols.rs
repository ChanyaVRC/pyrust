use pyrust_derive::pyrust_module;

pyrust_module! {
    /// Issue #1112: `BaseException.__init__(self, *args)` — updates `self.args`
    /// so that `super().__init__(msg)` in an exception subclass sets the correct
    /// `.args` tuple on the already-constructed instance.  Also mirrors the
    /// `StopIteration.value` special-case from `instantiate_exception`.
    ///
    /// CPython signature: `BaseException.__init__(self, *args)`
    #[py_name = "BaseException.__init__"]
    fn base_exception_init(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Ok(Value::none());
        };
        let inst_rc = match first.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => return Ok(Value::none()),
        };
        let exc_args: Vec<Value> = args[1..]
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| a.value.clone())
            .collect();
        // Update .args on the existing instance.
        inst_rc
            .borrow_mut()
            .attrs
            .insert_slot("args", Value::tuple(exc_args.clone()));
        // Mirror the StopIteration.value special-case.
        let (is_stop_iteration, is_unicode_decode, is_unicode_encode, is_unicode_translate) = {
            let class = Rc::clone(&inst_rc.borrow().class);
            let kinds = classify_exception_class(&class);
            (
                kinds.stop_iteration,
                kinds.unicode_decode_error,
                kinds.unicode_encode_error,
                kinds.unicode_translate_error,
            )
        };
        if is_stop_iteration {
            let val = exc_args.into_iter().next().unwrap_or_else(Value::none);
            inst_rc
                .borrow_mut()
                .attrs
                .insert_slot("value", val);
        } else if is_unicode_decode || is_unicode_encode || is_unicode_translate {
            // Mirror the Unicode-error attribute-setting from instantiate_exception
            // so that `super().__init__(enc, obj, start, end, reason)` in a
            // subclass's __init__ sets all five (or four) structured attributes.
            unicode_exc_set_attrs(
                &mut inst_rc.borrow_mut().attrs,
                &exc_args,
                is_unicode_decode || is_unicode_encode,
            );
        }
        Ok(Value::none())
    }

    /// Issue #2361: `BaseException.__reduce__(self)` — the pickle reduction.
    ///
    /// CPython's `BaseException.__reduce__` returns `(type(self), self.args)`,
    /// with a third element (the instance `__dict__`) appended whenever the
    /// instance carries any non-slot attributes.  The C-level exception slots
    /// (`args`, `__traceback__`, `__cause__`, `__context__`,
    /// `__suppress_context__`, and class-specific structured slots) are
    /// excluded from that state dict — which is why `copy`/`deepcopy` of a
    /// caught exception drop the traceback (#2360).
    ///
    /// CPython signature: `BaseException.__reduce__(self, /)`
    #[py_name = "BaseException.__reduce__"]
    fn base_exception_reduce(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__reduce__", "BaseException", method)
        })?;
        Ok(base_exception_reduce_value(&self_val))
    }

    /// Issue #2361: `BaseException.__reduce_ex__(self, protocol)` — CPython
    /// inherits `object.__reduce_ex__`, which for an exception ends up calling
    /// `self.__reduce__()`.  We return the same `(type, args[, state])` tuple so
    /// that the protocol the `copy` module relies on is exception-correct.
    ///
    /// CPython signature: `BaseException.__reduce_ex__(self, protocol, /)`
    #[py_name = "BaseException.__reduce_ex__"]
    fn base_exception_reduce_ex(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__reduce_ex__", "BaseException", method)
        })?;
        Ok(base_exception_reduce_value(&self_val))
    }

    /// Issue #1067: `BaseException.add_note(note)` — Python 3.11+ method.
    ///
    /// Appends `note` (a str) to `self.__notes__`, creating `__notes__` as
    /// a fresh list if it does not yet exist.  Matches CPython 3.12 semantics:
    /// - `note` must be a `str`; otherwise raises `TypeError`.
    /// - Returns `None`.
    /// - `hasattr(exc, "__notes__")` is `False` until `add_note` is called.
    ///
    /// CPython signature: `BaseException.add_note(self, note, /)`
    #[py_name = "BaseException.add_note"]
    fn base_exception_add_note(args) -> Result<Value> {
        // args[0] = self, args[1] = note; exactly one user argument expected.
        let user_argc = args.len().saturating_sub(1);
        if user_argc != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "BaseException.add_note() takes exactly one argument ({user_argc} given)"
                ),
            ));
        }
        // Reject keyword arguments.
        if args.iter().any(|a| a.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                "BaseException.add_note() takes no keyword arguments".to_string(),
            ));
        }
        let self_val = &args[0].value;
        let note_val = &args[1].value;

        // `note` must be a str.
        let note_str = match note_val.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "note must be a str, not '{}'",
                        value_type_name_str(note_val)
                    ),
                ));
            }
        };

        // Mutate self.__notes__ in place.
        let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor 'add_note' for 'BaseException' objects doesn't apply to a '{}' object",
                    value_type_name_str(self_val),
                ),
            ));
        };
        // If __notes__ is absent, insert a fresh empty list so we can
        // always call list_push on the value without re-borrowing inst.
        {
            let mut inst = inst_rc.borrow_mut();
            if !inst.attrs.contains_key("__notes__") {
                inst.attrs.insert("__notes__", Value::list(vec![]));
            }
        }
        // Re-borrow immutably to read the list value and push to it.
        // Value::list_push takes &self and uses RefCell internally — no
        // need to hold a mutable borrow on the instance for this step.
        // Read back via `get_cloned` so a dict-backed instance (#1981/#2637)
        // resolves the list we just inserted into its live `__dict__`; a raw
        // `get` (entries only) would miss it and hand back a fresh orphan list,
        // silently dropping every appended note after a `__dict__` swap.
        let notes_val = inst_rc
            .borrow()
            .attrs
            .get_cloned("__notes__")
            .unwrap_or_else(|| Value::list(vec![]));
        notes_val
            .list_push(Value::string(note_str))
            .map_err(|_| {
                PyError::named(
                    "TypeError",
                    "Cannot add note: __notes__ is not a list".to_string(),
                )
            })?;
        Ok(Value::none())
    }

    /// Issue #1441: `BaseException.with_traceback(tb)` — sets `self.__traceback__`
    /// to `tb` and returns `self`.
    ///
    /// CPython 3.12: `tb` must be a traceback object or `None`; anything else
    /// raises `TypeError: __traceback__ must be a traceback or None`.
    ///
    /// CPython signature: `BaseException.with_traceback(self, tb, /)`
    #[py_name = "BaseException.with_traceback"]
    fn base_exception_with_traceback(args) -> Result<Value> {
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "BaseException.with_traceback() takes exactly one argument ({} given)",
                    args.len().saturating_sub(1)
                ),
            ));
        }
        let self_val = &args[0].value;
        let tb_val = &args[1].value;
        // tb must be None or a traceback object.
        let ok = tb_val.is_none() || pyrust_builtins::traceback::is_traceback(tb_val);
        if !ok {
            return Err(PyError::named(
                "TypeError",
                "__traceback__ must be a traceback or None".to_string(),
            ));
        }
        let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor 'with_traceback' for 'BaseException' objects doesn't apply to a '{}' object",
                    value_type_name_str(self_val),
                ),
            ));
        };
        inst_rc
            .borrow_mut()
            .attrs
            .insert_slot("__traceback__", tb_val.clone());
        Ok(self_val.clone())
    }
}
