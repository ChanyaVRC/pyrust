// Hot-path "fast-path" specialization helpers, consolidated out of the big
// runtime files (`expr.rs` / `calls.rs` / `vm.rs`) into one discoverable module.
//
// Every item here is `include!`d into the same `impl Interpreter` / module scope
// from `runtime.rs`, so visibility, name resolution, and behaviour are identical
// to when these helpers lived inline next to their dispatch sites.  The split is
// purely organisational — this is a pure code move / light mechanical extraction
// (inline block → helper fn taking the needed params), not a logic rewrite.
//
// The dispatch sites stay in their original files (e.g. `set_binary_op` /
// `eval_index` / `eval_slice` in `expr.rs`, `call_user_function_expanded` in
// `calls.rs`) and call the moved helpers below; the helpers are marked
// `#[inline]` so the file boundary stays zero-cost (Rust inlines freely within
// a crate).
//
// What lives here:
//   - the set-op no-clone fast path (`set_direct_value` / `with_set_items` /
//     `set_algebra_fast`), #1978 / #2135.
//   - the built-in iterator step family (`step_*_iter` × 6), from `calls.rs`.
//   - the ASCII string index / slice + `step == 1` contiguous-copy subscript
//     fast paths (`fast_str_ascii_index` / `fast_slice_contiguous`), from
//     `eval_index` / `eval_slice`; #2032 / #2066 / #2111 / #2116 / #2136.
//   - the frame-binding fast path (`bind_param`), from
//     `call_user_function_expanded`; #2123 / #2137.
//   - the BinOp / comparison primitive fast paths (`int_int_fast` /
//     `float_float_fast` / `str_str_fast` / `classify_binop_tag` / `int_cmp` /
//     `str_cmp`), from the `vm.rs` `run_bytecode_inner` dispatch loop.
//
// What deliberately stayed inline at its dispatch site (extracting would change
// logic or hurt the hot path, not move it):
//   - the attr inline-cache HIT arms in the `vm.rs` GetAttr / SetAttr /
//     LoadGlobal dispatch loop, fused into the loop body with `continue` out of
//     the dispatch `match` (extracting would restructure control flow, a logic
//     change rather than a move).
//   - `pyrust-builtins/string.rs` internal ASCII helpers (a separate crate;
//     no cross-crate move).
//   - the stepped (`step != 1`) slice final-match arms in `eval_slice`, which
//     share the method's `indices` local with the list/tuple/bytes arms.

// ---------------------------------------------------------------------------
// Set-op no-clone fast path (issue #1978 / #2135), extracted from `expr.rs`.
//
// `set_binary_op` (the dispatch site, still in `expr.rs`) borrows both set
// operands in place and clones only the elements that land in the result,
// instead of cloning both whole operands up front.  These three helpers form
// that fast path.
// ---------------------------------------------------------------------------

/// Resolve a value to the direct `set` / `frozenset` `Value` backing it (peeling
/// a `PyInstance` subclass backing, possibly through several layers), together
/// with whether that backing is frozen.  The returned `Value` shares storage
/// with the original (an `Rc` bump, not an `IndexSet` clone), so callers can
/// borrow its `IndexSet` without copying the whole operand (issue #1978).
///
/// Returns `None` for anything that is not a `set` / `frozenset` (or subclass
/// thereof) — dict views, lists, etc. — leaving those to the materialising
/// `coerce_set_operand` path.
#[inline]
fn set_direct_value(v: &Value) -> Option<(Value, bool)> {
    if matches!(v.kind(), ValueKind::Set(_)) {
        return Some((v.clone(), false));
    }
    if pyrust_builtins::frozenset::as_items(v).is_some() {
        return Some((v.clone(), true));
    }
    if let Some(inst_rc) = v.as_py_instance_rc() {
        let backing = instance_builtin_data(inst_rc)?;
        return set_direct_value(&backing);
    }
    None
}

/// Scoped borrow access to the backing `IndexSet` of a direct `set` /
/// `frozenset` `Value` (as produced by [`set_direct_value`]).  Borrows in place;
/// never clones the `IndexSet` (issue #1978).
#[inline]
fn with_set_items<R>(v: &Value, f: impl FnOnce(&PySet) -> R) -> R {
    if let Some(rc) = pyrust_builtins::frozenset::as_items(v) {
        return f(&rc);
    }
    v.set_with(f).expect("set_direct_value guarantees a set/frozenset value")
}

