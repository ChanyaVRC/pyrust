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

    /// Arbitrary-precision `slice.indices(len)` for big-bound range slicing
    /// (#2118).  Mirrors [`Self::resolve_slice_bounds`] but in `BigInt` so a
    /// range whose *length* exceeds i64 (`range(10**20)[:5]`) still slices
    /// correctly instead of overflowing.  `lo`/`hi`/`st` come straight from the
    /// slice object (already `__index__`-resolved to int-like values).
    fn resolve_slice_bounds_big(
        len: &PyBigInt,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<(PyBigInt, PyBigInt, PyBigInt)> {
        let zero = PyBigInt::from(0);
        let one = PyBigInt::from(1);
        let to_big = |v: &Value| -> Result<PyBigInt> {
            value_to_bigint(v).ok_or_else(|| {
                pyrust_core::type_err!(
                    "slice indices must be integers or None or have an __index__ method"
                )
            })
        };
        let step = match st {
            None => one.clone(),
            Some(v) if v.is_none() => one.clone(),
            Some(v) => {
                let s = to_big(v)?;
                if s == zero {
                    return Err(pyrust_core::value_err!("slice step cannot be zero"));
                }
                s
            }
        };
        let step_pos = step.sign() == PyBigIntSign::Plus;
        // clamp(idx, lo, hi)
        let clamp = |idx: PyBigInt, low: &PyBigInt, high: &PyBigInt| -> PyBigInt {
            if idx < *low {
                low.clone()
            } else if idx > *high {
                high.clone()
            } else {
                idx
            }
        };
        let len_minus_1 = len - &one;
        let resolve = |v: &Value| -> Result<PyBigInt> {
            let i = to_big(v)?;
            let i = if i.sign() == PyBigIntSign::Minus { i + len } else { i };
            Ok(if step_pos {
                clamp(i, &zero, len)
            } else {
                // negative step lower bound is -1
                clamp(i, &(-&one), &len_minus_1)
            })
        };
        let start_default = if step_pos { zero.clone() } else { len_minus_1.clone() };
        let end_default = if step_pos { len.clone() } else { -&one };
        let start = match lo {
            None => start_default,
            Some(v) if v.is_none() => start_default,
            Some(v) => resolve(v)?,
        };
        let end = match hi {
            None => end_default,
            Some(v) if v.is_none() => end_default,
            Some(v) => resolve(v)?,
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
                return Err(pyrust_core::value_err!(
                    "attempt to assign sequence of size {} to extended slice of size {}",
                    new_items.len(),
                    indices.len()
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
        borrow.attrs.insert("__context__", ctx.clone());
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
    ///     or `None` if all exceptions were matched
    fn split_exception_group(
        &mut self,
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
        match group.kind() {
            ValueKind::PyInstance(i)
                if class_chain_contains_name(&i.borrow().class, "BaseExceptionGroup") => {}
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
            .get("message")
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
                    class_chain_contains_name(&i.borrow().class, "Exception")
                } else {
                    false
                }
            });
        let cls_name = if all_exc { "ExceptionGroup" } else { "BaseExceptionGroup" };
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
                    &[ExpandedCallArg { name: None, value: exc.clone() }],
                )?;
                Ok(res.truthy())
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
            match borrowed.attrs.get("exceptions") {
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
                if class_chain_contains_name(&i.borrow().class, "BaseExceptionGroup"));
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
                    ExpandedCallArg { name: None, value: group.clone() },
                    ExpandedCallArg { name: None, value: Value::list(excs) },
                ],
            )?,
            None => {
                // No derive in MRO (shouldn't happen): fall back to default.
                let message = match group.kind() {
                    ValueKind::PyInstance(i) => i
                        .borrow()
                        .attrs
                        .get("message")
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
            && !Rc::ptr_eq(src, dst) {
                let src_b = src.borrow();
                let mut dst_b = dst.borrow_mut();
                for key in ["__traceback__", "__cause__", "__context__", "__notes__"] {
                    if let Some(v) = src_b.attrs.get(key) {
                        dst_b.attrs.insert(key, v.clone());
                    }
                }
            }
        Ok(derived)
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

/// A PEP 654 `subgroup`/`split` condition, classified into either an
/// exception type / tuple-of-types (matched with `isinstance` semantics) or a
/// callable predicate `(exc) -> bool`.
enum EgMatcher {
    Type(Value),
    Predicate(Value),
}
