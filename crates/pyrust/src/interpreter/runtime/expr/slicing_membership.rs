impl Interpreter {
    /// Resolve one slice bound through the `__index__` protocol if needed.
    ///
    /// Used for the built-in sequence path (List/Tuple/Str/Bytes) where the
    /// caller needs a concrete integer from each bound.  `PyInstance` and
    /// `BuiltinObject` targets receive the raw (unresolved) bound values so
    /// that user `__getitem__` implementations see the original objects — the
    /// same as CPython: `a[Index(2):]` calls `list.__getitem__` which then
    /// applies `__index__`; `my_obj[Index(2):]` delivers `slice(Index(2),
    /// None, None)` unchanged to `my_obj.__getitem__`.
    ///
    /// `None` (a missing bound, e.g. `a[:]`) and Python `None` are passed
    /// through as-is.  `Int`, `Bool`, and `BigInt` are returned unchanged.
    /// `PyInstance` values that define `__index__` are called and the integer
    /// result is returned.  Anything else is left to `slice_index_from_value`
    /// to reject with a proper TypeError.
    fn resolve_slice_bound_val(&mut self, val: Option<Value>) -> Result<Option<Value>> {
        let v = match val {
            None => return Ok(None),
            Some(v) => v,
        };
        // Fast path: already an integer type or Python None — no protocol call needed.
        if v.is_none()
            || matches!(
                v.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return Ok(Some(v));
        }
        // Slow path: try __index__.
        let resolved = self.resolve_index_arg(v)?;
        Ok(Some(resolved))
    }

    /// Resolve a value assigned to a bytearray element (`ba[i] = v`) through
    /// the `__index__` protocol (#1908).  A `PyInstance` defining `__index__`
    /// is called and its integer result returned (`__index__ returned non-int`
    /// on a bad return).  Every other value — including plain ints, floats, and
    /// `__int__`-only objects — is returned unchanged so the receiver-side
    /// `value_to_byte` produces the correct range / type error verbatim.
    fn resolve_byte_value(&mut self, v: Value) -> Result<Value> {
        if !matches!(v.kind(), ValueKind::PyInstance(_)) {
            return Ok(v);
        }
        // Route dispatch through the optional shared protocol. A missing slot
        // maps back to the original value so receiver-side validation retains
        // its context-specific TypeError; slot errors still propagate.
        match self.try_value_to_index(&v)? {
            Some(resolved) => Ok(resolved),
            None => Ok(v),
        }
    }

    fn eval_slice(
        &mut self,
        target: &Value,
        lo: Option<Value>,
        hi: Option<Value>,
        st: Option<Value>,
    ) -> Result<Value> {
        // PyInstance: dispatch __getitem__ with a slice object built from the
        // raw (unresolved) bounds.  CPython passes the bound objects as-is so
        // that the user's __getitem__ sees them; resolution via __index__ is
        // the caller's responsibility (e.g. when the user delegates back to a
        // built-in sequence).
        if let ValueKind::PyInstance(inst) = target.kind() {
            let inst_rc = Rc::clone(inst);
            // Issue #994: if the instance has a backing primitive value
            // (tuple/frozenset/dict/list/set subclass), delegate slice to it.
            // eval_index does the same for integer subscripts; without this,
            // `MyTuple([1,2,3])[1:3]` reaches the __getitem__ branch and
            // raises TypeError because tuple subclasses don't register a
            // user-level __getitem__.
            // Issue #1134: check user __getitem__ before backing fast path,
            // matching the same ordering fix in eval_index.  The builtin
            // sentinels for the base types are not overrides.
            let class = Rc::clone(&inst_rc.borrow().class);
            // PEP 695: a generic `type X[T] = ...` alias is subscriptable with a
            // slice too — `Pair[1:2]` returns a `types.GenericAlias` whose single
            // arg is the (unresolved) `slice` object, repr `Pair[slice(1, 2,
            // None)]`.  A non-generic alias raises the same TypeError as the
            // integer-index path (issue #2779).  Mirrors `eval_index`.
            if is_type_alias_class(&class) {
                let has_params = inst_rc
                    .borrow()
                    .attrs
                    .get("__type_params__")
                    .is_some_and(|p| matches!(p.kind(), ValueKind::Tuple(t) if !t.is_empty()));
                if !has_params {
                    return Err(pyrust_core::type_err!(
                        "Only generic type aliases are subscriptable"
                    ));
                }
                let slice_val = make_slice_value(lo, hi, st);
                return Ok(pyrust_builtins::generic_alias::generic_alias(
                    Value::py_instance(inst_rc),
                    Value::tuple(vec![slice_val]),
                ));
            }
            let user_getitem = lookup_class_attr(&class, "__getitem__").filter(|v| {
                inherited_primitive_builtin_slot_kind(&class, "__getitem__", v).is_none()
            });
            if let Some(method_val) = user_getitem {
                let slice_val = make_slice_value(lo, hi, st);
                return invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[ExpandedCallArg {
                        name: None,
                        value: slice_val,
                    }],
                );
            }
            if let Some(backing) = builtin_data_backing(target) {
                return self.eval_slice(&backing, lo, hi, st);
            }
            return Err(pyrust_core::type_err!(
                "'{}' object is not subscriptable",
                pyrust_core::error_type_name(target)
            ));
        }

        // BuiltinObject: delegate to ops.get_item with a slice value (issue #847).
        // This mirrors what eval_index does when a runtime slice object is used
        // as a subscript, and lets BuiltinObject types opt into slice subscripting
        // via BuiltinTypeOps::get_item.  bytearray's receiver-only get_item can't
        // reach user dunders, so resolve any __index__ bounds here first (#1908);
        // other BuiltinObject types resolve internally so are passed raw.
        if let ValueKind::BuiltinObject { ops, .. } = target.kind()
            && ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray)
        {
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let slice_val = make_slice_value(lo, hi, st);
            let ValueKind::BuiltinObject { ops, state } = target.kind() else {
                unreachable!("target kind checked above");
            };
            return ops.get_item(state, &slice_val);
        }
        if let ValueKind::BuiltinObject { ops, state } = target.kind() {
            let slice_val = make_slice_value(lo, hi, st);
            return ops.get_item(state, &slice_val);
        }

        // Range slicing: compute the result range arithmetically, matching
        // CPython's range.__getitem__ for slice arguments.  Handled before
        // the general built-in sequence path so we never materialise elements.
        //
        // CPython's algorithm (Objects/rangeobject.c):
        //   (sl_start, sl_stop, sl_step) = slice.indices(len(r))
        //   new_start = r.start + sl_start * r.step
        //   new_stop  = r.start + sl_stop  * r.step  ← note: uses r.start, not new_start
        //   new_step  = r.step  * sl_step
        if let ValueKind::Range {
            start: r_start,
            stop: r_stop,
            step: r_step,
        } = target.kind()
        {
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let r_len = range_len(r_start, r_stop, r_step);
            // Preserve the allocation-free common path when both the logical
            // length and derived range fields fit i64.  Every operation is
            // checked because a valid slice may have a one-past-end stop or a
            // multiplied step outside i64 even when the source bounds fit.
            if let Ok(r_len_narrow) = i64::try_from(r_len) {
                let (sl_start, sl_stop, sl_step) = Self::resolve_slice_bounds(
                    r_len_narrow,
                    lo.as_ref(),
                    hi.as_ref(),
                    st.as_ref(),
                )?;
                let fields = sl_start
                    .checked_mul(r_step)
                    .and_then(|offset| r_start.checked_add(offset))
                    .zip(
                        sl_stop
                            .checked_mul(r_step)
                            .and_then(|offset| r_start.checked_add(offset)),
                    )
                    .zip(r_step.checked_mul(sl_step));
                if let Some(((new_start, new_stop), new_step)) = fields {
                    return Ok(Value::range(new_start, new_stop, new_step));
                }
            }

            // Wide i64-backed ranges can contain up to 2**64-1 elements.
            // Resolve their slice bounds and derived fields exactly, then let
            // `range_big` collapse the result back to the compact form when it
            // happens to fit.
            let r_start = PyBigInt::from(r_start);
            let r_step = PyBigInt::from(r_step);
            let r_len = PyBigInt::from(r_len);
            let (sl_start, sl_stop, sl_step) =
                Self::resolve_slice_bounds_big(&r_len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
            let new_start = &r_start + &sl_start * &r_step;
            let new_stop = &r_start + &sl_stop * &r_step;
            let new_step = &r_step * &sl_step;
            return Ok(Value::range_big(new_start, new_stop, new_step));
        }
        // Arbitrary-precision range slicing (#2118).  CPython resolves the slice
        // indices against the range length as a Python int (not Py_ssize_t), so
        // `range(10**20)[:5]` slices fine even though the length overflows i64.
        // All of len / slice-index resolution / the new start·stop·step run in
        // BigInt arithmetic.
        if let ValueKind::BigRange {
            start: r_start,
            stop: r_stop,
            step: r_step,
        } = target.kind()
        {
            let r_start = r_start.clone();
            let r_step = r_step.clone();
            let r_len = pyrust_core::bigrange_len(&r_start, r_stop, &r_step);
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let (sl_start, sl_stop, sl_step) =
                Self::resolve_slice_bounds_big(&r_len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
            let new_start = &r_start + &sl_start * &r_step;
            let new_stop = &r_start + &sl_stop * &r_step;
            let new_step = &r_step * &sl_step;
            return Ok(Value::range_big(new_start, new_stop, new_step));
        }

        // Built-in sequences: resolve bounds through __index__ before applying
        // the integer arithmetic in resolve_slice_bounds (issue #849).
        let lo = self.resolve_slice_bound_val(lo)?;
        let hi = self.resolve_slice_bound_val(hi)?;
        let st = self.resolve_slice_bound_val(st)?;

        // For str, the slice bounds are char-based; computing `len` as the char
        // count is O(n), so the contiguous fast path below resolves char->byte
        // offsets in a single forward scan instead of materialising Vec<char>.
        //
        // ASCII fast path (#2032): an all-ASCII string has char index == byte
        // index, so `len` is `s.len()` and every slice/index is direct byte
        // arithmetic — no char scan at all.  ASCII-ness is cached on the string
        // header (#2124), so the check is O(1) and we reuse the flag for both the
        // length and the contiguous/stepped slice arms below.
        let str_is_ascii = target.is_str() && target.str_is_ascii();
        let len = match target.kind() {
            ValueKind::List(items) => items.len() as i64,
            ValueKind::Tuple(items) => items.len() as i64,
            ValueKind::Str(s) if str_is_ascii => s.len() as i64,
            ValueKind::Str(_) => target.str_codepoint_len_for_index() as i64,
            ValueKind::Bytes(rc) => rc.len() as i64,
            _ => {
                return Err(pyrust_core::type_err!(
                    "'{}' object is not subscriptable",
                    pyrust_core::error_type_name(target)
                ));
            }
        };
        let (start, end, step) =
            Self::resolve_slice_bounds(len, lo.as_ref(), hi.as_ref(), st.as_ref())?;

        // Full-slice identity short-circuit (#2277): CPython's `tuple` / `bytes`
        // `__getitem__` return the original object when the resolved slice
        // covers the whole sequence with unit step (`start == 0 && end == len &&
        // step == 1`), so `t[:] is t`, `t[0:len(t)] is t`, `t[::1] is t`,
        // `t[0:100] is t` (stop clamps to len) all hold.  The Rc-shared tuple
        // backing (#2268) and Rc-shared bytes make the clone identity-preserving
        // (`value_id` reads the same obj_id / Rc pointer), so this is cheap and
        // correct.  `list` is excluded — `l[:]` always copies in CPython.
        //
        // `str` is intentionally NOT short-circuited: pyrust strings have no
        // stable object identity even under plain aliasing (`x = s; x is s` is
        // already False), so a full str slice cannot match CPython's `s[:] is s`
        // == True regardless of what this returns.  That is a broader str
        // identity gap tracked separately, not a slice bug.
        if step == 1
            && start == 0
            && end == len
            && matches!(target.kind(), ValueKind::Tuple(_) | ValueKind::Bytes(_))
        {
            return Ok(target.clone());
        }

        // Contiguous fast path: `step == 1` produces the contiguous run
        // `[start, end)` (memcpy for bytes, range clone for list/tuple, zero-copy
        // shared-buffer slice for ASCII str) — see #2066 / #2111 / #2116 / #2136.
        // The body lives in `fast_path.rs::fast_slice_contiguous`.
        if step == 1 {
            return fast_slice_contiguous(target, start, end, str_is_ascii);
        }

        let indices = Self::slice_target_indices(len, start, end, step);

        match target.kind() {
            ValueKind::List(items) => Ok(Value::list(
                indices
                    .into_iter()
                    .map(|ix| items[ix].clone())
                    .collect::<Vec<Value>>(),
            )),
            ValueKind::Tuple(items) => Ok(Value::tuple(
                indices
                    .into_iter()
                    .map(|ix| items[ix].clone())
                    .collect::<Vec<Value>>(),
            )),
            ValueKind::Str(s) if str_is_ascii => {
                // ASCII fast path (#2032): char index == byte index, so index the
                // bytes directly — no Vec<char> materialisation.
                let bytes = s.as_bytes();
                let out: String = indices.into_iter().map(|ix| bytes[ix] as char).collect();
                Ok(Value::string(out))
            }
            ValueKind::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let mut out = String::new();
                for ix in indices {
                    out.push(chars[ix]);
                }
                Ok(Value::string(out))
            }
            ValueKind::Bytes(rc) => {
                Ok(Value::bytes(indices.into_iter().map(|ix| rc[ix]).collect()))
            }
            _ => unreachable!(),
        }
    }

    /// `item in items` for a list/tuple element slice, dispatching user
    /// `__eq__` only when an operand can fire it.
    ///
    /// Single-pass fast scan (mirrors `call_seq_remove`): when `item` itself
    /// cannot fire user `__eq__`, walk the slice once.  While each element is a
    /// scalar (`cannot_user_eq` — a tag-only check, no `ValueKind` build, no
    /// pointer deref) compare with the primitive `Value::eq`.  On the first
    /// non-scalar element (which might match `item` through its own `__eq__`),
    /// or when `item` can dispatch, snapshot the slice (so a re-entrant user
    /// `__eq__` cannot invalidate the backing store through an alias) and walk
    /// with `values_user_eq`, whose identity short-circuit keeps the mixed
    /// primitive+instance case allocation-light.
    ///
    /// Replaces the previous two-pass shape (a full `needs_dispatch` pre-scan
    /// over every element followed by the membership scan), which was O(n) even
    /// when the match was the first element (#2341).
    fn seq_membership(&mut self, items: &[Value], item: &Value) -> Result<Value> {
        if !Self::value_search_dispatches(item) {
            for elem in items {
                if !elem.cannot_user_eq() {
                    // Non-scalar element: a dispatching element could match
                    // `item` through its own `__eq__`.  Snapshot from the front
                    // and restart on the dispatch path (preserving semantics).
                    let snapshot: Vec<Value> = items.to_vec();
                    for elem in &snapshot {
                        // Identity short-circuit (CPython `PyObject_RichCompareBool`)
                        // before `__eq__` — needed for NaN-bearing complex, which is
                        // non-scalar and so reaches this dispatch branch instead of
                        // the scalar fast path below (#2535).
                        if elem.is_identical_nan(item) || self.values_user_eq(elem, item)? {
                            return Ok(Value::bool_(true));
                        }
                    }
                    return Ok(Value::bool_(false));
                }
                // Identity short-circuit (CPython `PyObject_RichCompareBool`):
                // a NaN searching for itself matches even though `==` is False.
                if elem == item || elem.is_identical_nan(item) {
                    return Ok(Value::bool_(true));
                }
            }
            return Ok(Value::bool_(false));
        }
        // `item` can fire user `__eq__`: snapshot (re-entrancy safety) and walk
        // with full dispatch.
        let snapshot: Vec<Value> = items.to_vec();
        for elem in &snapshot {
            if self.values_user_eq(elem, item)? {
                return Ok(Value::bool_(true));
            }
        }
        Ok(Value::bool_(false))
    }

    /// List membership with the `RefCell` guard limited to one element read.
    ///
    /// Python `__eq__` may mutate the list being searched. Keeping a guarded
    /// slice alive across that callback would turn previously-unguarded UB into
    /// a RefCell panic. Reading the current element by index and cloning only
    /// callback-capable values both permits re-entry and retains CPython's live
    /// list-walk behaviour.
    fn list_membership(&mut self, list: &Value, item: &Value) -> Result<Value> {
        let item_dispatches = Self::value_search_dispatches(item);
        let mut index = 0;
        loop {
            enum Step {
                Match,
                Compare(Value),
                Next,
            }

            let step = {
                let Some(items) = list.as_list() else {
                    return Ok(Value::bool_(false));
                };
                let Some(element) = items.get(index) else {
                    return Ok(Value::bool_(false));
                };
                if !item_dispatches && element.cannot_user_eq() {
                    if element == item || element.is_identical_nan(item) {
                        Step::Match
                    } else {
                        Step::Next
                    }
                } else if element.is_identical_nan(item) {
                    Step::Match
                } else {
                    Step::Compare(element.clone())
                }
            };

            match step {
                Step::Match => return Ok(Value::bool_(true)),
                Step::Compare(element) if self.values_user_eq(&element, item)? => {
                    return Ok(Value::bool_(true));
                }
                Step::Compare(_) | Step::Next => index += 1,
            }
        }
    }

    pub(crate) fn eval_in(&mut self, container: Value, item: Value) -> Result<Value> {
        // Handle Dict/Set separately so the temporary `&IndexMap`/`&IndexSet`
        // from `container.kind()` doesn't outlive the call into
        // `dict_lookup`/`set_lookup` (which may run user `__eq__`).
        if container.is_dict() {
            let found = if let Some(s) = item.as_str() {
                self.dict_str_lookup(&container, s)?.is_some()
            } else {
                let key = self.value_to_pykey(&item)?;
                self.dict_lookup(&container, &key)?.is_some()
            };
            return Ok(Value::bool_(found));
        }
        if container.is_set() {
            let key = self.value_to_pykey(&item)?;
            return Ok(Value::bool_(self.set_lookup(&container, &key)?.is_some()));
        }
        // Frozenset membership — must intercept before the generic BuiltinObject
        // arm because `FrozenSetOps::contains` calls `item.to_key()` which has
        // no interpreter access and cannot dispatch user `__hash__`.  Mirror the
        // Set path above: get the key via `value_to_pykey` (which runs user
        // `__hash__`) then search the underlying `IndexSet` via `set_lookup_in`
        // (which dispatches user `__eq__` for `PyKey::Object` entries).
        if let Some(rc) = pyrust_builtins::frozenset::as_items(&container) {
            let key = self.value_to_pykey(&item)?;
            return Ok(Value::bool_(self.set_lookup_in(&rc, &key)?.is_some()));
        }
        // List and Tuple: `seq_membership` does a single-pass primitive scan
        // and only snapshots + dispatches user `__eq__` when an operand can
        // fire it (see its doc comment for the full contract).
        if container.is_list() {
            return self.list_membership(&container, &item);
        }
        if let Some(items) = container.as_tuple() {
            return self.seq_membership(items, &item);
        }
        match container.kind() {
            ValueKind::List(_) | ValueKind::Tuple(_) => unreachable!("handled above"),
            ValueKind::Set(_) => unreachable!("handled above"),
            ValueKind::BuiltinObject { ops, state } => {
                // bytearray accepts any bytes-like object (bytes subclass,
                // bytearray) as the left operand of `in` (#1928).  Coerce the
                // item for bytearray; other BuiltinObjects (frozenset) keep the
                // original value so their hashing / equality is unaffected.
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) {
                    let item = coerce_bytes_subclass_arg(item);
                    ops.contains(state, &item).map(Value::bool_)
                } else {
                    ops.contains(state, &item).map(Value::bool_)
                }
            }
            ValueKind::Bytes(rc) => {
                // CPython accepts any bytes-like object (bytes subclass,
                // bytearray) as the left operand of `in` (#1928).  Coerce the
                // item to its `Bytes` backing before the match; non-bytes-like
                // values are returned untouched and hit the error arm.
                let item = coerce_bytes_subclass_arg(item);
                match item.kind() {
                    ValueKind::Int(n) if (0..=255).contains(&n) => {
                        Ok(Value::bool_(rc.contains(&(n as u8))))
                    }
                    // bool is a subclass of int in Python; True==1 and False==0 are
                    // valid byte values, so treat them as their integer equivalents.
                    ValueKind::Bool(b) => {
                        Ok(Value::bool_(rc.contains(&(if b { 1u8 } else { 0u8 }))))
                    }
                    ValueKind::Int(_) | ValueKind::BigInt(_) => {
                        Err(pyrust_core::value_err!("byte must be in range(0, 256)"))
                    }
                    ValueKind::Bytes(sub) => Ok(Value::bool_(
                        sub.is_empty()
                            || rc.windows(sub.len()).any(|w| w == sub.as_ref().as_slice()),
                    )),
                    _ => Err(pyrust_core::type_err!(
                        "a bytes-like object is required, not '{}'",
                        value_type_name_str(&item)
                    )),
                }
            }
            ValueKind::Str(s) => {
                // CPython accepts any str subclass as the left operand of `in`
                // (#1927).  Coerce the item to its `Str` backing first.
                let item = coerce_str_subclass_arg(item);
                match item.kind() {
                    ValueKind::Str(sub) => Ok(Value::bool_(s.contains(sub))),
                    _ => Err(pyrust_core::type_err!(
                        "'in <string>' requires string as left operand, not {}",
                        value_type_name_str(&item)
                    )),
                }
            }
            ValueKind::Dict(_) => unreachable!("handled above"),
            ValueKind::Range { start, stop, step } => {
                match item.kind() {
                    ValueKind::Int(v) => Ok(Value::bool_(i64_range_contains(start, stop, step, v))),
                    // bool is a subclass of int; True==1, False==0.
                    ValueKind::Bool(b) => Ok(Value::bool_(i64_range_contains(
                        start, stop, step, b as i64,
                    ))),
                    // BigInt: if it fits in i64 apply the check; if it overflows
                    // it cannot be in any range whose bounds are i64.
                    ValueKind::BigInt(n) => Ok(Value::bool_(
                        n.to_i64()
                            .is_some_and(|v| i64_range_contains(start, stop, step, v)),
                    )),
                    // Float: if the value is an integer-valued finite float,
                    // convert to i64 and do the fast O(1) range check.
                    // Non-integer or non-finite floats cannot equal any integer.
                    // This matches CPython 3.12's range.__contains__ behaviour.
                    //
                    // Bounds are checked before casting to avoid Rust's saturating
                    // f64-to-i64 cast.  float(2**63) and float(2**63-1) are the same
                    // f64 value (both round to 9.223372036854776e18), so the round-trip
                    // check `(f as i64) as f64 == f` does NOT detect saturation at the
                    // positive boundary.  Use strict half-open bounds instead:
                    // i64 range is [-2**63, 2**63), both endpoints are exact f64 values.
                    ValueKind::Float(f) => {
                        // 9223372036854775808.0 == 2**63 as f64 (exactly representable)
                        const I64_MIN_F: f64 = i64::MIN as f64;
                        const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                        let in_range = f.is_finite()
                            && f.fract() == 0.0
                            && (I64_MIN_F..I64_MAX_PLUS1_F).contains(&f)
                            && i64_range_contains(start, stop, step, f as i64);
                        Ok(Value::bool_(in_range))
                    }
                    // Complex: if imaginary part is zero and real part is an
                    // integer-valued finite float, same fast O(1) check.
                    ValueKind::Complex(re, im) => {
                        const I64_MIN_F: f64 = i64::MIN as f64;
                        const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                        let in_range = im == 0.0
                            && re.is_finite()
                            && re.fract() == 0.0
                            && (I64_MIN_F..I64_MAX_PLUS1_F).contains(&re)
                            && i64_range_contains(start, stop, step, re as i64);
                        Ok(Value::bool_(in_range))
                    }
                    _ => Ok(Value::bool_(false)),
                }
            }
            ValueKind::BigRange { start, stop, step } => {
                // Arbitrary-precision range membership (#2118).  Mirrors the i64
                // O(1) check but in BigInt arithmetic: `v` is a member iff it lies
                // within [start, stop) (for positive step, or (stop, start] for
                // negative) and `(v - start)` is divisible by `step`.
                let bigrange_contains = |v: &PyBigInt| -> bool {
                    use pyrust_core::PyBigIntSign;
                    let sgn = step.sign();
                    let in_bounds = if sgn == PyBigIntSign::Plus {
                        v >= start && v < stop
                    } else {
                        v <= start && v > stop
                    };
                    in_bounds && ((v - start) % step).sign() == PyBigIntSign::NoSign
                };
                // Resolve `item` to an integer value when possible (int/bool/bigint,
                // or an integer-valued finite float/complex).  Anything else is a
                // non-member.
                use num_traits::FromPrimitive;
                let int_valued_float = |f: f64| -> Option<PyBigInt> {
                    if f.is_finite() && f.fract() == 0.0 {
                        PyBigInt::from_f64(f)
                    } else {
                        None
                    }
                };
                // A float-literal pattern (`Complex(re, 0.0)`) would trigger the
                // deprecated `illegal_floating_point_literal_pattern` lint, so
                // keep the equality guard despite `redundant_guards`.
                #[allow(clippy::redundant_guards)]
                let as_int: Option<PyBigInt> = match item.kind() {
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                        value_to_bigint(&item)
                    }
                    ValueKind::Float(f) => int_valued_float(f),
                    ValueKind::Complex(re, im) if im == 0.0 => int_valued_float(re),
                    _ => None,
                };
                Ok(Value::bool_(as_int.is_some_and(|v| bigrange_contains(&v))))
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__contains__") {
                    let result = invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[ExpandedCallArg {
                            name: None,
                            value: item.clone(),
                        }],
                    )?;
                    return Ok(Value::bool_(self.truthy_value(&result)?));
                }
                // list/dict/set subclass with no user-defined __contains__:
                // delegate to the backing primitive, matching CPython's
                // inherited tp_sq_contains / sq_contains slot behaviour.
                if let Some(backing) = builtin_data_backing(&container) {
                    return self.eval_in(backing, item);
                }
                // No __contains__ or __builtin_data__: fall back to __iter__ if available.
                if let Some(iter_method) = lookup_class_attr(&class, "__iter__") {
                    let iter_obj = invoke_class_method(
                        self,
                        iter_method,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[],
                    )?;
                    loop {
                        match self.call_next(&iter_obj, None) {
                            Ok(elem) => {
                                if self.values_user_eq(&elem, &item)? {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(ref e) if is_stop_iteration_error(e) => {
                                return Ok(Value::bool_(false));
                            }
                            // Only the canonical class and real subclasses terminate;
                            // any unrelated exception (including a same-named class)
                            // propagates.
                            Err(PyError::Raised(exc)) => return Err(PyError::Raised(exc)),
                            Err(e) => return Err(e),
                        }
                    }
                }
                // Legacy sequence-iter protocol (#394): if the class
                // defines `__getitem__` but no `__iter__`/`__contains__`,
                // walk indices 0, 1, … until IndexError/StopIteration.
                // **Short-circuits** on first match (#416 Copilot
                // review): the lazy iterator stops calling
                // `__getitem__` past the matching index, so a later
                // index raising `RuntimeError` doesn't surface.
                if lookup_class_attr(&class, "__getitem__").is_some() {
                    let iter_val = self.make_getitem_iter(Rc::clone(&inst_rc))?;
                    loop {
                        match self.call_next(&iter_val, None) {
                            Ok(elem) => {
                                if self.values_user_eq(&elem, &item)? {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(ref e) if is_stop_iteration_error(e) => {
                                return Ok(Value::bool_(false));
                            }
                            // Any remaining Raised error is not the canonical
                            // StopIteration class or a real subclass.
                            Err(PyError::Raised(exc)) => return Err(PyError::Raised(exc)),
                            Err(e) => return Err(e),
                        }
                    }
                }
                Err(pyrust_core::type_err!(
                    "argument of type '{}' is not iterable",
                    class.borrow().name
                ))
            }
            // Generators and every native iterator object (map/filter/zip/
            // enumerate/reversed/iter(...)/itertools iterators — all carried by
            // the Generator value tag): CPython's last-resort sq_contains walks
            // the iterator lazily with `__eq__`, short-circuiting on first
            // match (and consuming it up to the hit).  Coroutines and async
            // generators share the tag but are not iterable — they fall through
            // to the TypeError below with their own type name.
            ValueKind::Generator(_)
                if !is_coroutine_value(&container) && !is_async_generator_value(&container) =>
            {
                loop {
                    match self.call_next(&container, None) {
                        Ok(elem) => {
                            if self.values_user_eq(&elem, &item)? {
                                return Ok(Value::bool_(true));
                            }
                        }
                        Err(ref e) if is_stop_iteration_error(e) => {
                            return Ok(Value::bool_(false));
                        }
                        // Only the canonical class and real subclasses terminate;
                        // any other exception propagates.
                        Err(PyError::Raised(exc)) => return Err(PyError::Raised(exc)),
                        Err(e) => return Err(e),
                    }
                }
            }
            // A class whose metaclass defines `__contains__` (e.g. an `Enum`
            // subclass under `EnumMeta`): `member in Color` dispatches the
            // metaclass slot with the class as the receiver (#2611).
            ValueKind::PyClass(cls) if metaclass_dunder(cls, "__contains__").is_some() => {
                let method_val = metaclass_dunder(cls, "__contains__").unwrap();
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_class(Rc::clone(cls)),
                    &[ExpandedCallArg {
                        name: None,
                        value: item.clone(),
                    }],
                )?;
                Ok(Value::bool_(self.truthy_value(&result)?))
            }
            // Scalar non-iterables (int/float/bool/bigint/complex/None …) reach
            // here.  CPython raises `TypeError: argument of type '<type>' is not
            // iterable` with the operand's type name — matching the PyInstance
            // arm above (issue #2030); a bare `RuntimeError` escaped before.
            _ => Err(pyrust_core::type_err!(
                "argument of type '{}' is not iterable",
                value_type_name_str(&container)
            )),
        }
    }
}
