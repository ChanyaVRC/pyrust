// ---------------------------------------------------------------------------
// Built-in iterator step family (`step_*_iter`), owned by iteration semantics.
//
// One cohesive group of six lazy "advance one element" steps backing the
// built-in iterator types (`getitem` / `callable` / `map` / `filter` /
// `enumerate` / `zip`).  Each is called once per produced element from
// `call_next` and from the `ForIter` dispatch, so they are marked `#[inline]`
// to keep the file boundary zero-cost.  (Forcing `#[inline(always)]` here was
// measured to *regress* `sum(map(user_fn, ...))` by bloating the already-huge
// `call_next`; plain `#[inline]` is perf-neutral — see the PR perf table.)
// Iterator construction, materialisation, and one-step advancement belong to
// this domain. Python-visible `next()` argument validation remains at the
// built-in API boundary.
// ---------------------------------------------------------------------------

fn zip_shorter_message(short_idx: usize) -> String {
    if short_idx == 1 {
        format!(
            "zip() argument {} is shorter than argument 1",
            short_idx + 1
        )
    } else {
        format!(
            "zip() argument {} is shorter than arguments 1-{}",
            short_idx + 1,
            short_idx
        )
    }
}

fn zip_longer_message(long_idx: usize) -> String {
    if long_idx == 1 {
        format!("zip() argument {} is longer than argument 1", long_idx + 1)
    } else {
        format!(
            "zip() argument {} is longer than arguments 1-{}",
            long_idx + 1,
            long_idx
        )
    }
}

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
        let snapshot: Option<(Value, Value, i64, i64)> = {
            let borrow = state_rc.borrow();
            if let Some(it) = borrow.downcast_ref::<GetItemIter>() {
                if it.exhausted || it.remaining == Some(0) {
                    return Ok(None);
                }
                Some((it.obj.clone(), it.method.clone(), it.index, it.step))
            } else {
                return Err(PyError::Runtime(
                    "step_getitem_iter on non-GetItemIter state".to_string(),
                ));
            }
        };
        let (obj, method, index, step) = snapshot.unwrap();
        let arg = ExpandedCallArg {
            name: None,
            value: Value::int(index),
        };
        let result = invoke_class_method(self, method, obj, &[arg]);
        match result {
            Ok(v) => {
                if let Some(it) = state_rc.borrow_mut().downcast_mut::<GetItemIter>() {
                    it.index = it.index.saturating_add(step);
                    if let Some(remaining) = &mut it.remaining {
                        *remaining -= 1;
                    }
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
            let s = borrow.downcast_ref::<MapIter>().ok_or_else(|| {
                PyError::Runtime("step_map_iter on non-MapIter state".to_string())
            })?;
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
                Ok(v) => args.push(ExpandedCallArg {
                    name: None,
                    value: v,
                }),
                Err(e) if is_stop_iteration_error(&e) => {
                    state_rc
                        .borrow_mut()
                        .downcast_mut::<MapIter>()
                        .unwrap()
                        .done = true;
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
                Err(e) if is_stop_iteration_error(&e) => {
                    state_rc
                        .borrow_mut()
                        .downcast_mut::<FilterIter>()
                        .unwrap()
                        .done = true;
                    return Ok(None);
                }
                Err(e) => return Err(e),
            };
            let keep = if let Some(func) = &func_opt {
                let test = self.call_function_expanded(
                    func.clone(),
                    &[ExpandedCallArg {
                        name: None,
                        value: item.clone(),
                    }],
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

    /// Advance a standard-library iterator through its owner-supplied typed
    /// callback. The generic runtime never inspects the provider's cursor or
    /// names its public API.
    #[inline]
    pub(crate) fn step_provider_iterator(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        let (advance, provider_state) = {
            let state = state_rc.borrow();
            state
                .downcast_ref::<ProviderIterator>()
                .map(ProviderIterator::advance_parts)
                .ok_or_else(|| {
                    PyError::Runtime("invalid standard-library iterator adapter".to_string())
                })?
        };
        advance(self, &provider_state)
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
            Err(e) if is_stop_iteration_error(&e) => {
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

    /// One step of the lazy i64-backed range iterator.
    pub(crate) fn step_range_iter(
        &mut self,
        state_rc: &Rc<RefCell<Box<dyn std::any::Any>>>,
    ) -> Result<Option<Value>> {
        let mut borrow = state_rc.borrow_mut();
        let state = borrow.downcast_mut::<RangeIter>().ok_or_else(|| {
            PyError::Runtime("step_range_iter on non-RangeIter state".to_string())
        })?;
        let exhausted = if state.step > 0 {
            state.cur >= state.stop
        } else {
            state.cur <= state.stop
        };
        if exhausted {
            return Ok(None);
        }

        let value = state.cur;
        // A valid range never yields an out-of-i64 value. Saturation handles
        // the final overshooting addition without wrapping the cursor back
        // across the stop boundary.
        state.cur = state.cur.saturating_add(state.step);
        Ok(Some(Value::int(value)))
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
        let (n_sources, strict) = {
            let borrow = state_rc.borrow();
            let s = borrow.downcast_ref::<ZipIter>().ok_or_else(|| {
                PyError::Runtime("step_zip_iter on non-ZipIter state".to_string())
            })?;
            if s.done {
                return Ok(None);
            }
            (s.sources.len(), s.strict)
        };
        if n_sources == 0 {
            state_rc
                .borrow_mut()
                .downcast_mut::<ZipIter>()
                .unwrap()
                .done = true;
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
                Err(e) if is_stop_iteration_error(&e) => {
                    stopped_at = Some(i);
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        if let Some(short_idx) = stopped_at {
            state_rc
                .borrow_mut()
                .downcast_mut::<ZipIter>()
                .unwrap()
                .done = true;
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
                                return Err(pyrust_core::value_err!(zip_longer_message(j)));
                            } else {
                                return Err(pyrust_core::value_err!(zip_shorter_message(
                                    short_idx
                                )));
                            }
                        }
                        Err(e) if is_stop_iteration_error(&e) => {}
                        Err(e) => return Err(e),
                    }
                }
                if short_idx > 0 {
                    return Err(pyrust_core::value_err!(zip_shorter_message(short_idx)));
                }
            }
            return Ok(None);
        }
        state_rc
            .borrow_mut()
            .downcast_mut::<ZipIter>()
            .unwrap()
            .count += 1;
        Ok(Some(Value::tuple(row)))
    }
}
