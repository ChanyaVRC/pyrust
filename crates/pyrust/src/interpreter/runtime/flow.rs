impl Interpreter {
    fn slice_index_from_value(value: &Value) -> Result<i64> {
        match value.kind() {
            ValueKind::Int(i) => Ok(i),
            ValueKind::Bool(b) => Ok(if b { 1 } else { 0 }),
            // BigInt slice bounds: clamp to i64 range, matching CPython's behaviour
            // of clamping to sys.maxsize / -sys.maxsize-1 (which on 64-bit platforms
            // equals i64::MAX / i64::MIN).
            ValueKind::BigInt(big) => Ok(match big.to_i64() {
                Some(i) => i,
                None => match big.sign() {
                    PyBigIntSign::Minus => i64::MIN,
                    _ => i64::MAX,
                },
            }),
            _ => Err(pyrust_core::type_err!("slice indices must be integers or None or have an __index__ method")),
        }
    }

    fn resolve_slice_bounds(
        len: i64,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<(i64, i64, i64)> {
        let step = match st {
            None => 1,
            Some(v) if v.is_none() => 1,
            Some(v) => {
                let s = Self::slice_index_from_value(v)?;
                if s == 0 {
                    return Err(pyrust_core::value_err!("slice step cannot be zero"));
                }
                s
            }
        };

        let normalize = |idx: i64| -> i64 {
            if idx < 0 {
                (idx + len).clamp(0, len)
            } else {
                idx.clamp(0, len)
            }
        };

        let start_default = if step > 0 { 0 } else { len - 1 };
        let end_default = if step > 0 { len } else { -1 };

        let start = match lo {
            None => start_default,
            Some(v) if v.is_none() => start_default,
            Some(v) => {
                let i = Self::slice_index_from_value(v)?;
                if step > 0 {
                    normalize(i)
                } else if i < 0 {
                    (i + len).clamp(-1, len - 1)
                } else {
                    i.clamp(-1, len - 1)
                }
            }
        };

        let end = match hi {
            None => end_default,
            Some(v) if v.is_none() => end_default,
            Some(v) => {
                let i = Self::slice_index_from_value(v)?;
                if step > 0 {
                    normalize(i)
                } else if i < 0 {
                    (i + len).clamp(-1, len - 1)
                } else {
                    i.clamp(-1, len - 1)
                }
            }
        };

        Ok((start, end, step))
    }

    fn slice_target_indices(len: i64, start: i64, end: i64, step: i64) -> Vec<usize> {
        let mut targets = Vec::new();
        let mut i = start;

        if step > 0 {
            while i < end {
                if i >= 0 && i < len {
                    targets.push(i as usize);
                }
                i += step;
            }
        } else {
            while i > end {
                if i >= 0 && i < len {
                    targets.push(i as usize);
                }
                i += step;
            }
        }
        targets
    }

    /// If `key` is a runtime `slice` object (produced by `BuildSlice`), unpack it.
    /// Returns `Some((lo, hi, step))` where each is `None` for a missing bound.
    ///
    /// Prior to issue #931 this function matched any 3-element tuple, which
    /// ambiguously treated user tuples like `(1, 2, 3)` as slice keys.  The
    /// `BuildSlice` instruction now creates a real slice BuiltinObject, so we
    /// match on that instead.
    pub(crate) fn unpack_slice_key(key: &Value) -> Option<(Option<Value>, Option<Value>, Option<Value>)> {
        if let ValueKind::BuiltinObject { ops, state } = key.kind()
            && ops.type_name() == pyrust_builtins::slice::TYPE_NAME
        {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<pyrust_builtins::slice::SliceState>()
                .expect("unpack_slice_key: SliceState type mismatch");
            let opt = |v: &Value| if v.is_none() { None } else { Some(v.clone()) };
            return Some((opt(&s.start), opt(&s.stop), opt(&s.step)));
        }
        None
    }

    /// Slice-assign: `items[lo:hi:step] = new_items`.
    pub(crate) fn slice_setitem(
        items: &mut Vec<Value>,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
        new_items: Vec<Value>,
    ) -> Result<()> {
        let len = items.len() as i64;
        let (start, end, step) = Self::resolve_slice_bounds(len, lo, hi, st)?;
        if step == 1 {
            let s = start as usize;
            let e = end as usize;
            items.splice(s..e, new_items);
        } else {
            let indices = Self::slice_target_indices(len, start, end, step);
            if indices.len() != new_items.len() {
                return Err(PyError::Runtime(
                    "attempt to assign sequence of wrong size".to_string(),
                ));
            }
            for (ix, val) in indices.into_iter().zip(new_items) {
                items[ix] = val;
            }
        }
        Ok(())
    }

    /// Slice-delete: `del items[lo:hi:step]` (equivalent to `items[lo:hi:step] = []`).
    pub(crate) fn slice_delitem(
        items: &mut Vec<Value>,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<()> {
        let len = items.len() as i64;
        let (start, end, step) = Self::resolve_slice_bounds(len, lo, hi, st)?;
        let indices = Self::slice_target_indices(len, start, end, step);
        // Remove in reverse so indices stay valid.
        let mut sorted = indices;
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        for ix in sorted {
            items.remove(ix);
        }
        Ok(())
    }

    /// Returns true when the two `Value`s wrap the same `PyInstance`
    /// (pointer-equal).  Used to detect when control is inside an
    /// active `except` handler body — i.e. when the interpreter's
    /// `active_exception` is the same instance as the top of
    /// `handled_exc_stack`.
    pub(crate) fn values_are_same_exception(a: &Value, b: &Value) -> bool {
        match (a.kind(), b.kind()) {
            (ValueKind::PyInstance(x), ValueKind::PyInstance(y)) => Rc::ptr_eq(x, y),
            _ => false,
        }
    }

    /// PEP 3134 implicit exception chaining: if a `raise` happens inside an
    /// active `except` handler, attach the currently-handled exception as
    /// the new exception's `__context__`.  Skipped if `__context__` is
    /// already set (e.g. via prior `raise X from Y`) or if the new
    /// exception IS the currently-handled one (a bare re-raise) — both
    /// cases would create a self-referential cycle.
    pub(crate) fn attach_implicit_context(&self, exc: &Value) {
        let Some(ctx) = self.handled_exc_stack.last() else {
            return;
        };
        let ValueKind::PyInstance(inst) = exc.kind() else {
            return;
        };
        // Avoid setting context to self (bare `raise` inside an except).
        if let ValueKind::PyInstance(ctx_inst) = ctx.kind()
            && Rc::ptr_eq(inst, ctx_inst)
        {
            return;
        }
        let mut borrow = inst.borrow_mut();
        // Don't clobber an existing __context__ (already attached on a
        // previous raise that propagated through here).
        if borrow.attrs.contains_key("__context__") {
            return;
        }
        borrow.attrs.insert("__context__".to_string(), ctx.clone());
    }

    fn coerce_to_exception(&mut self, value: Value) -> Result<Value> {
        match value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if is_exception_class(&instance.borrow().class) {
                    Ok(Value::py_instance(instance))
                } else {
                    Err(pyrust_core::type_err!("exceptions must derive from BaseException"))
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if is_exception_class(&class) {
                    // Use call_class_expanded so that user-defined __init__ is
                    // invoked (e.g. `raise MyError` where MyError.__init__ has
                    // default args).  Mirrors CPython's do_raise behaviour.
                    self.call_class_expanded(class, &[])
                } else {
                    Err(pyrust_core::type_err!("exceptions must derive from BaseException"))
                }
            }
            _ => Err(pyrust_core::type_err!("exceptions must derive from BaseException")),
        }
    }

    /// Validate and coerce a `raise X from Y` cause value.
    ///
    /// CPython accepts `None` (clears cause) or any `BaseException` instance/
    /// subclass as cause.  A class is auto-instantiated with no args, matching
    /// CPython's `ceval.c::do_raise`.  Anything else raises
    /// `TypeError: exception causes must derive from BaseException`.
    fn coerce_to_exception_cause(&mut self, value: Value) -> Result<Value> {
        if value.is_none() {
            return Ok(value);
        }
        match value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if is_exception_class(&instance.borrow().class) {
                    Ok(Value::py_instance(instance))
                } else {
                    Err(pyrust_core::type_err!("exception causes must derive from BaseException"))
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if is_exception_class(&class) {
                    // Use call_class_expanded so that user-defined __init__ is
                    // invoked when a class is used as a cause.
                    self.call_class_expanded(class, &[])
                } else {
                    Err(pyrust_core::type_err!("exception causes must derive from BaseException"))
                }
            }
            _ => Err(pyrust_core::type_err!("exception causes must derive from BaseException")),
        }
    }

    fn instantiate_named_exception(&self, name: &str, message: String) -> Result<Value> {
        let class = lookup_exc_class(name)
            .ok_or_else(|| PyError::Runtime(format!("built-in exception '{}' is not defined", name)))?;
        let args = if message.is_empty() { vec![] } else { vec![Value::string(message)] };
        Ok(instantiate_exception(class, args))
    }

    /// Like [`instantiate_named_exception`] but stores a raw `Value` as
    /// `args[0]` instead of a `Value::string(message)`.  Used for `KeyError`
    /// so that `e.args[0]` returns the original key object, matching CPython.
    fn instantiate_named_exception_with_value(&self, name: &str, arg: Value) -> Result<Value> {
        let class = lookup_exc_class(name)
            .ok_or_else(|| PyError::Runtime(format!("built-in exception '{}' is not defined", name)))?;
        Ok(instantiate_exception(class, vec![arg]))
    }

    /// Instantiate a `NameError` or `UnboundLocalError` with the CPython 3.12
    /// `.name` instance attribute set to the identifier that was not found.
    ///
    /// `class_name` must be `"NameError"` or `"UnboundLocalError"`.
    /// `name` is the identifier string (or `None` for `UnboundLocalError`).
    fn instantiate_name_error_exception(
        &self,
        class_name: &str,
        message: String,
        name: Option<String>,
    ) -> Result<Value> {
        let class = lookup_exc_class(class_name)
            .ok_or_else(|| PyError::Runtime(format!("built-in exception '{class_name}' is not defined")))?;
        Ok(instantiate_name_error(class, message, name))
    }

    /// Instantiate an `ImportError` or `ModuleNotFoundError` with the CPython
    /// 3.12 `.name` and `.path` instance attributes set.
    ///
    /// `class_name` must be `"ImportError"` or `"ModuleNotFoundError"`.
    fn instantiate_import_error_exception(
        &self,
        class_name: &str,
        message: String,
        module_name: Option<String>,
    ) -> Result<Value> {
        let class = lookup_exc_class(class_name)
            .ok_or_else(|| PyError::Runtime(format!("built-in exception '{class_name}' is not defined")))?;
        Ok(instantiate_import_error(class, message, module_name))
    }

    /// Instantiate an `AttributeError` with the CPython 3.12 `.name` and `.obj`
    /// instance attributes set to the missing attribute name and the receiver.
    fn instantiate_attribute_error_exception(
        &self,
        message: String,
        name: Option<String>,
        obj: Option<Value>,
    ) -> Result<Value> {
        let class = lookup_exc_class("AttributeError")
            .ok_or_else(|| PyError::Runtime("built-in exception 'AttributeError' is not defined".to_string()))?;
        Ok(instantiate_attribute_error(class, message, name, obj))
    }

    /// Instantiate an `OSError` (or subclass) with `errno`, `strerror`, and
    /// `filename` instance attributes set, matching CPython 3.12's behaviour
    /// when raising OS errors from real filesystem operations.
    fn instantiate_os_error_exception(
        &self,
        class_name: &str,
        errno: i64,
        strerror: String,
        filename: Option<String>,
        filename2: Option<String>,
    ) -> Result<Value> {
        let class = lookup_exc_class(class_name).ok_or_else(|| {
            PyError::Runtime(format!("built-in exception '{class_name}' is not defined"))
        })?;
        Ok(instantiate_os_error(class, errno, strerror, filename, filename2))
    }

    /// Instantiate a `UnicodeDecodeError` with all five structured attributes
    /// set from a `PyError::UnicodeDecodeError` variant raised internally (e.g.
    /// from `bytes.decode()`).
    fn instantiate_unicode_decode_error_exception(
        &self,
        encoding: String,
        object: Vec<u8>,
        start: usize,
        end: usize,
        reason: String,
    ) -> Result<Value> {
        let class = lookup_exc_class("UnicodeDecodeError").ok_or_else(|| {
            PyError::Runtime("built-in exception 'UnicodeDecodeError' is not defined".to_string())
        })?;
        Ok(instantiate_unicode_decode_error(class, encoding, object, start, end, reason))
    }

    /// Instantiate a `UnicodeEncodeError` with all five structured attributes
    /// set from a `PyError::UnicodeEncodeError` variant raised internally (e.g.
    /// from `str.encode()`).
    fn instantiate_unicode_encode_error_exception(
        &self,
        encoding: String,
        object: String,
        start: usize,
        end: usize,
        reason: String,
    ) -> Result<Value> {
        let class = lookup_exc_class("UnicodeEncodeError").ok_or_else(|| {
            PyError::Runtime("built-in exception 'UnicodeEncodeError' is not defined".to_string())
        })?;
        Ok(instantiate_unicode_encode_error(class, encoding, object, start, end, reason))
    }

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
    ///                          or `None` if all exceptions were matched
    fn split_exception_group(
        &self,
        group_in: &Value,
        kind: &Value,
    ) -> Result<Option<(Value, Option<Value>)>> {
        // PEP 654: if the active exception is a plain (non-group) exception,
        // wrap it in an ExceptionGroup before filtering — matching CPython's
        // implicit wrapping behaviour for `except*`.
        let group_owned;
        let group = if let ValueKind::PyInstance(inst_rc) = group_in.kind() {
            let cls = Rc::clone(&inst_rc.borrow().class);
            if !class_chain_contains_name(&cls, "BaseExceptionGroup") {
                let is_exception = class_chain_contains_name(&cls, "Exception");
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
                    vec![Value::string(String::new()), Value::tuple(vec![group_in.clone()])],
                );
                &group_owned
            } else {
                group_in
            }
        } else {
            return Ok(None);
        };

        // Must be an instance of BaseExceptionGroup.
        let inst_rc = match group.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok(None),
        };
        let class = Rc::clone(&inst_rc.borrow().class);
        if !class_chain_contains_name(&class, "BaseExceptionGroup") {
            return Ok(None);
        }
        // Read `.exceptions` attribute.
        let exceptions_val = inst_rc.borrow().attrs.get("exceptions").cloned();
        let exceptions = match exceptions_val {
            Some(v) => v,
            None => return Ok(None),
        };
        let items: Vec<Value> = if let Some(t) = exceptions.as_tuple() {
            t.to_vec()
        } else if let Some(l) = exceptions.as_list() {
            l.to_vec()
        } else {
            return Ok(None);
        };
        // Split into matched and remaining.
        let mut matched: Vec<Value> = Vec::new();
        let mut remaining: Vec<Value> = Vec::new();
        for exc_val in &items {
            let exc_inst = match exc_val.kind() {
                ValueKind::PyInstance(i) => Rc::clone(i),
                _ => {
                    remaining.push(exc_val.clone());
                    continue;
                }
            };
            let exc_class = Rc::clone(&exc_inst.borrow().class);
            if self.exception_matches_class(&exc_class, kind)? {
                matched.push(exc_val.clone());
            } else {
                remaining.push(exc_val.clone());
            }
        }
        if matched.is_empty() {
            return Ok(None);
        }
        // Read `.message` attribute.
        let message = inst_rc
            .borrow()
            .attrs
            .get("message")
            .cloned()
            .unwrap_or_else(|| Value::string(String::new()));

        let make_group = |excs: Vec<Value>| -> Value {
            let all_exc = excs.iter().all(|v| {
                if let ValueKind::PyInstance(i) = v.kind() {
                    class_chain_contains_name(&i.borrow().class, "Exception")
                } else {
                    false
                }
            });
            let cls_name = if all_exc { "ExceptionGroup" } else { "BaseExceptionGroup" };
            let cls = lookup_exc_class(cls_name).unwrap_or_else(|| Rc::clone(&class));
            instantiate_exception(cls, vec![message.clone(), Value::tuple(excs)])
        };

        let matched_group = make_group(matched);
        let remaining_group = if remaining.is_empty() {
            None
        } else {
            Some(make_group(remaining))
        };
        Ok(Some((matched_group, remaining_group)))
    }

    /// Check whether an exception `class` (already an Rc<RefCell<PyClass>>) is an
    /// instance of `kind` (a Value that is a PyClass or tuple of PyClass).
    fn exception_matches_class(
        &self,
        exc_class: &Rc<RefCell<PyClass>>,
        kind: &Value,
    ) -> Result<bool> {
        match kind.kind() {
            ValueKind::PyClass(expected) => {
                let expected = Rc::clone(expected);
                if !is_exception_class(&expected) {
                    return Err(pyrust_core::type_err!("catching classes that do not inherit from BaseException is not allowed"));
                }
                Ok(class_is_subclass_of(exc_class, &expected))
            }
            ValueKind::Tuple(items) => {
                let mut matched = false;
                for item in items {
                    match item.kind() {
                        ValueKind::PyClass(expected) => {
                            let expected = Rc::clone(expected);
                            if !is_exception_class(&expected) {
                                return Err(pyrust_core::type_err!("catching classes that do not inherit from BaseException is not allowed"));
                            }
                            if class_is_subclass_of(exc_class, &expected) {
                                matched = true;
                            }
                        }
                        _ => {
                            return Err(pyrust_core::type_err!("catching classes that do not inherit from BaseException is not allowed"));
                        }
                    }
                }
                Ok(matched)
            }
            _ => Err(pyrust_core::type_err!("catching classes that do not inherit from BaseException is not allowed")),
        }
    }

    fn exception_matches(&self, exception: &Value, kind: &Value) -> Result<bool> {
        let instance = match exception.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok(false),
        };

        let raised_class = Rc::clone(&instance.borrow().class);
        match kind.kind() {
            ValueKind::PyClass(expected) => {
                let expected = Rc::clone(expected);
                if !is_exception_class(&expected) {
                    return Err(pyrust_core::type_err!("catching classes that do not inherit from BaseException is not allowed"));
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
                                return Err(pyrust_core::type_err!("catching classes that do not inherit from BaseException is not allowed"));
                            }
                            if class_is_subclass_of(&raised_class, &expected) {
                                matched = true;
                            }
                        }
                        _ => return Err(pyrust_core::type_err!("catching classes that do not inherit from BaseException is not allowed")),
                    }
                }
                Ok(matched)
            }
            _ => Err(pyrust_core::type_err!("catching classes that do not inherit from BaseException is not allowed")),
        }
    }

}
