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
            _ => Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method".to_string(),
            )),
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
                    return Err(PyError::named(
                        "ValueError",
                        "slice step cannot be zero".to_string(),
                    ));
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

    fn coerce_to_exception(&self, value: Value) -> Result<Value> {
        match value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if is_exception_class(&instance.borrow().class) {
                    Ok(Value::py_instance(instance))
                } else {
                    Err(PyError::named(
                        "TypeError",
                        "exceptions must derive from BaseException".to_string(),
                    ))
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if is_exception_class(&class) {
                    Ok(instantiate_exception(class, Vec::new()))
                } else {
                    Err(PyError::named(
                        "TypeError",
                        "exceptions must derive from BaseException".to_string(),
                    ))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                "exceptions must derive from BaseException".to_string(),
            )),
        }
    }

    /// Validate and coerce a `raise X from Y` cause value.
    ///
    /// CPython accepts `None` (clears cause) or any `BaseException` instance/
    /// subclass as cause.  A class is auto-instantiated with no args, matching
    /// CPython's `ceval.c::do_raise`.  Anything else raises
    /// `TypeError: exception causes must derive from BaseException`.
    fn coerce_to_exception_cause(&self, value: Value) -> Result<Value> {
        if value.is_none() {
            return Ok(value);
        }
        match value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if is_exception_class(&instance.borrow().class) {
                    Ok(Value::py_instance(instance))
                } else {
                    Err(PyError::named(
                        "TypeError",
                        "exception causes must derive from BaseException".to_string(),
                    ))
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if is_exception_class(&class) {
                    Ok(instantiate_exception(class, Vec::new()))
                } else {
                    Err(PyError::named(
                        "TypeError",
                        "exception causes must derive from BaseException".to_string(),
                    ))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                "exception causes must derive from BaseException".to_string(),
            )),
        }
    }

    fn instantiate_named_exception(&self, name: &str, message: String) -> Result<Value> {
        let class = lookup_exc_class(name)
            .ok_or_else(|| PyError::Runtime(format!("built-in exception '{}' is not defined", name)))?;
        Ok(instantiate_exception(class, vec![Value::string(message)]))
    }

    /// Like [`instantiate_named_exception`] but stores a raw `Value` as
    /// `args[0]` instead of a `Value::string(message)`.  Used for `KeyError`
    /// so that `e.args[0]` returns the original key object, matching CPython.
    fn instantiate_named_exception_with_value(&self, name: &str, arg: Value) -> Result<Value> {
        let class = lookup_exc_class(name)
            .ok_or_else(|| PyError::Runtime(format!("built-in exception '{}' is not defined", name)))?;
        Ok(instantiate_exception(class, vec![arg]))
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
                    return Err(PyError::Runtime(
                        "except clause must reference an exception class".to_string(),
                    ));
                }
                Ok(class_is_subclass_of(&raised_class, &expected))
            }
            ValueKind::Tuple(items) => {
                for item in items {
                    match item.kind() {
                        ValueKind::PyClass(expected) => {
                            let expected = Rc::clone(expected);
                            if !is_exception_class(&expected) {
                                return Err(PyError::Runtime(
                                    "except clause must reference an exception class".to_string(),
                                ));
                            }
                            if class_is_subclass_of(&raised_class, &expected) {
                                return Ok(true);
                            }
                        }
                        _ => return Err(PyError::Runtime(
                            "except clause must reference an exception class".to_string(),
                        )),
                    }
                }
                Ok(false)
            }
            _ => Err(PyError::Runtime(
                "except clause must reference an exception class".to_string(),
            )),
        }
    }

}
