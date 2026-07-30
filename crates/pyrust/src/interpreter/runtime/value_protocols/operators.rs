impl Interpreter {
    /// Try to call a binary dunder method on `left` (named `method`), then on
    /// `right` (named `rmethod`).  Returns `Some(result)` if a dunder was found
    /// and called, or `None` if neither operand has the method.
    ///
    /// Routes both `UserFunction` (pure-Python class methods) and
    /// `BuiltinFunction` (methods defined via `pyrust_module!`'s
    /// `class { … }` block, e.g. `Counter.__add__`) through
    /// `invoke_class_method` so operator-overloading works for both
    /// kinds of class — issue #331.
    pub(super) fn try_dunder_binary(
        &mut self,
        left: &Value,
        right: &Value,
        method: &str,
        rmethod: &str,
    ) -> Option<Result<Value>> {
        // Subtype priority (mirrors CPython `binary_op1`): when `rmethod` is a
        // reflected arithmetic slot (e.g. `__radd__`, starts with `__r`) and
        // `right`'s class is a *proper* subtype of `left`'s class AND `right`'s
        // resolved `rmethod` slot (via MRO) differs from `left`'s resolved slot
        // (one is None, or they're different functions), try
        // `right.rmethod(left)` before `left.method(right)`.  This mirrors
        // CPython's `slotw != slotv` check in `binary_op1`: a right type that
        // inherits a different `__radd__` from an intermediate class gets
        // priority, not only types that directly define `rmethod` in their own
        // `__dict__`.  Comparison reflected ops (`__gt__`, `__ge__`, …) do not
        // start with `__r`, so they are unaffected by this check.
        let right_has_subtype_priority = rmethod.starts_with("__r") && {
            if let (ValueKind::PyInstance(li), ValueKind::PyInstance(ri)) =
                (left.kind(), right.kind())
            {
                let lc = Rc::clone(&li.borrow().class);
                let rc_class = Rc::clone(&ri.borrow().class);
                if !Rc::ptr_eq(&lc, &rc_class) && class_is_subclass_of(&rc_class, &lc) {
                    let right_slot = lookup_class_attr(&rc_class, rmethod);
                    let left_slot = lookup_class_attr(&lc, rmethod);
                    right_slot.is_some() && right_slot != left_slot
                } else {
                    false
                }
            } else {
                false
            }
        };

        if right_has_subtype_priority && let ValueKind::PyInstance(inst) = right.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, rmethod) {
                match self.dispatch_binary_slot(m, right, inst, left) {
                    Some(Ok(v)) if is_not_implemented(&v) => {}
                    Some(result) => return Some(result),
                    None => {}
                }
            }
        }

        if let ValueKind::PyInstance(inst) = left.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method) {
                match self.dispatch_binary_slot(m, left, inst, right) {
                    Some(Ok(v)) if is_not_implemented(&v) => {}
                    Some(result) => return Some(result),
                    None => {}
                }
            }
        }

        // Same-type skip (mirrors CPython `binary_op1`): when the forward slot
        // already ran and returned NotImplemented, and both operands are the
        // *same* type, CPython sets the reflected slot `slotw` to NULL because
        // it would be identical to the forward slot already tried — so the
        // reflected method is not called and the op falls through to TypeError.
        // Scoped to reflected arithmetic slots (`__r*`): comparison reflected
        // ops (`__gt__`, `__ge__`, …) do not start with `__r`, and CPython
        // *does* try both sides for same-type comparisons, so they must stay
        // unaffected.  See issue #2092.
        let same_type_reflected_arith = rmethod.starts_with("__r")
            && matches!(
                (left.kind(), right.kind()),
                (ValueKind::PyInstance(li), ValueKind::PyInstance(ri))
                    if Rc::ptr_eq(&li.borrow().class, &ri.borrow().class)
            );
        if !right_has_subtype_priority
            && !same_type_reflected_arith
            && let ValueKind::PyInstance(inst) = right.kind()
        {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, rmethod) {
                match self.dispatch_binary_slot(m, right, inst, left) {
                    Some(Ok(v)) if is_not_implemented(&v) => {}
                    Some(result) => return Some(result),
                    None => {}
                }
            }
        }
        None
    }

    /// Evaluate one operand's `__ne__` step in CPython's `do_richcompare(Py_NE)`
    /// for a `PyInstance` `owner` against `other` (issue #2648).
    ///
    /// Returns:
    /// - `Some(Ok(v))` — the step produced a definitive (non-NotImplemented)
    ///   result `v` (the caller returns it);
    /// - `Some(Err(..))` — the slot raised;
    /// - `None` — the step yielded NotImplemented, so the caller should try the
    ///   next step (reflected operand, then identity).
    ///
    /// A *user-defined* `__ne__` is dispatched directly.  The inherited default
    /// (`object.__ne__`) is `not owner.__eq__(other)` *single-sided*: it negates
    /// only `owner`'s own `__eq__` (not the full `==`, which would also try the
    /// reflected `__eq__`), and stays NotImplemented when that `__eq__` is
    /// NotImplemented.  Verified against python3.12 `object.__ne__`.
    pub(super) fn pyinstance_ne_step(
        &mut self,
        owner: &Value,
        other: &Value,
    ) -> Option<Result<Value>> {
        let ValueKind::PyInstance(inst) = owner.kind() else {
            return None;
        };
        let class = Rc::clone(&inst.borrow().class);
        // User-defined `__ne__` wins outright.
        if lookup_class_attr(&class, "__ne__")
            .as_ref()
            .is_some_and(|method| {
                !crate::interpreter::value_is_canonical_slot(
                    method,
                    crate::interpreter::CanonicalSlot::ObjectNe,
                )
            })
        {
            let m = lookup_class_attr(&class, "__ne__")?;
            return match self.dispatch_binary_slot(m, owner, inst, other) {
                Some(Ok(v)) if is_not_implemented(&v) => None,
                other => other,
            };
        }
        // Default `object.__ne__`: negate `owner.__eq__(other)` single-sided.
        let eq = lookup_class_attr(&class, "__eq__")?;
        match self.dispatch_binary_slot(eq, owner, inst, other) {
            Some(Ok(v)) if is_not_implemented(&v) => None,
            Some(Ok(v)) => Some(
                self.truthy_value(&v)
                    .map(|is_truthy| Value::bool_(!is_truthy)),
            ),
            other => other,
        }
    }

    /// Dispatch a resolved binary-operator slot `m` (the value of e.g.
    /// `type(owner).__add__`) found on `owner` (a `PyInstance` backed by `inst`)
    /// with `other` as the single operand argument.
    ///
    /// Returns:
    /// - `Some(Ok(v))` when the slot was invoked (the result may be
    ///   `NotImplemented`, which the caller treats as "try the next slot");
    /// - `Some(Err(..))` when the slot raised, OR when the slot exists but is
    ///   *non-callable* (issue #2055: `__add__ = 5` → `TypeError: 'int' object
    ///   is not callable`);
    /// - `None` is never returned (the slot was already found by the caller);
    ///   it is kept in the signature only so callers read uniformly.
    fn dispatch_binary_slot(
        &mut self,
        m: Value,
        owner: &Value,
        inst: &Rc<RefCell<PyInstance>>,
        other: &Value,
    ) -> Option<Result<Value>> {
        if !slot_is_dispatchable(&m) {
            return Some(Err(PyError::named(
                "TypeError",
                format!("'{}' object is not callable", value_type_name_str(&m)),
            )));
        }
        // BuiltinFunction dunders (e.g. `int.__radd__`) operate on the backing
        // primitive value.  Pass the coerced value so `eval_binary` inside the
        // dunder doesn't re-dispatch to the same method on the still-wrapped
        // PyInstance (infinite loop).
        let self_val = if matches!(m.kind(), ValueKind::BuiltinFunction(_)) {
            coerce_numeric(owner)
        } else {
            Value::py_instance(Rc::clone(inst))
        };
        let arg = ExpandedCallArg {
            name: None,
            value: other.clone(),
        };
        Some(invoke_class_method(self, m, self_val, &[arg]))
    }

    /// Try to call a unary dunder method on a PyInstance.  Routes both
    /// `UserFunction` and `BuiltinFunction` class methods through
    /// `invoke_class_method` — same parity with `try_dunder_binary`.
    pub(crate) fn try_dunder_unary(&mut self, val: &Value, method: &str) -> Option<Result<Value>> {
        if let ValueKind::PyInstance(inst) = val.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method) {
                // Issue #2055: a slot that exists but is non-callable
                // (`__neg__ = 5`) raises `TypeError: 'int' object is not
                // callable`, matching CPython, rather than silently skipping.
                if !slot_is_dispatchable(&m) {
                    return Some(Err(PyError::named(
                        "TypeError",
                        format!("'{}' object is not callable", value_type_name_str(&m)),
                    )));
                }
                // BuiltinFunction dunders operate on the backing primitive value;
                // pass the coerced value so they don't reject the PyInstance wrapper.
                let self_val = if matches!(m.kind(), ValueKind::BuiltinFunction(_)) {
                    coerce_numeric(val)
                } else {
                    Value::py_instance(Rc::clone(inst))
                };
                return Some(invoke_class_method(self, m, self_val, &[]));
            }
        }
        None
    }

    /// Three-way ordering comparison that dispatches `__lt__` / `__gt__` on
    /// user instances before falling back to `compare_values` for primitives.
    ///
    /// Used by `sorted()` and `min()` — always tries `__lt__` first,
    /// matching CPython's `Py_LT`-primary reduction for those builtins.
    /// For `max()` call `richcmp_order_gt` instead, which mirrors CPython's
    /// `Py_GT`-primary reduction and emits `'>' not supported` on error.
    ///
    /// Algorithm:
    /// 1. If neither operand is a `PyInstance`, delegate to `compare_values`
    ///    (fast primitive path — zero overhead for the common int/str case).
    /// 2. Try `a < b` via `__lt__` on `a` / `__gt__` on `b`.  If truthy →
    ///    `Less`.
    /// 3. If step 2 returned falsy, try `b < a` to distinguish `Equal` from
    ///    `Greater`.
    /// 4. If no dunder was found, fall through to `compare_values` (which
    ///    raises `TypeError: '<' not supported …`, matching CPython `min` /
    ///    `sorted` behaviour).
    pub(crate) fn richcmp_order(&mut self, a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
        use std::cmp::Ordering;

        // Fast path: neither operand is a user instance.
        if !matches!(a.kind(), ValueKind::PyInstance(_))
            && !matches!(b.kind(), ValueKind::PyInstance(_))
        {
            let primitive_result = compare_values(a, b);
            if primitive_result.is_ok() {
                return primitive_result;
            }
            // A concrete list/tuple can contain a user instance at any depth.
            // `compare_values` deliberately remains interpreter-free, so it
            // reports TypeError when it reaches such an element.  Retry only
            // that failed sequence case through the interpreter-aware
            // lexicographic path.  Successful primitive comparisons above
            // retain their existing one-pass fast path.
            if let Some(sequence_result) = self.richcmp_sequence_order(a, b, false) {
                return sequence_result;
            }
            return primitive_result;
        }

        // Issue #1934/#1939: a builtin-subclass operand (int/float/str/bytes/
        // list/tuple/… subclass) with no user comparison override inherits the
        // base type's ordering, so `min`/`max`/`sorted` must compare via the
        // backing value (`min(F(1.0), F(2.0))`, `sorted([L([2]), [1]])`).
        // Coerce each side to its backing when the subclass doesn't override
        // `__lt__`/`__gt__`/`__le__`/`__ge__`, then recurse so the primitive
        // fast path runs.  A genuine user comparison dunder is left intact and
        // dispatched below.
        const ORDER_OVERRIDES: &[&str] = &["__lt__", "__gt__", "__le__", "__ge__"];
        let a_b = coerce_subclass_backing(a, ORDER_OVERRIDES);
        let b_b = coerce_subclass_backing(b, ORDER_OVERRIDES);
        if a_b.is_some() || b_b.is_some() {
            let a_c = a_b.unwrap_or_else(|| a.clone());
            let b_c = b_b.unwrap_or_else(|| b.clone());
            return self.richcmp_order(&a_c, &b_c);
        }

        // Try a < b (dispatches __lt__ on a, then __gt__ on b).
        match self.try_dunder_binary(a, b, "__lt__", "__gt__") {
            Some(Ok(v)) => {
                let lt = self.truthy_value(&v)?;
                if lt {
                    return Ok(Ordering::Less);
                }
                // a is not less than b; try b < a to tell Equal from Greater.
                match self.try_dunder_binary(b, a, "__lt__", "__gt__") {
                    Some(Ok(v2)) => Ok(if self.truthy_value(&v2)? {
                        Ordering::Greater
                    } else {
                        Ordering::Equal
                    }),
                    Some(Err(e)) => Err(e),
                    // No reverse dunder found: incomparable pair — raise
                    // TypeError just as CPython does for these builtins.
                    None => compare_values(a, b),
                }
            }
            Some(Err(e)) => Err(e),
            // No __lt__/__gt__ on either operand; fall through to primitive
            // comparison, which raises TypeError for incomparable instance
            // pairs — matches CPython's behaviour when no comparison dunder
            // is defined.
            None => compare_values(a, b),
        }
    }

    /// Three-way ordering comparison for `max()` — tries `__gt__` first,
    /// matching CPython's `Py_GT`-primary reduction for `max`.
    ///
    /// Emits `TypeError: '>' not supported …` (not `'<'`) when no comparison
    /// dunder is found, matching CPython 3.12 parity for `max()`.
    ///
    /// Algorithm mirrors `richcmp_order` but with primary/reflected dunders
    /// swapped and the fallback error using `>` instead of `<`.
    pub(crate) fn richcmp_order_gt(&mut self, a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
        use std::cmp::Ordering;

        // Fast path: neither operand is a user instance.
        if !matches!(a.kind(), ValueKind::PyInstance(_))
            && !matches!(b.kind(), ValueKind::PyInstance(_))
        {
            let primitive_result = compare_values_with_op(a, b, ">");
            if primitive_result.is_ok() {
                return primitive_result;
            }
            if let Some(sequence_result) = self.richcmp_sequence_order(a, b, true) {
                return sequence_result;
            }
            return primitive_result;
        }

        // Issue #1934/#1939: coerce builtin-subclass operands to their backing
        // (mirrors `richcmp_order`) so `max(...)` over subclass elements
        // compares via the inherited base-type ordering.
        const ORDER_OVERRIDES: &[&str] = &["__lt__", "__gt__", "__le__", "__ge__"];
        let a_b = coerce_subclass_backing(a, ORDER_OVERRIDES);
        let b_b = coerce_subclass_backing(b, ORDER_OVERRIDES);
        if a_b.is_some() || b_b.is_some() {
            let a_c = a_b.unwrap_or_else(|| a.clone());
            let b_c = b_b.unwrap_or_else(|| b.clone());
            return self.richcmp_order_gt(&a_c, &b_c);
        }

        // Try a > b (dispatches __gt__ on a, then __lt__ on b).
        match self.try_dunder_binary(a, b, "__gt__", "__lt__") {
            Some(Ok(v)) => {
                let gt = self.truthy_value(&v)?;
                if gt {
                    return Ok(Ordering::Greater);
                }
                // a is not greater than b; try b > a to tell Equal from Less.
                match self.try_dunder_binary(b, a, "__gt__", "__lt__") {
                    Some(Ok(v2)) => Ok(if self.truthy_value(&v2)? {
                        Ordering::Less
                    } else {
                        Ordering::Equal
                    }),
                    Some(Err(e)) => Err(e),
                    // No reverse dunder found: incomparable pair — raise
                    // TypeError just as CPython does for max().
                    None => Err(pyrust_core::type_err!(
                        "'>' not supported between instances of '{}' and '{}'",
                        value_type_name_str(a),
                        value_type_name_str(b),
                    )),
                }
            }
            Some(Err(e)) => Err(e),
            // No __gt__/__lt__ on either operand; emit '>' error matching
            // CPython's max() TypeError wording.
            None => Err(pyrust_core::type_err!(
                "'>' not supported between instances of '{}' and '{}'",
                value_type_name_str(a),
                value_type_name_str(b),
            )),
        }
    }

    /// Interpreter-aware lexicographic ordering for concrete list/tuple pairs.
    ///
    /// This is a slow-path companion to the interpreter-free `compare_values`:
    /// callers reach it only after the primitive comparator failed.  CPython
    /// tests each pair for equality before applying the requested ordering op
    /// to the first unequal pair, so the prefix uses `values_user_eq` and the
    /// differing pair recursively re-enters the appropriate rich comparison.
    ///
    /// Values are snapshotted before user code runs.  Besides avoiding a
    /// `RefCell` borrow across `__eq__`/`__lt__`/`__gt__`, this matches the
    /// ownership boundary used by the other interpreter-aware container
    /// protocols.
    fn richcmp_sequence_order(
        &mut self,
        a: &Value,
        b: &Value,
        greater_primary: bool,
    ) -> Option<Result<std::cmp::Ordering>> {
        let (left, right): (Vec<Value>, Vec<Value>) = match (a.kind(), b.kind()) {
            (ValueKind::List(left), ValueKind::List(right)) => (
                left.iter().cloned().collect(),
                right.iter().cloned().collect(),
            ),
            (ValueKind::Tuple(left), ValueKind::Tuple(right)) => (left.to_vec(), right.to_vec()),
            _ => return None,
        };

        Some((|| {
            for (left_item, right_item) in left.iter().zip(right.iter()) {
                if self.values_user_eq(left_item, right_item)? {
                    continue;
                }
                return if greater_primary {
                    self.richcmp_order_gt(left_item, right_item)
                } else {
                    self.richcmp_order(left_item, right_item)
                };
            }
            Ok(left.len().cmp(&right.len()))
        })())
    }
}
