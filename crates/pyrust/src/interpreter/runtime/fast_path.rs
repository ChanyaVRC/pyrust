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
//
// What deliberately stayed inline at its dispatch site (extracting would change
// logic or hurt the hot path, not move it):
//   - the attr inline-cache match arms in the `vm.rs` GetAttr/SetAttr dispatch
//     loop, fused into the hot dispatch loop (extracting risks the hot path).
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
fn with_set_items<R>(v: &Value, f: impl FnOnce(&indexmap::IndexSet<PyKey>) -> R) -> R {
    if let Some(rc) = pyrust_builtins::frozenset::as_items(v) {
        return f(&rc);
    }
    v.set_with(f).expect("set_direct_value guarantees a set/frozenset value")
}

/// Primitive-key set algebra over borrowed operands: clones only the elements
/// that land in the result and builds it with a capacity hint (issue #1978).
#[inline]
fn set_algebra_fast(
    a: &indexmap::IndexSet<PyKey>,
    b: &indexmap::IndexSet<PyKey>,
    op: SetOp,
) -> indexmap::IndexSet<PyKey> {
    let cap = match op {
        SetOp::And => a.len().min(b.len()),
        SetOp::Sub => a.len(),
        SetOp::Or => a.len() + b.len(),
        SetOp::Xor => a.len() + b.len(),
    };
    let mut out = indexmap::IndexSet::with_capacity(cap);
    match op {
        SetOp::Or => {
            for k in a.iter().chain(b.iter()) {
                out.insert(k.clone());
            }
        }
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
        let (iter_val, counter): (Value, i64) = {
            let borrow = state_rc.borrow();
            let s = borrow.downcast_ref::<EnumerateIter>().ok_or_else(|| {
                PyError::Runtime("step_enumerate_iter on non-EnumerateIter state".to_string())
            })?;
            if s.done {
                return Ok(None);
            }
            (s.source.clone(), s.counter)
        };
        match self.call_next(&iter_val, None) {
            Ok(item) => {
                let mut borrow = state_rc.borrow_mut();
                let s = borrow.downcast_mut::<EnumerateIter>().unwrap();
                s.counter += 1;
                Ok(Some(Value::tuple(vec![Value::int(counter), item])))
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
            Ok(Value::string(string[byte_start..byte_end].to_string()))
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
                    .insert(function.params[pi].name.clone(), val);
            }
        }
        pyrust_core::ParamBind::None => {}
    }
    Ok(())
}