/// Primitive-key set algebra over borrowed operands: clones only the elements
/// that land in the result and builds it with a capacity hint (issue #1978).
#[inline]
fn set_algebra_fast(
    a: &PySet,
    b: &PySet,
    op: SetOp,
) -> PySet {
    // `Or` clones the LHS backing table wholesale — a raw bucket copy that
    // preserves every element's already-computed hash, so a's elements are NOT
    // re-hashed — then adds only the RHS elements (CPython's `set_or`: copy LHS,
    // update with RHS).  Insertion order (a's elements, then b's extras) is
    // identical to the previous `a.iter().chain(b.iter())` build, but it skips
    // re-hashing all of a.
    if let SetOp::Or = op {
        let mut out = a.clone();
        out.reserve(b.len());
        for k in b.iter() {
            out.insert(k.clone());
        }
        return out;
    }
    let cap = match op {
        SetOp::And => a.len().min(b.len()),
        SetOp::Sub => a.len(),
        SetOp::Or => unreachable!(),
        SetOp::Xor => a.len() + b.len(),
    };
    let mut out = PySet::with_capacity_and_hasher(cap, Default::default());
    match op {
        SetOp::Or => {}
        SetOp::And => {
            for k in a.iter() {
                if b.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
        SetOp::Sub => {
            for k in a.iter() {
                if !b.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
        SetOp::Xor => {
            for k in a.iter() {
                if !b.contains(k) {
                    out.insert(k.clone());
                }
            }
            for k in b.iter() {
                if !a.contains(k) {
                    out.insert(k.clone());
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Built-in iterator step family (`step_*_iter`), extracted from `calls.rs`.
//
// One cohesive group of six lazy "advance one element" steps backing the
// built-in iterator types (`getitem` / `callable` / `map` / `filter` /
// `enumerate` / `zip`).  Each is called once per produced element from
// `call_next` and from the `ForIter` dispatch, so they are marked `#[inline]`
// to keep the file boundary zero-cost.  (Forcing `#[inline(always)]` here was
// measured to *regress* `sum(map(user_fn, ...))` by bloating the already-huge
// `call_next`; plain `#[inline]` is perf-neutral — see the PR perf table.)
// Pure relocation — behaviour identical to when they lived next to
// `make_getitem_iter` / `step_or_stop` in `calls.rs`.
// ---------------------------------------------------------------------------

impl Interpreter {
    /// One step of the lazy `__getitem__` iterator.
    /// `Ok(Some(v))` → next element; `Ok(None)` → exhausted (caller
    /// should yield `StopIteration` or return default); `Err(e)` →
    /// any non-terminator exception from `__getitem__` propagates.
    ///
    /// Called from `call_next`'s GetItemIter branch and from
    /// `ForIter`'s `UserDefined` arm via the same downcast.
    #[inline]
    pub(crate) fn step_getitem_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        let snapshot: Option<(Value, Value, i64)> = {
            let borrow = state_rc.borrow();
            if let Some(it) = borrow.downcast_ref::<GetItemIter>() {
                if it.exhausted {
                    return Ok(None);
                }
                Some((it.obj.clone(), it.method.clone(), it.index))
            } else {
                return Err(PyError::Runtime("step_getitem_iter on non-GetItemIter state".to_string()));
            }
        };
        let (obj, method, index) = snapshot.unwrap();
        let arg = ExpandedCallArg {
            name: None,
            value: Value::int(index),
        };
        let result = invoke_class_method(self, method, obj, &[arg]);
        match result {
            Ok(v) => {
                if let Some(it) = state_rc.borrow_mut().downcast_mut::<GetItemIter>() {
                    it.index = it.index.saturating_add(1);
                }
                Ok(Some(v))
            }
            Err(e) if is_sequence_iter_terminator(self, &e) => {
                if let Some(it) = state_rc.borrow_mut().downcast_mut::<GetItemIter>() {
                    it.exhausted = true;
                }
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// One step of the callable iterator created by `iter(callable, sentinel)`.
    /// Invokes `callable()` with no arguments, then checks whether the returned
    /// value equals `sentinel`.  Returns `Ok(Some(v))` when a value was
    /// produced, `Ok(None)` when the sentinel was matched (exhausted), or
    /// `Err(e)` when the callable raised.
    ///
    /// The borrow on `state_rc` is fully released before `call_function_expanded`
    /// is invoked, mirroring the `step_getitem_iter` approach.
    #[inline]
    pub(crate) fn step_callable_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        // Extract callable and sentinel while releasing the borrow, so that
        // call_function_expanded can re-enter the interpreter without aliasing.
        let snapshot: Option<(Value, Value)> = {
            let borrow = state_rc.borrow();
            if let Some(it) = borrow.downcast_ref::<CallableIter>() {
                if it.done {
                    return Ok(None);
                }
                Some((it.callable.clone(), it.sentinel.clone()))
            } else {
                return Err(PyError::Runtime(
                    "step_callable_iter on non-CallableIter state".to_string(),
                ));
            }
        };
        let (callable, sentinel) = snapshot.unwrap();
        let result = self.call_function_expanded(callable, &[]);
        match result {
            Ok(v) => {
                let equal = self.values_user_eq(&v, &sentinel)?;
                if equal {
                    if let Some(it) = state_rc.borrow_mut().downcast_mut::<CallableIter>() {
                        it.done = true;
                    }
                    Ok(None)
                } else {
                    Ok(Some(v))
                }
            }
            Err(e) => Err(e),
        }
    }

    /// One step of the lazy `map(func, *iterables)` iterator.
    ///
    /// Advances each stored source iterator by one element via `call_next`
    /// and invokes `func` with the resulting row.  Stops as soon as any
    /// source raises `StopIteration` (CPython map stops at the shortest).
    ///
    /// `Ok(Some(v))` → next mapped value; `Ok(None)` → exhausted;
    /// `Err(e)` → error from `func` or an iterator.
    #[inline]
    pub(crate) fn step_map_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        // Snapshot func + source iterators with a SINGLE borrow+downcast.  The
        // `func`/`sources` fields are immutable after construction (only `done`
        // is mutated), so the clones — cheap Rc bumps — stay valid across the
        // call_next / call_function_expanded callouts below.  The borrow is
        // dropped before any callout so a re-entrant `next()` on the same map
        // object can re-borrow the state.
        let (func, sources): (Value, IterSrcBuf) = {
            let borrow = state_rc.borrow();
            let s = borrow
                .downcast_ref::<MapIter>()
                .ok_or_else(|| PyError::Runtime("step_map_iter on non-MapIter state".to_string()))?;
            if s.done {
                return Ok(None);
            }
            (s.func.clone(), s.sources.clone())
        };
        // Advance each source iterator by one element into a stack-allocated arg
        // buffer (no heap Vec for the common 1-/2-source case).
        let mut args: ExpandedArgBuf = ExpandedArgBuf::with_capacity(sources.len());
        for iter_val in &sources {
            match self.call_next(iter_val, None) {
                Ok(v) => args.push(ExpandedCallArg { name: None, value: v }),
                Err(e) if e.class_name_is("StopIteration") => {
                    state_rc.borrow_mut().downcast_mut::<MapIter>().unwrap().done = true;
                    return Ok(None);
                }
                Err(e) => return Err(e),
            }
        }
        let result = self.call_function_expanded(func, &args)?;
        Ok(Some(result))
    }

    /// One step of the lazy `filter(func, iterable)` iterator.
    ///
    /// Advances the stored source iterator by one element via `call_next` and
    /// tests the result with `func` (or truthiness when `func` is `None`).
    /// Repeats until a passing item is found or the source is exhausted.
    ///
    /// `Ok(Some(v))` → next passing value; `Ok(None)` → exhausted;
    /// `Err(e)` → error from `func` or the iterator.
    #[inline]
    pub(crate) fn step_filter_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        // Snapshot func + source iterator ONCE with a single borrow+downcast.
        // Both fields are immutable after construction (only `done` is mutated),
        // so these clones — cheap Rc bumps — stay valid across the whole
        // scan loop and every callout, avoiding a re-borrow/re-downcast/re-clone
        // per candidate element.  The borrow is dropped before any callout so a
        // re-entrant `next()` on the same filter object can re-borrow the state.
        let (func_opt, iter_val): (Option<Value>, Value) = {
            let borrow = state_rc.borrow();
            let s = borrow.downcast_ref::<FilterIter>().ok_or_else(|| {
                PyError::Runtime("step_filter_iter on non-FilterIter state".to_string())
            })?;
            if s.done {
                return Ok(None);
            }
            (s.func.clone(), s.source.clone())
        };
        loop {
            // Advance the source by one element.
            let item = match self.call_next(&iter_val, None) {
                Ok(v) => v,
                Err(e) if e.class_name_is("StopIteration") => {
                    state_rc.borrow_mut().downcast_mut::<FilterIter>().unwrap().done = true;
                    return Ok(None);
                }
                Err(e) => return Err(e),
            };
            let keep = if let Some(func) = &func_opt {
                let test = self.call_function_expanded(
                    func.clone(),
                    &[ExpandedCallArg { name: None, value: item.clone() }],
                )?;
                self.truthy_value(&test)?
            } else {
                self.truthy_value(&item)?
            };
            if keep {
                return Ok(Some(item));
            }
            // else continue to next candidate
        }
    }

    /// One step of the lazy `itertools.chain.from_iterable(outer)` iterator
    /// (#2362).
    ///
    /// Advances the current inner iterator by one element; on inner exhaustion
    /// pulls the next inner iterable from `outer` (StopIteration ends the whole
    /// chain) and `iter()`s it lazily (a non-iterable element raises
    /// `TypeError` only when reached, matching CPython).  Both outer and inner
    /// are driven one element at a time so an inner generator's interleaved
    /// side effects run in step with the consumer (CPython laziness; an
    /// `islice` that stops mid-inner must not over-consume).
    ///
    /// Fast path: when the inner is a `NativeIterFrame` (the common
    /// `from_iterable(list_of_lists)` case — lists / tuples / ranges / dict
    /// views), drive it with a direct index-walk under a single borrow,
    /// skipping the `call_next` dispatch / re-borrow per element.
    ///
    /// `Ok(Some(v))` → next value; `Ok(None)` → exhausted; `Err(e)` → error
    /// from a source iterator or a non-iterable inner element.
    #[inline]
    pub(crate) fn step_chain_from_iterable(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        loop {
            // Snapshot the current inner (a cheap Rc bump) under a brief borrow,
            // then release it before any per-element work.
            let inner: Option<Value> = {
                let borrow = state_rc.borrow();
                let s = borrow.downcast_ref::<ChainFromIterableIter>().ok_or_else(|| {
                    PyError::Runtime(
                        "step_chain_from_iterable on non-ChainFromIterableIter state".to_string(),
                    )
                })?;
                if s.done {
                    return Ok(None);
                }
                s.inner.clone()
            };
            if let Some(inner) = inner {
                // Fast path: a `NativeIterFrame` inner (lists / tuples / ranges /
                // dict views — no Python-level side effects) is advanced with a
                // direct index-walk, skipping the `call_next` dispatch per
                // element.  The snapshot above already released the chain's own
                // state borrow, so borrowing the inner's cell here is safe.
                if let ValueKind::Generator(inner_rc) = inner.kind()
                    && let Ok(mut inner_borrow) = inner_rc.try_borrow_mut()
                    && let Some(native) = inner_borrow.downcast_mut::<NativeIterFrame>()
                {
                    native.guard_check()?;
                    if native.pos < native.items.len() {
                        let v = native.items[native.pos].clone();
                        native.pos += 1;
                        return Ok(Some(v));
                    }
                    // Native inner exhausted — drop it and loop to pull the next.
                    drop(inner_borrow);
                    state_rc
                        .borrow_mut()
                        .downcast_mut::<ChainFromIterableIter>()
                        .unwrap()
                        .inner = None;
                    continue;
                }
                // Generic path: drive a non-native inner (generator / PyInstance)
                // one element at a time.
                match self.call_next(&inner, None) {
                    Ok(v) => return Ok(Some(v)),
                    Err(e) if e.class_name_is("StopIteration") => {
                        state_rc
                            .borrow_mut()
                            .downcast_mut::<ChainFromIterableIter>()
                            .unwrap()
                            .inner = None;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            // No current inner: pull the next inner iterable from the outer
            // source (StopIteration ends the whole chain), then `iter()` it.
            let outer = {
                let borrow = state_rc.borrow();
                borrow
                    .downcast_ref::<ChainFromIterableIter>()
                    .unwrap()
                    .outer
                    .clone()
            };
            let next_iterable = match self.call_next(&outer, None) {
                Ok(v) => v,
                Err(e) if e.class_name_is("StopIteration") => {
                    state_rc
                        .borrow_mut()
                        .downcast_mut::<ChainFromIterableIter>()
                        .unwrap()
                        .done = true;
                    return Ok(None);
                }
                Err(e) => return Err(e),
            };
            let new_inner = crate::builtin_modules::builtins::make_iterator(self, &next_iterable)?;
            state_rc
                .borrow_mut()
                .downcast_mut::<ChainFromIterableIter>()
                .unwrap()
                .inner = Some(new_inner);
            // loop to drain the freshly-pulled inner
        }
    }

    /// One step of the lazy `enumerate(iterable, start=N)` iterator.
    ///
    /// Advances the stored source iterator by one element via `call_next` and
    /// returns `(counter, element)`.  Counter is incremented after each yield.
    ///
    /// `Ok(Some(v))` → next `(i, x)` tuple; `Ok(None)` → exhausted;
    /// `Err(e)` → error from the source iterator.
    #[inline]
    pub(crate) fn step_enumerate_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        let (iter_val, counter): (Value, Value) = {
            let borrow = state_rc.borrow();
            let s = borrow.downcast_ref::<EnumerateIter>().ok_or_else(|| {
                PyError::Runtime("step_enumerate_iter on non-EnumerateIter state".to_string())
            })?;
            if s.done {
                return Ok(None);
            }
            (s.source.clone(), s.counter.clone())
        };
        match self.call_next(&iter_val, None) {
            Ok(item) => {
                // Increment the counter, promoting to BigInt on i64 overflow
                // instead of wrapping to a negative index (#2125).  The common
                // inline-int case stays a single `checked_add`.
                let next = match counter.as_int() {
                    Some(n) => match n.checked_add(1) {
                        Some(m) => Value::int(m),
                        None => value_from_bigint(PyBigInt::from(n) + 1),
                    },
                    None => value_from_bigint(
                        value_to_bigint(&counter).expect("enumerate counter is always an int") + 1,
                    ),
                };
                let mut borrow = state_rc.borrow_mut();
                let s = borrow.downcast_mut::<EnumerateIter>().unwrap();
                s.counter = next;
                Ok(Some(Value::tuple(vec![counter, item])))
            }
            Err(e) if e.class_name_is("StopIteration") => {
                state_rc
                    .borrow_mut()
                    .downcast_mut::<EnumerateIter>()
                    .unwrap()
                    .done = true;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// One step of the lazy arbitrary-precision range iterator (#2118).
    /// Yields `cur` then advances by `step`; `Ok(None)` once `stop` is reached.
    pub(crate) fn step_bigrange_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        let mut borrow = state_rc.borrow_mut();
        let s = borrow.downcast_mut::<BigRangeIter>().ok_or_else(|| {
            PyError::Runtime("step_bigrange_iter on non-BigRangeIter state".to_string())
        })?;
        let exhausted = if s.step.sign() == PyBigIntSign::Plus {
            s.cur >= s.stop
        } else {
            s.cur <= s.stop
        };
        if exhausted {
            return Ok(None);
        }
        let v = value_from_bigint(s.cur.clone());
        s.cur += &s.step;
        Ok(Some(v))
    }

    /// One step of the lazy `zip(it1, it2, ..., strict=False)` iterator.
    ///
    /// Advances all stored source iterators by one element each via `call_next`
    /// and returns a tuple of the results.  Stops at the first `StopIteration`
    /// from any source.  When `strict=True`, checks that all other sources also
    /// raise `StopIteration` at the same position and raises `ValueError` if any
    /// mismatch is detected.
    ///
    /// `Ok(Some(v))` → next row tuple; `Ok(None)` → exhausted;
    /// `Err(e)` → error from a source or strict-mode mismatch.
    #[inline]
    pub(crate) fn step_zip_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        // Read only scalar metadata — do NOT clone the sources Vec.
        let (n_sources, strict, count) = {
            let borrow = state_rc.borrow();
            let s = borrow.downcast_ref::<ZipIter>().ok_or_else(|| {
                PyError::Runtime("step_zip_iter on non-ZipIter state".to_string())
            })?;
            if s.done {
                return Ok(None);
            }
            (s.sources.len(), s.strict, s.count)
        };
        if n_sources == 0 {
            state_rc.borrow_mut().downcast_mut::<ZipIter>().unwrap().done = true;
            return Ok(None);
        }
        // Advance each iterator one step.  Clone one Value at a time (cheap Rc
        // bump) so we never allocate a temporary Vec<Value> for the whole row.
        let mut row: Vec<Value> = Vec::with_capacity(n_sources);
        let mut stopped_at: Option<usize> = None;
        for i in 0..n_sources {
            let iter_val = {
                let borrow = state_rc.borrow();
                borrow.downcast_ref::<ZipIter>().unwrap().sources[i].clone()
            };
            match self.call_next(&iter_val, None) {
                Ok(v) => row.push(v),
                Err(e) if e.class_name_is("StopIteration") => {
                    stopped_at = Some(i);
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        if let Some(short_idx) = stopped_at {
            state_rc.borrow_mut().downcast_mut::<ZipIter>().unwrap().done = true;
            if strict && n_sources > 1 {
                let check_start = if short_idx == 0 { 1 } else { 0 };
                for j in check_start..n_sources {
                    if j == short_idx {
                        continue;
                    }
                    let iter_val = {
                        let borrow = state_rc.borrow();
                        borrow.downcast_ref::<ZipIter>().unwrap().sources[j].clone()
                    };
                    match self.call_next(&iter_val, None) {
                        Ok(_) => {
                            if short_idx == 0 {
                                return Err(pyrust_core::value_err!(zip_longer_message(j, count)));
                            } else {
                                return Err(pyrust_core::value_err!(zip_shorter_message(short_idx, count)));
                            }
                        }
                        Err(e) if e.class_name_is("StopIteration") => {}
                        Err(e) => return Err(e),
                    }
                }
                if short_idx > 0 {
                    return Err(pyrust_core::value_err!(zip_shorter_message(short_idx, count)));
                }
            }
            return Ok(None);
        }
        state_rc.borrow_mut().downcast_mut::<ZipIter>().unwrap().count += 1;
        Ok(Some(Value::tuple(row)))
    }
}

// ---------------------------------------------------------------------------
// ASCII string index / slice + step==1 contiguous-copy fast paths, extracted
// from `expr.rs` (`eval_index` / `eval_slice`).
//
// These are the bodies of the subscript fast paths (#2032 / #2066 / #2111 /
// #2116 / #2136).  The dispatch sites in `expr.rs` keep the `if <applies>`
// guard and call these helpers; pure relocation, behaviour identical.
// ---------------------------------------------------------------------------

/// O(1) ASCII string index: when the backing `str` is all-ASCII, char index ==
/// byte index, so the i-th char is a single byte — no O(idx) char scan (#2032 /
/// #2116).  Caller has already confirmed `target.str_is_ascii()`.
#[inline]
fn fast_str_ascii_index(text: &str, index: &Value) -> Result<Value> {
    let idx = normalize_index(index, text.len(), "string")?;
    let b = text.as_bytes()[idx];
    Ok(Value::string((b as char).encode_utf8(&mut [0u8; 4]) as &str))
}

/// `step == 1` contiguous slice: produces the contiguous run `[start, end)`.
/// `resolve_slice_bounds` clamps both bounds to `[0, len]` for positive step, so
/// `start..end.max(start)` is always in range; copy the run directly (memcpy for
/// bytes, range clone for list/tuple, zero-copy shared-buffer slice for ASCII
/// str) instead of building a full index `Vec` and copying element-by-element
/// (#2066 / #2111).  `str_is_ascii` is the cached ASCII flag computed by the
/// caller (#2032 / #2116 / #2136).
#[inline]
fn fast_slice_contiguous(target: &Value, start: i64, end: i64, str_is_ascii: bool) -> Result<Value> {
    let s = start as usize;
    let e = (end.max(start)) as usize;
    match target.kind() {
        ValueKind::List(items) => Ok(Value::list(items[s..e].to_vec())),
        ValueKind::Tuple(items) => Ok(Value::tuple(items[s..e].to_vec())),
        ValueKind::Bytes(rc) => Ok(Value::bytes(rc[s..e].to_vec())),
        ValueKind::Str(string) => {
            if s >= e {
                return Ok(Value::string(String::new()));
            }
            // ASCII fast path (#2032): char index == byte index, so the
            // slice bounds are already byte offsets — O(1) zero-copy
            // shared-buffer slice, no char_indices scan.
            if str_is_ascii {
                return Ok(target.string_slice(s, e));
            }
            // s/e are char indices; walk char_indices once to find the
            // corresponding byte offsets, then slice the &str (no
            // Vec<char> allocation, char-boundary correct for multibyte).
            let mut byte_start = string.len();
            let mut byte_end = string.len();
            for (ci, (bi, _)) in string.char_indices().enumerate() {
                if ci == s {
                    byte_start = bi;
                }
                if ci == e {
                    byte_end = bi;
                    break;
                }
            }
            Ok(Value::string(&string[byte_start..byte_end]))
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// Frame-binding fast path (`bind_param`), extracted from `calls.rs`
// (`call_user_function_expanded`'s no-variadic fast path, #2123).
//
// Routes one bound argument value to its compile-time destination — a frame
// register (the common case) or, for cell-var params under a local env, an env
// entry by name — and marks the param bound.  Previously a local `macro_rules!`
// closing over the frame-local state; converted to an `#[inline]` helper taking
// that state by reference so the family lives here.  Pure relocation: the body
// is byte-for-byte the old macro expansion; the call sites just add `?`.
// ---------------------------------------------------------------------------

#[inline(always)]
fn bind_param(
    bound: &mut [bool],
    function: &Rc<UserFunction>,
    num_regs: usize,
    regs: &mut RegsBuf,
    local_env: &Option<EnvRef>,
    pi: usize,
    val: Value,
) -> Result<()> {
    bound[pi] = true;
    bind_param_direct(function, num_regs, regs, local_env, pi, val)
}

/// Route a bound value to parameter `pi`'s compile-time destination without
/// touching a per-param `bound` flag array.  Used by the exact-arity
/// positional fast bind in `call_user_function_expanded`, where every
/// parameter is filled exactly once in order so the bound-flag bookkeeping is
/// unnecessary.  `bind_param` delegates here after marking the flag.
#[inline]
pub(crate) fn bind_param_direct(
    function: &Rc<UserFunction>,
    num_regs: usize,
    regs: &mut RegsBuf,
    local_env: &Option<EnvRef>,
    pi: usize,
    val: Value,
) -> Result<()> {
    match function.param_binds[pi] {
        pyrust_core::ParamBind::Reg(reg) => {
            if reg as usize >= num_regs {
                return Err(pyrust_core::py_err!("SystemError", "parameter '{}' register index {} out of range (num_regs={})",
                        function.params[pi].name, reg, num_regs));
            }
            regs[reg as usize] = val;
        }
        pyrust_core::ParamBind::Cell => {
            // Only reachable under `needs_local_env`.
            if let Some(env) = local_env {
                env.borrow_mut()
                    .values
                    .insert(&function.params[pi].name, val);
            }
        }
        pyrust_core::ParamBind::None => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BinOp / comparison primitive fast paths, extracted from `vm.rs`'s
// `run_bytecode_inner` dispatch loop.
//
// These six free functions are the type-specialized inner fast branches the
// hot `Insn::BinOp` / `BinOpImm` / `JumpCmp*` arms consult before falling
// through to the general `eval_binary` slow path:
//   - `int_int_fast`     — int×int arithmetic / bitwise / shift / comparison.
//   - `float_float_fast` — float×float arithmetic / comparison.
//   - `str_str_fast`     — str×str concat / comparison.
//   - `classify_binop_tag` — the inline-cache type-tag classifier.
//   - `int_cmp` / `str_cmp` — comparison-only fast arms for `JumpCmp*`.
//
// They are `#[inline(always)]` so the file boundary is zero-cost: codegen at
// every dispatch site is byte-identical to when these lived inline in `vm.rs`
// (Rust inlines freely within a crate; verified perf-neutral, see the PR
// bench table).  Pure relocation — no logic change.
//
// CLAUDE.md flags int-int Move/BinOp as extremely perf-sensitive (PR #478: an
// Int-specialized "fast path" was a 15% loss).  This is NOT that: the move
// keeps `#[inline(always)]` and changes no logic, so the optimizer produces
// identical machine code; the bench table confirms zero regression.
//
// The larger inline-cache machinery (the `BinOp` adaptive cache and the
// `GetAttr` / `SetAttr` inline caches) is also extracted here, as
// `#[inline(always)]` methods on `impl Interpreter` (see further below):
//   - `exec_binop`     — full `Insn::BinOp` body: int fast path + adaptive
//     float/str inline cache (Counting → Specialized → Megamorphic).
//   - `exec_get_attr`  — full `Insn::GetAttr` body: InstanceAttr / ClassAttr
//     cache hit + slow-path fill / invalidation.
//   - `exec_set_attr`  — full `Insn::SetAttr` body: SetInstanceAttr write cache
//     hit + slow-path fill / invalidation.
// Each takes the loop state it touches (`regs`, `code`, `pc`, the instruction
// operands, `num_locals`) and returns `Result<()>`; the dispatch arm just calls
// `vm_try!(self.exec_*(...))`, routing any error through the handler stack
// exactly as the inline `vm_try!` did.  The bodies are byte-for-byte the old
// inline blocks (the `pool_get!` / `vm_try!` macros become plain `?`), so
// codegen and behaviour are identical; `#[inline(always)]` keeps the file
// boundary zero-cost.  The bench table confirms perf neutrality.
//
// `LoadGlobal`'s inline-cache hit is a two-line read (`entry.0 == cur_ver` →
// clone) that shares `cur_ver` with its own slow path; it stays inline (a
// helper would be larger than the code it replaces and gains nothing).
// ---------------------------------------------------------------------------

/// Which primitive built-in (if any) `class` inherits its storage layout from.
/// The three categories are mutually exclusive (a class cannot inherit from two
/// incompatible primitive layouts), so a *single* base-chain walk classifies
/// all of them.  `instantiate_normal_instance` previously walked the chain three
/// times (`find_mutable_primitive_base` + `find_immutable_primitive_base` +
/// `find_scalar_primitive_base`) on every instantiation; folding them into one
/// walk removes two redundant chain traversals from the hot construction path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PrimitiveBase {
    /// No primitive base anywhere in the chain (the overwhelmingly common case
    /// for ordinary user classes).
    None,
    /// Mutable container primitive (`dict` / `list` / `set`): needs an empty
    /// backing store pre-initialised before `__init__` runs.
    Mutable(&'static str),
    /// Immutable container primitive (`frozenset` / `tuple`): backing built from
    /// the constructor args at construction time.
    Immutable(&'static str),
    /// Scalar primitive (`str` / `int` / `float` / `bytes` / `bytearray` /
    /// `complex`): backing built from the constructor args at construction time.
    Scalar(&'static str),
}

/// Walk the (single-inheritance) base chain of `class` once and classify which
/// primitive built-in — if any — it derives its storage layout from.  Mirrors
/// the per-category `find_*_primitive_base` helpers exactly (same primitive
/// names, same `is_primitive_class` gate), but in a single traversal.
#[inline]
fn classify_primitive_base(class: &Rc<RefCell<PyClass>>) -> PrimitiveBase {
    let mut cur = Rc::clone(class);
    loop {
        if is_primitive_class(&cur) {
            match cur.borrow().name.as_str() {
                "dict" => return PrimitiveBase::Mutable("dict"),
                "list" => return PrimitiveBase::Mutable("list"),
                "set" => return PrimitiveBase::Mutable("set"),
                "frozenset" => return PrimitiveBase::Immutable("frozenset"),
                "tuple" => return PrimitiveBase::Immutable("tuple"),
                "str" => return PrimitiveBase::Scalar("str"),
                "int" => return PrimitiveBase::Scalar("int"),
                "float" => return PrimitiveBase::Scalar("float"),
                "bytes" => return PrimitiveBase::Scalar("bytes"),
                "bytearray" => return PrimitiveBase::Scalar("bytearray"),
                "complex" => return PrimitiveBase::Scalar("complex"),
                _ => {}
            }
        }
        let next = cur.borrow().base.clone();
        match next {
            Some(b) => cur = b,
            None => return PrimitiveBase::None,
        }
    }
}

/// Resolved per-class construction facts that `instantiate_normal_instance`
/// re-derives on *every* `Cls(...)` call: the MRO-resolved `__new__` and
/// `__init__` values, plus which primitive built-in (if any) supplies the
/// storage layout.  All three are read by walking the same base chain, so a
/// single traversal yields them together.
struct ConstructionPlan {
    /// The `__new__` resolved via the MRO (identical to
    /// `lookup_class_attr(class, "__new__")`), or `None` if none found.
    new_val: Option<Value>,
    /// The `__init__` resolved via the MRO (identical to
    /// `lookup_class_attr(class, "__init__")`), or `None`.
    init_val: Option<Value>,
    /// Primitive storage-layout base classification.
    prim: PrimitiveBase,
}

/// Resolve `__new__`, `__init__`, and the primitive-base classification for
/// `class` in a *single* linear base-chain walk.  Returns `None` (so the caller
/// falls back to the byte-identical per-attr `lookup_class_attr` path) whenever
/// any node in the chain participates in *multiple* inheritance — attribute
/// resolution there must follow the C3 MRO, which a plain depth-first chain walk
/// does not reproduce.  For the single-inheritance case (the common path, and
/// the one whose per-construction cost scales with MRO depth) this folds the
/// three separate `lookup_class_attr` / `classify_primitive_base` traversals
/// into one.
///
/// The walk mirrors `lookup_class_attr` exactly: a name resolves to the first
/// class in the chain whose *own* `attrs` defines it, and a chain that
/// terminates without an explicit base falls through to the `object` singleton
/// (the same `!has_explicit_base && !is_primitive_class` fallback).
#[inline]
fn resolve_construction_plan(class: &Rc<RefCell<PyClass>>) -> Option<ConstructionPlan> {
    let mut new_val: Option<Value> = None;
    let mut init_val: Option<Value> = None;
    let mut prim = PrimitiveBase::None;
    let mut cur = Rc::clone(class);
    loop {
        let borrowed = cur.borrow();
        // Multiple inheritance anywhere in the chain → bail to the C3 slow path.
        if !borrowed.extra_bases.is_empty() {
            return None;
        }
        if new_val.is_none() {
            new_val = borrowed.attrs.get("__new__").cloned();
        }
        if init_val.is_none() {
            init_val = borrowed.attrs.get("__init__").cloned();
        }
        let is_prim = is_primitive_class(&cur);
        if prim == PrimitiveBase::None && is_prim {
            prim = match borrowed.name.as_str() {
                "dict" => PrimitiveBase::Mutable("dict"),
                "list" => PrimitiveBase::Mutable("list"),
                "set" => PrimitiveBase::Mutable("set"),
                "frozenset" => PrimitiveBase::Immutable("frozenset"),
                "tuple" => PrimitiveBase::Immutable("tuple"),
                "str" => PrimitiveBase::Scalar("str"),
                "int" => PrimitiveBase::Scalar("int"),
                "float" => PrimitiveBase::Scalar("float"),
                "bytes" => PrimitiveBase::Scalar("bytes"),
                "bytearray" => PrimitiveBase::Scalar("bytearray"),
                "complex" => PrimitiveBase::Scalar("complex"),
                _ => PrimitiveBase::None,
            };
        }
        let has_explicit_base = borrowed.base.is_some();
        let next = borrowed.base.clone();
        drop(borrowed);
        match next {
            Some(b) => cur = b,
            None => {
                // Chain terminated.  Mirror `lookup_class_attr`'s implicit
                // `object` fallback: a class with no explicit base (and that is
                // not itself a primitive singleton) inherits `object`'s attrs.
                if !has_explicit_base && !is_prim {
                    let obj = object_class_singleton();
                    if !Rc::ptr_eq(&cur, &obj) {
                        let ob = obj.borrow();
                        if new_val.is_none() {
                            new_val = ob.attrs.get("__new__").cloned();
                        }
                        if init_val.is_none() {
                            init_val = ob.attrs.get("__init__").cloned();
                        }
                    }
                }
                break;
            }
        }
    }
    Some(ConstructionPlan {
        new_val,
        init_val,
        prim,
    })
}

// Tags used to round-trip `PrimitiveBase` through the core's
// `CachedConstructionPlan` (which may only reference core-visible types).  The
// `&'static str` payload is stored alongside in `prim_name`; `PrimitiveBase::None`
// carries no name (`""`).
const PRIM_TAG_NONE: u8 = 0;
const PRIM_TAG_MUTABLE: u8 = 1;
const PRIM_TAG_IMMUTABLE: u8 = 2;
const PRIM_TAG_SCALAR: u8 = 3;

impl PrimitiveBase {
    #[inline]
    fn to_cache_tag(self) -> (u8, &'static str) {
        match self {
            PrimitiveBase::None => (PRIM_TAG_NONE, ""),
            PrimitiveBase::Mutable(n) => (PRIM_TAG_MUTABLE, n),
            PrimitiveBase::Immutable(n) => (PRIM_TAG_IMMUTABLE, n),
            PrimitiveBase::Scalar(n) => (PRIM_TAG_SCALAR, n),
        }
    }

    #[inline]
    fn from_cache_tag(tag: u8, name: &'static str) -> PrimitiveBase {
        match tag {
            PRIM_TAG_MUTABLE => PrimitiveBase::Mutable(name),
            PRIM_TAG_IMMUTABLE => PrimitiveBase::Immutable(name),
            PRIM_TAG_SCALAR => PrimitiveBase::Scalar(name),
            _ => PrimitiveBase::None,
        }
    }
}

/// Resolve the construction plan for `class`, reusing the per-class cache when it
/// is still valid (issue #2330).  The cache is validated exactly like the
/// attribute inline caches: a hit requires both the class's own
/// `mutation_version` and the global `class_epoch()` to match the values stamped
/// when the plan was last resolved.  Either changing (a direct monkeypatch of
/// this class, or *any* class mutation that bumps the global epoch — e.g. a base
/// class being patched) forces a fresh `resolve_construction_plan` walk.
///
/// Multiply-inherited classes (`resolve_construction_plan` → `None`) are never
/// cached; the caller keeps the byte-identical per-attr C3 fallback for them.
#[inline]
fn resolve_construction_plan_cached(class: &Rc<RefCell<PyClass>>) -> Option<ConstructionPlan> {
    let epoch = pyrust_core::class_epoch();
    let class_version = class.borrow().mutation_version.get();
    // Fast path: a still-valid cached plan reproduces the resolved values with
    // no base-chain walk (cheap `Value` clones + a Copy `PrimitiveBase`).
    if let Some(cached) = class.borrow().construction_cache.borrow().as_deref()
        && cached.class_version == class_version
        && cached.epoch == epoch
    {
        return Some(ConstructionPlan {
            new_val: cached.new_val.clone(),
            init_val: cached.init_val.clone(),
            prim: PrimitiveBase::from_cache_tag(cached.prim_tag, cached.prim_name),
        });
    }
    // Miss (or stale): re-resolve and refresh the cache.  Only single-inheritance
    // classes (the ones `resolve_construction_plan` resolves) are cacheable.
    let plan = resolve_construction_plan(class)?;
    let (prim_tag, prim_name) = plan.prim.to_cache_tag();
    *class.borrow().construction_cache.borrow_mut() =
        Some(Box::new(pyrust_core::CachedConstructionPlan {
            new_val: plan.new_val.clone(),
            init_val: plan.init_val.clone(),
            prim_tag,
            prim_name,
            class_version,
            epoch,
        }));
    Some(plan)
}

#[inline(always)]
fn int_int_fast(a: i64, b: i64, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add    => a.checked_add(b).map(Value::int),
        BinaryOp::Sub    => a.checked_sub(b).map(Value::int),
        BinaryOp::Mul    => a.checked_mul(b).map(Value::int),
        BinaryOp::BitAnd => Some(Value::int(a & b)),
        BinaryOp::BitOr  => Some(Value::int(a | b)),
        BinaryOp::BitXor => Some(Value::int(a ^ b)),
        BinaryOp::LShift => {
            if b < 0 {
                // Negative shift → ValueError; fall through to eval_binary.
                None
            } else if b >= 64 {
                // Shift count ≥ 64: result is BigInt (or 0 for a==0).
                // Fall through to eval_binary which handles BigInt promotion.
                None
            } else {
                let n = b as u32;
                // Shift left then shift right; if we get back the original
                // value no significant bits were lost and the result fits i64.
                let r = a.wrapping_shl(n);
                if r.wrapping_shr(n) == a {
                    Some(Value::int(r))
                } else {
                    // Overflow: fall through for BigInt promotion.
                    None
                }
            }
        }
        BinaryOp::RShift => {
            if b < 0 {
                // Negative shift → ValueError; fall through to eval_binary.
                None
            } else if b >= 64 {
                // Saturate to sign bit (0 for non-negative, -1 for negative).
                // This is safe to handle here without BigInt.
                Some(Value::int(if a < 0 { -1 } else { 0 }))
            } else {
                Some(Value::int(a >> b))
            }
        }
        BinaryOp::Eq  => Some(Value::bool_(a == b)),
        BinaryOp::Ne  => Some(Value::bool_(a != b)),
        BinaryOp::Lt  => Some(Value::bool_(a < b)),
        BinaryOp::Le  => Some(Value::bool_(a <= b)),
        BinaryOp::Gt  => Some(Value::bool_(a > b)),
        BinaryOp::Ge  => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// Float-float fast path for arithmetic and comparison BinOps.
///
/// Returns `None` for:
/// - Ops that don't apply to floats (e.g. `BitAnd`).
/// - Cases where the Rust float result would diverge from CPython's
///   exception-raising behaviour: `Div`/`FloorDiv`/`Mod` by zero, and
///   `0.0 ** negative` for `Pow`.  The caller falls through to
///   `eval_binary` which raises the correct `ZeroDivisionError`.
///
/// NaN comparisons are handled correctly: Rust float comparisons with NaN
/// always return `false`, matching CPython's `float('nan') < x == False`.
#[inline(always)]
fn float_float_fast(a: f64, b: f64, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add => Some(Value::float(a + b)),
        BinaryOp::Sub => Some(Value::float(a - b)),
        BinaryOp::Mul => Some(Value::float(a * b)),
        BinaryOp::Div => {
            if b == 0.0 {
                // ZeroDivisionError: "float division by zero" — fall through.
                None
            } else {
                Some(Value::float(a / b))
            }
        }
        BinaryOp::FloorDiv => {
            if b == 0.0 {
                // ZeroDivisionError — fall through to eval_binary.
                None
            } else {
                // CPython's fmod-based float_divmod: handles inf/nan/signed-zero
                // and keeps `//` consistent with `divmod`/`%` (#2025).
                let (div, _) = float_divmod(a, b);
                Some(Value::float(div))
            }
        }
        BinaryOp::Mod => {
            if b == 0.0 {
                None
            } else {
                let mut r = a % b;
                // Match CPython float_rem: zero result copies sign of divisor;
                // non-zero result is adjusted so sign matches divisor.
                if r == 0.0 {
                    r = r.copysign(b);
                } else if r.signum() != b.signum() {
                    r += b;
                }
                Some(Value::float(r))
            }
        }
        BinaryOp::Pow => {
            // 0.0 ** negative → ZeroDivisionError in CPython; Rust returns ±inf.
            if a == 0.0 && b < 0.0 {
                None
            } else {
                Some(Value::float(a.powf(b)))
            }
        }
        BinaryOp::Eq => Some(Value::bool_(a == b)),
        BinaryOp::Ne => Some(Value::bool_(a != b)),
        BinaryOp::Lt => Some(Value::bool_(a < b)),
        BinaryOp::Le => Some(Value::bool_(a <= b)),
        BinaryOp::Gt => Some(Value::bool_(a > b)),
        BinaryOp::Ge => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// String fast path for BinOps that apply to `str`.
///
/// Currently handles `Add` (concatenation) and comparison operators.
/// Returns `None` for any op that doesn't apply to strings.
#[inline(always)]
fn str_str_fast(a: &str, b: &str, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add => {
            let mut s = String::with_capacity(a.len() + b.len());
            s.push_str(a);
            s.push_str(b);
            Some(Value::string(s))
        }
        BinaryOp::Eq => Some(Value::bool_(a == b)),
        BinaryOp::Ne => Some(Value::bool_(a != b)),
        BinaryOp::Lt => Some(Value::bool_(a < b)),
        BinaryOp::Le => Some(Value::bool_(a <= b)),
        BinaryOp::Gt => Some(Value::bool_(a > b)),
        BinaryOp::Ge => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// Classify two `Value` operands into a `BinopTypeTag` for the inline cache.
///
/// `Int` is returned only when both values tag as `TAG_INT` (or `BigInt`
/// that fits in i64 via `as_int()`).  `Float` requires both to be true floats
/// (`is_float()`).  `Str` requires both to be strings.  Everything else maps
/// to `Other`.  The tags are mutually exclusive because `as_int()` returns
/// `None` for floats and strings.
#[inline(always)]
fn classify_binop_tag(a: &Value, b: &Value) -> crate::bytecode::BinopTypeTag {
    use crate::bytecode::BinopTypeTag;
    // is_float() is a fast bit-mask check; check it first so Float beats Int
    // for Bool operands (Bool's tag is TAG_BOOL, not TAG_INT, so as_int won't
    // fire for bools, and this branch won't trip for them either).
    if a.is_float() && b.is_float() {
        return BinopTypeTag::Float;
    }
    if a.as_int().is_some() && b.as_int().is_some() {
        return BinopTypeTag::Int;
    }
    if a.is_str() && b.is_str() {
        return BinopTypeTag::Str;
    }
    BinopTypeTag::Other
}

#[inline(always)]
fn int_cmp(a: i64, b: i64, op: BinaryOp) -> Option<bool> {
    match op {
        BinaryOp::Eq => Some(a == b),
        BinaryOp::Ne => Some(a != b),
        BinaryOp::Lt => Some(a < b),
        BinaryOp::Le => Some(a <= b),
        BinaryOp::Gt => Some(a > b),
        BinaryOp::Ge => Some(a >= b),
        _ => None,
    }
}

#[inline(always)]
fn str_cmp(a: &str, b: &str, op: BinaryOp) -> Option<bool> {
    match op {
        BinaryOp::Eq => Some(a == b),
        BinaryOp::Ne => Some(a != b),
        BinaryOp::Lt => Some(a < b),
        BinaryOp::Le => Some(a <= b),
        BinaryOp::Gt => Some(a > b),
        BinaryOp::Ge => Some(a >= b),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// BinOp / GetAttr / SetAttr inline-cache machinery, extracted from the `vm.rs`
// `run_bytecode_inner_impl` dispatch loop.
//
// These three `#[inline(always)]` methods carry the FULL body of the
// `Insn::BinOp` / `Insn::GetAttr` / `Insn::SetAttr` dispatch arms — both the
// cache-hit fast path and the slow-path cache fill / invalidation.  The dispatch
// arm in `vm.rs` becomes a single `vm_try!(self.exec_*(...))` call, so any error
// the slow path returns is routed through the active exception handler stack
// exactly as the old inline `vm_try!(...)` did.
//
// The bodies are a mechanical relocation of the old inline blocks:
//   - `vm_try!(expr)` → `expr?` (error propagation is now via `?`, and the
//     caller's `vm_try!` does the handler-stack routing);
//   - `pool_get!(code.names, idx, "name")` → an inline `code.names.get(..)`
//     with the identical out-of-range `PyError::Runtime` message.
// No cache-fill / invalidation / megamorphic logic changed — the inline caches
// are correctness-critical (#1912 / #1998 / #2102 / #2108) and must stay
// byte-identical.  `#[inline(always)]` keeps codegen at the dispatch site
// identical to when these lived inline; the PR bench table confirms zero
// regression on the hot int-arith / float / str / attr-read / attr-write paths.
// ---------------------------------------------------------------------------

impl Interpreter {
    /// Full `Insn::BinOp` body: the unconditional int–int fast path followed by
    /// the adaptive float/str inline cache (Empty → Counting → Specialized, with
    /// Megamorphic deopt on a type mismatch).  See the original dispatch-arm
    /// comments inlined below for the per-state rationale.
    #[inline(always)]
    // Hot-path VM instruction handler: the arg list is the decoded `BinOp`
    // operands (dst/lhs/op/rhs + regs/code/pc/num_locals). Bundling into a struct
    // would add an indirection on the dispatch loop's hottest path — do not refactor.
    #[allow(clippy::too_many_arguments)]
    fn exec_binop(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        dst: crate::bytecode::Reg,
        lhs: crate::bytecode::Reg,
        op: BinaryOp,
        rhs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        use crate::bytecode::{BinOpCacheEntry, BinopTypeTag, BINOP_SPEC_THRESHOLD};
        // Hot path: `as_int()` is a tagged-u64 check that bypasses `kind()`'s
        // scoped RefCell borrow for the List/Dict/Set kinds (#450).  Unlike
        // `Insn::Move` (where #441 showed the int specialization is a wash), the
        // BinOp fast path also short-circuits the entire `eval_binary` dispatch
        // for int–int ops, so the savings are real.  This check runs
        // unconditionally (no cache overhead) so int-int loops pay nothing.
        if let (Some(a), Some(b)) =
            (regs[lhs as usize].as_int(), regs[rhs as usize].as_int())
            && let Some(result) = int_int_fast(a, b, op)
        {
            regs[dst as usize] = result;
            return Ok(());
        }
        // Adaptive inline cache for float / str specialisation.  Consulted after
        // the int fast path misses, so int-int code never pays cache-lookup
        // overhead.
        //
        // Deopt policy: only deopt (→ Megamorphic) on a *type* mismatch.
        // Edge-case values (e.g. div-by-zero in the Float path) fall through to
        // eval_binary while keeping the cache Specialized so subsequent calls
        // still hit the fast path.
        let cache_slot = pc - 1;
        let cache_entry = code.binop_cache.borrow()[cache_slot];
        match cache_entry {
            BinOpCacheEntry::Megamorphic => {
                // Permanently polymorphic site: skip classification, go straight
                // to eval_binary.
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Specialized(BinopTypeTag::Float) => {
                let lv = &regs[lhs as usize];
                let rv = &regs[rhs as usize];
                if lv.is_float() && rv.is_float() {
                    let a = lv.as_float_raw();
                    let b = rv.as_float_raw();
                    if let Some(result) = float_float_fast(a, b, op) {
                        regs[dst as usize] = result;
                        return Ok(());
                    }
                    // Fast path returned None (e.g. div-by-zero, unsupported
                    // op): site is still Float/Float, so keep the Specialized
                    // state and fall through to eval_binary for this edge case.
                } else {
                    // Actual type mismatch: deopt to Megamorphic.
                    code.binop_cache.borrow_mut()[cache_slot] = BinOpCacheEntry::Megamorphic;
                }
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Specialized(BinopTypeTag::Str) => {
                let lv = &regs[lhs as usize];
                let rv = &regs[rhs as usize];
                if lv.is_str() && rv.is_str() {
                    let a = lv.as_str().unwrap();
                    let b = rv.as_str().unwrap();
                    if let Some(result) = str_str_fast(a, b, op) {
                        regs[dst as usize] = result;
                        return Ok(());
                    }
                    // Unsupported op for str (e.g. str * str): keep Specialized,
                    // fall through to eval_binary which will raise the proper
                    // TypeError.
                } else {
                    // Type mismatch: deopt.
                    code.binop_cache.borrow_mut()[cache_slot] = BinOpCacheEntry::Megamorphic;
                }
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Specialized(BinopTypeTag::Int) => {
                // int_int_fast already ran above and returned None (overflow or
                // unsupported op such as FloorDiv/Mod/Div/Pow).  The types are
                // still correct; keep Specialized so the fast path is retried
                // next loop iteration.  Fall through to eval_binary for this
                // invocation.
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Specialized(BinopTypeTag::Other) => {
                // No fast path for 'Other' types.  If the types are still
                // 'Other', stay Specialized (no deopt needed since there's no
                // fast path to lose).  Fall through.
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Counting { tag, count } => {
                let observed = classify_binop_tag(&regs[lhs as usize], &regs[rhs as usize]);
                let new_entry = if observed == tag {
                    let new_count = count + 1;
                    if new_count >= BINOP_SPEC_THRESHOLD {
                        BinOpCacheEntry::Specialized(tag)
                    } else {
                        BinOpCacheEntry::Counting {
                            tag,
                            count: new_count,
                        }
                    }
                } else {
                    BinOpCacheEntry::Megamorphic
                };
                code.binop_cache.borrow_mut()[cache_slot] = new_entry;
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
            BinOpCacheEntry::Empty => {
                let observed = classify_binop_tag(&regs[lhs as usize], &regs[rhs as usize]);
                code.binop_cache.borrow_mut()[cache_slot] = BinOpCacheEntry::Counting {
                    tag: observed,
                    count: 1,
                };
                let l = vm_read(regs, lhs, num_locals)?;
                let r = vm_read(regs, rhs, num_locals)?;
                regs[dst as usize] = self.eval_binary(l, op, r)?;
            }
        }
        Ok(())
    }

    /// Full `Insn::GetAttr` body: the InstanceAttr / ClassAttr inline-cache hit
    /// path followed by the slow-path `get_attr` call + cache fill / invalidation.
    /// The cache machinery (#1912, epoch/version guards #2102/#2108) is verbatim.
    #[inline(always)]
    // Hot-path VM instruction handler: the arg list is the decoded `GetAttr`
    // operands (dst/obj/name_idx + regs/code/pc/num_locals) feeding the inline
    // attribute cache; do not bundle into a struct on this dispatch-loop path.
    #[allow(clippy::too_many_arguments)]
    fn exec_get_attr(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        use crate::bytecode::AttrCacheEntry;
        use pyrust_core::UserFunctionKind;

        // Inline cache fast path: only for PyInstance objects.  Properties,
        // __class__, __dict__, cached_property, and megamorphic sites all fall
        // through to the slow path.
        enum AttrFastResult {
            Hit(Value),
            Miss,
        }
        let fast = {
            let cache = code.attr_cache.borrow();
            match &cache[pc - 1] {
                // Instance-attribute read cache (#1912): the name has no
                // data-descriptor shadow on the class, so the instance __dict__
                // takes priority.  Probe it directly, skipping the MRO walk in
                // get_attr_instance_raw.
                AttrCacheEntry::InstanceAttr {
                    class_ptr,
                    class_version,
                    epoch,
                } => {
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let same_class = Rc::as_ptr(&inst.class) as *const () == *class_ptr;
                        let version_ok =
                            inst.class.borrow().mutation_version.get() == *class_version;
                        let epoch_ok = pyrust_core::class_epoch() == *epoch;
                        if same_class && version_ok && epoch_ok {
                            let name = code.names.get(name_idx as usize).map(|s| s.as_str());
                            match name.and_then(|n| inst.attrs.get(n)) {
                                Some(v) => AttrFastResult::Hit(v.clone()),
                                // Not in the instance dict — fall to the slow
                                // path (method / non-data descriptor /
                                // __getattr__ / AttributeError).
                                None => AttrFastResult::Miss,
                            }
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                // `__slots__` slot read (#2207): the slot's value lives in the
                // same `inst.attrs` store as a plain instance attribute, so a
                // hit reads it directly — exactly what `member_descriptor.__get__`
                // does — skipping the data-descriptor dispatch path that made
                // slotted reads ~15× slower than plain instance reads.  An UNSET
                // slot (name absent from `inst.attrs`) is NOT served here: it
                // falls through to the slow path so the descriptor raises the
                // proper `AttributeError` (and any `__getattr__` runs), preserving
                // #2084's unset-slot semantics byte-for-byte.
                AttrCacheEntry::SlotAttr {
                    class_ptr,
                    class_version,
                    epoch,
                } => {
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let same_class = Rc::as_ptr(&inst.class) as *const () == *class_ptr;
                        let version_ok =
                            inst.class.borrow().mutation_version.get() == *class_version;
                        let epoch_ok = pyrust_core::class_epoch() == *epoch;
                        if same_class && version_ok && epoch_ok {
                            let name = code.names.get(name_idx as usize).map(|s| s.as_str());
                            match name.and_then(|n| inst.attrs.get(n)) {
                                Some(v) => AttrFastResult::Hit(v.clone()),
                                // Unset slot — slow path raises AttributeError.
                                None => AttrFastResult::Miss,
                            }
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                AttrCacheEntry::ClassAttr {
                    class_ptr,
                    class_version,
                    epoch,
                    value: unbound,
                } => {
                    // Check: object is a PyInstance, same class, no instance
                    // shadow, class not mutated, and the global class-mutation
                    // epoch unchanged (catches base-class mutations that don't
                    // bump the leaf class version).
                    if let Some(inst_rc) = regs[obj as usize].as_py_instance_rc() {
                        let inst = inst_rc.borrow();
                        let name_opt = code.names.get(name_idx as usize).map(|s| s.as_str());
                        let same_class = Rc::as_ptr(&inst.class) as *const () == *class_ptr;
                        // If name_idx is somehow out of range (bytecode
                        // invariant violation), treat as a shadow present —
                        // forces slow path.
                        let no_shadow =
                            name_opt.is_some_and(|n| !inst.attrs.contains_key(n));
                        let version_ok =
                            inst.class.borrow().mutation_version.get() == *class_version;
                        let epoch_ok = pyrust_core::class_epoch() == *epoch;
                        if same_class && no_shadow && version_ok && epoch_ok {
                            // Rebind the unbound class attr to this instance —
                            // same logic as get_attr's regular path, but avoids
                            // the MRO walk.
                            let unbound = unbound.clone();
                            let inst_rc_clone = Rc::clone(inst_rc);
                            let class_rc = Rc::clone(&inst.class);
                            drop(inst);
                            enum Tag {
                                Regular(std::rc::Rc<pyrust_core::UserFunction>),
                                ClassMethod(std::rc::Rc<pyrust_core::UserFunction>),
                                StaticMethod(std::rc::Rc<pyrust_core::UserFunction>),
                                // fn_name_matches: true when the
                                // BuiltinFunction's embedded name matches the
                                // attribute name; see the env.rs
                                // AttrKind::BuiltinFunction comment for the full
                                // rationale.
                                Builtin { fn_name_matches: bool },
                                Other,
                            }
                            let tag = match unbound.kind() {
                                ValueKind::UserFunction(f) => match f.kind {
                                    UserFunctionKind::Regular => Tag::Regular(Rc::clone(f)),
                                    UserFunctionKind::ClassMethod => {
                                        Tag::ClassMethod(Rc::clone(f))
                                    }
                                    UserFunctionKind::StaticMethod => {
                                        Tag::StaticMethod(Rc::clone(f))
                                    }
                                    UserFunctionKind::Builtin(_) => Tag::Builtin {
                                        fn_name_matches: false,
                                    },
                                },
                                ValueKind::BuiltinFunction(fn_name) => Tag::Builtin {
                                    fn_name_matches: name_opt.is_some_and(|n| {
                                        fn_name
                                            .rfind('.')
                                            .is_some_and(|i| &fn_name[i + 1..] == n)
                                    }),
                                },
                                _ => Tag::Other,
                            };
                            let bound = match tag {
                                Tag::Regular(f) => Value::bound_method(f, inst_rc_clone),
                                Tag::ClassMethod(f) => Value::class_bound_method(f, class_rc),
                                Tag::StaticMethod(f) => {
                                    if let Some(inner) = f.wrapped_func.as_ref() {
                                        Value::user_function(Rc::clone(inner))
                                    } else {
                                        Value::with_function_kind(
                                            f,
                                            pyrust_core::UserFunctionKind::Regular,
                                        )
                                    }
                                }
                                Tag::Builtin { fn_name_matches } => {
                                    if fn_name_matches {
                                        let n = name_opt.unwrap_or_default();
                                        pyrust_builtins::bound_method::bound_method(
                                            n.to_string(),
                                            Value::py_instance(inst_rc_clone),
                                        )
                                    } else {
                                        // The builtin was stored under a
                                        // user-chosen alias (e.g. A.f = len).
                                        // CPython does not bind it.
                                        unbound
                                    }
                                }
                                Tag::Other => unbound,
                            };
                            AttrFastResult::Hit(bound)
                        } else {
                            AttrFastResult::Miss
                        }
                    } else {
                        AttrFastResult::Miss
                    }
                }
                _ => AttrFastResult::Miss,
            }
        };
        match fast {
            AttrFastResult::Hit(result) => {
                regs[dst as usize] = result;
            }
            AttrFastResult::Miss => {
                let name = code.names.get(name_idx as usize).ok_or_else(|| {
                    PyError::Runtime(format!(
                        "bytecode error: name index {} out of range (pool size {})",
                        name_idx,
                        code.names.len()
                    ))
                })?;
                let obj_val = vm_read(regs, obj, num_locals)?;
                let result = self.get_attr(&obj_val, name)?;
                regs[dst as usize] = result;
                // Fill the cache after the slow path.  `fill_get_attr_cache` is
                // `#[inline(always)]`, so this is byte-identical to the old
                // inline `vm.rs` fill — a `#[cold]` out-of-line split was
                // measured to *regress* the bound-method hot path by ~6% (it
                // perturbed LLVM's codegen of the hot ClassAttr arm).
                fill_get_attr_cache(code, pc, name, &obj_val);
            }
        }
        Ok(())
    }

    /// Full `Insn::SetAttr` body: the SetInstanceAttr write-cache hit path
    /// followed by the slow-path `assign_attr` call + cache fill / invalidation
    /// (#1998).  The cache machinery is verbatim.
    #[inline(always)]
    // Hot-path VM instruction handler: the arg list is the decoded `SetAttr`
    // operands (obj/name_idx/val + regs/code/pc/num_locals) feeding the inline
    // attribute cache; do not bundle into a struct on this dispatch-loop path.
    #[allow(clippy::too_many_arguments)]
    fn exec_set_attr(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        val: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        use crate::bytecode::AttrCacheEntry;
        let obj_val = vm_read(regs, obj, num_locals)?;
        let val_val = vm_read(regs, val, num_locals)?;
        let name = code.names.get(name_idx as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: name index {} out of range (pool size {})",
                name_idx,
                code.names.len()
            ))
        })?;

        // Write inline cache fast path (#1998): a monomorphic site proven to be
        // a plain instance-dict write (no __setattr__ override, no __set__ data
        // descriptor on the MRO, no __slots__ restriction, not __class__/__dict__)
        // writes straight into inst.attrs, skipping the MRO walk in
        // assign_attr_instance.
        let mut handled = false;
        {
            let cache = code.attr_cache.borrow();
            if let AttrCacheEntry::SetInstanceAttr {
                class_ptr,
                class_version,
                epoch,
            } = &cache[pc - 1]
                && let Some(inst_rc) = obj_val.as_py_instance_rc() {
                    let (same_class, version_ok) = {
                        let inst = inst_rc.borrow();
                        (
                            Rc::as_ptr(&inst.class) as *const () == *class_ptr,
                            inst.class.borrow().mutation_version.get() == *class_version,
                        )
                    };
                    let epoch_ok = pyrust_core::class_epoch() == *epoch;
                    if same_class && version_ok && epoch_ok {
                        inst_rc
                            .borrow_mut()
                            .attrs
                            .insert(name, val_val.clone());
                        handled = true;
                    }
                }
        }
        if !handled {
            self.assign_attr(obj_val.clone(), name, val_val)?;
            // Fill / update the cache after the slow path.
            if name != "__class__" && name != "__dict__" {
                let mut cache = code.attr_cache.borrow_mut();
                match &cache[pc - 1] {
                    AttrCacheEntry::Megamorphic => {}
                    AttrCacheEntry::SetInstanceAttr {
                        class_ptr: existing_ptr,
                        ..
                    } => {
                        if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                            let new_ptr = Rc::as_ptr(&inst_rc.borrow().class) as *const ();
                            if new_ptr != *existing_ptr {
                                cache[pc - 1] = AttrCacheEntry::Megamorphic;
                            } else {
                                cache[pc - 1] = AttrCacheEntry::Empty;
                            }
                        }
                    }
                    AttrCacheEntry::Empty => {
                        // Cache only a plain instance-dict write: no __setattr__
                        // override, no __set__ data descriptor for this name on
                        // the MRO, the class has no __slots__ (would restrict
                        // assignment), and is not the bare object() singleton or
                        // an exception class (slot-type validation must keep
                        // running).
                        if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                            let class = Rc::clone(&inst_rc.borrow().class);
                            let no_setattr_override =
                                lookup_class_attr(&class, "__setattr__").is_none_or(|v| {
                                    matches!(
                                        v.kind(),
                                        ValueKind::BuiltinFunction(n)
                                            if n == "object.__setattr__"
                                    )
                                });
                            let no_data_desc = lookup_class_attr(&class, name)
                                .is_none_or(|v| !is_data_descriptor(&v));
                            let no_slots = class.borrow().slots.is_none();
                            let is_object_singleton =
                                Rc::ptr_eq(&class, &object_class_singleton());
                            let is_exc = is_exception_class(&class);
                            if no_setattr_override
                                && no_data_desc
                                && no_slots
                                && !is_object_singleton
                                && !is_exc
                            {
                                cache[pc - 1] = AttrCacheEntry::SetInstanceAttr {
                                    class_ptr: Rc::as_ptr(&class) as *const (),
                                    class_version: class.borrow().mutation_version.get(),
                                    epoch: pyrust_core::class_epoch(),
                                };
                            }
                        }
                    }
                    // ClassAttr / InstanceAttr are GetAttr-only entries; a
                    // SetAttr site never produces them.
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

/// Cache-fill for the `GetAttr` inline cache, called from `exec_get_attr`'s
/// slow-path Miss arm.  Factored out only for readability; `#[inline(always)]`
/// folds it straight back into `exec_get_attr`, so codegen is byte-identical to
/// the old inline `vm.rs` fill block (a `#[cold]` out-of-line split was measured
/// to *regress* the bound-method hot path by ~6%).  Fills `InstanceAttr`
/// (#1912) / `ClassAttr`, deopts cross-class sites to `Megamorphic`, and resets
/// stale same-class entries to `Empty`.
#[inline(always)]
fn fill_get_attr_cache(
    code: &crate::bytecode::FnCode,
    pc: usize,
    name: &str,
    obj_val: &Value,
) {
    use crate::bytecode::AttrCacheEntry;
    // Fill the cache after the slow path, but only for PyInstance targets that
    // resolve to a class attr (not an instance attr, not a property, not
    // __class__ / __dict__, and not megamorphic already).
    if name == "__class__" || name == "__dict__" {
        return;
    }
    let mut cache = code.attr_cache.borrow_mut();
    match &cache[pc - 1] {
        AttrCacheEntry::Megamorphic => {}
        AttrCacheEntry::ClassAttr {
            class_ptr: existing_ptr,
            ..
        }
        | AttrCacheEntry::InstanceAttr {
            class_ptr: existing_ptr,
            ..
        }
        | AttrCacheEntry::SlotAttr {
            class_ptr: existing_ptr,
            ..
        } => {
            if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                let new_ptr = Rc::as_ptr(&inst_rc.borrow().class) as *const ();
                if new_ptr != *existing_ptr {
                    // Different class at this call site — go megamorphic.
                    cache[pc - 1] = AttrCacheEntry::Megamorphic;
                } else {
                    // Same class but version changed, or the resolution flipped
                    // between instance/class attr (e.g. an instance attr was
                    // deleted and now resolves to a method).  Reset to Empty so
                    // the next slow-path execution refills.
                    cache[pc - 1] = AttrCacheEntry::Empty;
                }
            }
        }
        AttrCacheEntry::SetInstanceAttr { .. } => {
            // A SetAttr-only entry should never appear at a GetAttr site; if it
            // somehow does, drop it.
            cache[pc - 1] = AttrCacheEntry::Empty;
        }
        AttrCacheEntry::Empty => {
            // Try to fill: for PyInstance targets, either an instance-dict
            // resolution (#1912 InstanceAttr) or a class-attr resolution
            // (ClassAttr).
            if let Some(inst_rc) = obj_val.as_py_instance_rc() {
                let inst = inst_rc.borrow();
                let has_custom_getattribute =
                    lookup_class_attr(&inst.class, "__getattribute__")
                        .is_some_and(|v| matches!(v.kind(), ValueKind::UserFunction(_)));
                if inst.attrs.contains_key(name) {
                    // Instance attr.  Cache the fast path only when no data
                    // descriptor on the MRO shadows it (data descriptors take
                    // priority over the instance dict in CPython's lookup order)
                    // and there is no custom __getattribute__.  The numeric-tower
                    // names are read-only data descriptors on int/float, so
                    // exclude them too.
                    let is_numeric_tower = matches!(
                        name,
                        "real" | "imag" | "numerator" | "denominator"
                    );
                    let class_attr = lookup_class_attr(&inst.class, name);
                    // A `__slots__` slot is shadowed by a `member_descriptor`
                    // data descriptor whose `__get__` reads this very
                    // `inst.attrs[name]` (#2084).  That read is identical to a
                    // plain InstanceAttr probe, so cache a dedicated `SlotAttr`
                    // entry instead of giving up to the slow descriptor-dispatch
                    // path on every read (#2207).  Any OTHER data descriptor
                    // (property, user `__set__`/`__delete__`, numeric tower) is
                    // NOT cacheable here — it can run arbitrary code.
                    let is_slot_descriptor = class_attr.as_ref().is_some_and(|v| {
                        pyrust_builtins::member_descriptor::as_member_descriptor(v).is_some()
                    });
                    let shadowed_by_data_desc =
                        class_attr.as_ref().is_some_and(is_data_descriptor);
                    // `__traceback__` is stored as a deferred placeholder that
                    // must be materialised by get_attr's interceptor on every
                    // read (issue #2351); a cache hit would return the raw
                    // placeholder, so never fill the cache for this name.
                    if !has_custom_getattribute && !is_numeric_tower && name != "__traceback__" {
                        if is_slot_descriptor {
                            cache[pc - 1] = AttrCacheEntry::SlotAttr {
                                class_ptr: Rc::as_ptr(&inst.class) as *const (),
                                class_version: inst.class.borrow().mutation_version.get(),
                                epoch: pyrust_core::class_epoch(),
                            };
                        } else if !shadowed_by_data_desc {
                            cache[pc - 1] = AttrCacheEntry::InstanceAttr {
                                class_ptr: Rc::as_ptr(&inst.class) as *const (),
                                class_version: inst.class.borrow().mutation_version.get(),
                                epoch: pyrust_core::class_epoch(),
                            };
                        }
                    }
                } else {
                    // No instance attr — resolve via the class.  Don't cache when
                    // the class has a user-defined __getattribute__ — the cache
                    // bypasses get_attr entirely and would skip the
                    // __getattribute__ dispatch (issue #1254).
                    if !has_custom_getattribute {
                        // No property — we only cache the straightforward
                        // class-attr case.
                        let unbound = lookup_class_attr(&inst.class, name);
                        if let Some(unbound_val) = unbound {
                            // Don't cache cached_property (it mutates
                            // instance.attrs on first access and must go through
                            // get_attr).
                            let is_cached_prop =
                                pyrust_builtins::cached_property::with_cached_property(
                                    &unbound_val,
                                    |_| (),
                                )
                                .is_some();
                            let is_property = pyrust_builtins::property::with_property(
                                &unbound_val,
                                |_| (),
                            )
                            .is_some();
                            // A `member_descriptor` reached here means an UNSET
                            // `__slots__` slot (the name is absent from the
                            // instance dict).  The slow path correctly raised
                            // `AttributeError`; caching it as a `ClassAttr` would
                            // wrongly return the raw descriptor object on the next
                            // read.  Leave the site Empty so it refills via the
                            // descriptor slow path until the slot is set (then it
                            // becomes a `SlotAttr` via the contains_key branch).
                            let is_slot_descriptor =
                                pyrust_builtins::member_descriptor::as_member_descriptor(
                                    &unbound_val,
                                )
                                .is_some();
                            if !is_cached_prop && !is_property && !is_slot_descriptor {
                                let class_ptr = Rc::as_ptr(&inst.class) as *const ();
                                let class_version = inst.class.borrow().mutation_version.get();
                                let epoch = pyrust_core::class_epoch();
                                cache[pc - 1] = AttrCacheEntry::ClassAttr {
                                    class_ptr,
                                    class_version,
                                    epoch,
                                    value: unbound_val,
                                };
                            }
                        }
                    }
                }
            }
        }
    }
}
