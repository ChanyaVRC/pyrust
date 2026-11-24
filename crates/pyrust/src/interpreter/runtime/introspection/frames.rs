fn refresh_cached_frame_state(frame: &Value, lineno: i64) {
    let ValueKind::BuiltinObject { state, .. } = frame.kind() else {
        return;
    };
    let mut state = state.borrow_mut();
    let Some(state) = state.downcast_mut::<frame_obj::FrameState>() else {
        return;
    };
    // Recursive f_back construction has no line for the suspended outer frame
    // and passes 0. Do not erase a real line published by an earlier direct
    // lookup of that same cached object.
    if lineno != 0 {
        state.lineno = lineno;
    }
}

fn initial_function_local_names(view: &VmFrameView, snapshot: &PyDict) -> Vec<PyKey> {
    let mut names = compiler_owned_frame_local_keys(view);
    // Snapshot-only keys are resolved free variables. Record them once they
    // appear so a later unbind removes their old value without treating an
    // arbitrary key inserted through f_locals as compiler-owned.
    for key in snapshot.keys() {
        if !names.contains(key) {
            names.push(key.clone());
        }
    }
    names
}

fn sync_function_frame_locals(
    locals: &Rc<std::cell::RefCell<PyDict>>,
    snapshot: PyDict,
    known_names: &mut Vec<PyKey>,
) {
    for key in snapshot.keys() {
        if !known_names.contains(key) {
            known_names.push(key.clone());
        }
    }
    let removed = known_names
        .iter()
        .filter(|key| !snapshot.contains_key(*key))
        .cloned()
        .collect();
    let locals_value = Value::dict_shared(Rc::clone(locals));
    locals_value
        .dict_shift_remove_many(removed)
        .expect("cached frame locals must remain a dict");
    locals_value
        .dict_extend(snapshot.into_iter().collect())
        .expect("cached frame locals must remain a dict");
}

fn with_frame_cache<R>(
    interp: &Interpreter,
    view_index: usize,
    read: impl FnOnce(Option<&VmFrameCache>) -> R,
) -> R {
    let view = &interp.vm_frame_views[view_index];
    if let Some(gen_frame) = view.gen_frame {
        // SAFETY: `gen_frame` points to the heap-stable `GeneratorFrame` for
        // exactly as long as this active view is on `vm_frame_views`; both VM
        // entry paths pop the view before the frame can move or be dropped.
        // Reconstruct only a shared reference and access only the cache's
        // `RefCell`. The borrow is scoped to this non-reentrant callback.
        let gen_frame = unsafe { gen_frame.as_ref() };
        let cache = gen_frame.frame_cache.borrow();
        read(cache.as_deref())
    } else {
        let cache = interp
            .vm_frame_caches
            .as_deref()
            .and_then(|caches| caches.get(view_index))
            .and_then(Option::as_deref);
        read(cache)
    }
}

fn with_frame_cache_mut<R>(
    interp: &mut Interpreter,
    view_index: usize,
    update: impl FnOnce(&mut Option<Box<VmFrameCache>>) -> R,
) -> R {
    if let Some(gen_frame) = interp.vm_frame_views[view_index].gen_frame {
        // SAFETY: same lifetime guarantee as `with_frame_cache`. This never
        // creates a raw-derived `&mut GeneratorFrame`; mutation is confined to
        // the dedicated `RefCell`, whose short borrow ends before this returns.
        let gen_frame = unsafe { gen_frame.as_ref() };
        let mut cache = gen_frame.frame_cache.borrow_mut();
        update(&mut cache)
    } else {
        let caches = interp
            .vm_frame_caches
            .get_or_insert_with(|| Box::new(Vec::new()));
        if caches.len() <= view_index {
            caches.resize_with(view_index + 1, || None);
        }
        update(&mut caches[view_index])
    }
}

fn cached_frame_object(interp: &Interpreter, view_index: usize) -> Option<Value> {
    with_frame_cache(interp, view_index, |cache| {
        cache
            .and_then(|cache| cache.object.as_ref())
            .and_then(pyrust_core::WeakValueCache::upgrade)
    })
}

fn cached_function_locals(
    interp: &Interpreter,
    view_index: usize,
) -> Option<Rc<std::cell::RefCell<PyDict>>> {
    with_frame_cache(interp, view_index, |cache| {
        cache.and_then(|cache| {
            cache.persistent_function_locals.clone().or_else(|| {
                cache
                    .function_locals
                    .as_ref()
                    .and_then(std::rc::Weak::upgrade)
            })
        })
    })
}

fn install_materialized_frame_cache(
    interp: &mut Interpreter,
    view_index: usize,
    object: pyrust_core::WeakValueCache,
    function_locals: Option<std::rc::Weak<std::cell::RefCell<PyDict>>>,
    function_local_names: Vec<PyKey>,
) {
    with_frame_cache_mut(interp, view_index, |slot| {
        if let Some(cache) = slot.as_deref_mut() {
            cache.object = Some(object);
            if cache
                .function_locals
                .as_ref()
                .and_then(std::rc::Weak::upgrade)
                .is_none()
            {
                cache.function_locals = function_locals;
                cache.function_local_names = function_local_names;
            }
        } else {
            *slot = Some(Box::new(VmFrameCache {
                object: Some(object),
                function_locals,
                persistent_function_locals: None,
                function_local_names,
            }));
        }
    });
}

impl Interpreter {
    /// Publish the running source line only for the frame-introspection
    /// built-in. Exact `sys` API recognition belongs to this domain rather
    /// than the opcode loop or generic method-call router.
    #[inline]
    pub(crate) fn publish_frame_line_for_builtin(callable: &Value, line: u32) {
        if matches!(
            callable.kind(),
            ValueKind::BuiltinFunction(name) if name.ends_with("_getframe")
        ) {
            pyrust_core::set_current_vm_line(line);
        }
    }

    /// Detach the top non-generator frame view from `vm_frame_views` without
    /// yet touching `vm_frame_caches`. Callers that still own the popped
    /// frame's register slice drop/truncate those registers and restore the
    /// caller's `self.env` first (so non-escaping frame objects drop before
    /// cache inspection, and any escaping frame that links to its parent sees
    /// the caller's environment), then call [`Self::finish_pop_vm_frame_view`]
    /// (issue #3028).
    #[inline]
    pub(crate) fn detach_vm_frame_view(&mut self) -> Option<VmFrameView> {
        let view = self.vm_frame_views.pop()?;
        debug_assert!(
            view.gen_frame.is_none(),
            "generator frame views must use the direct hot-path pop"
        );
        Some(view)
    }

    /// Finalize frame-cache cleanup after [`Self::detach_vm_frame_view`] once
    /// the popped frame's registers are dropped and the caller's environment is
    /// active again.
    #[inline]
    pub(crate) fn finish_pop_vm_frame_view(&mut self) {
        let new_len = self.vm_frame_views.len();
        if self
            .vm_frame_caches
            .as_ref()
            .is_some_and(|caches| caches.len() > new_len)
        {
            self.truncate_vm_frame_caches(new_len);
        }
    }

    /// Pop one non-generator frame view and discard any materialized cache for
    /// that activation. Generator views use direct `Vec::pop` on every hot
    /// resume/exit path and own no side-vector slot.
    #[inline]
    pub(crate) fn pop_vm_frame_view(&mut self) -> Option<VmFrameView> {
        let view = self.detach_vm_frame_view()?;
        self.finish_pop_vm_frame_view();
        Some(view)
    }

    /// Link an escaping generator frame to the caller that exhausted it
    /// (issue #3028). Called only when a generator transitions to `done = true`
    /// and already has a materialized `frame_cache`; yielded frames intentionally
    /// keep `f_back = None`, matching CPython 3.12.
    #[cold]
    #[inline(never)]
    pub(crate) fn link_finished_generator_frame_back(&mut self, frame: &GeneratorFrame) {
        if self.vm_frame_views.is_empty() {
            return;
        }
        let cached = frame
            .frame_cache
            .borrow()
            .as_deref()
            .and_then(|cache| cache.object.as_ref())
            .and_then(pyrust_core::WeakValueCache::upgrade);
        if let Some(frame_object) = cached
            && frame_obj::stored_back(&frame_object).is_none_or(|b| b.is_none())
        {
            let back = self.build_frame_object(0, 0);
            frame_obj::set_back(&frame_object, back);
        }
    }

    #[cold]
    #[inline(never)]
    fn truncate_vm_frame_caches(&mut self, new_len: usize) {
        let mut escaping_frames: Vec<Value> = Vec::new();
        let remove_vector = {
            let caches = self
                .vm_frame_caches
                .as_deref_mut()
                .expect("cache vector checked by finish_pop_vm_frame_view");
            for slot in caches.drain(new_len..) {
                if let Some(cache) = slot
                    && let Some(frame) = cache
                        .object
                        .as_ref()
                        .and_then(pyrust_core::WeakValueCache::upgrade)
                {
                    escaping_frames.push(frame);
                }
            }
            while caches.last().is_some_and(Option::is_none) {
                caches.pop();
            }
            caches.is_empty()
        };
        if remove_vector {
            self.vm_frame_caches = None;
        }
        // Link genuinely escaping frames to their immediate live parent
        // (#3028). Non-escaping frames already dropped with the callee's
        // registers so `upgrade()` returns `None` and no outer frame is built.
        if !escaping_frames.is_empty() && !self.vm_frame_views.is_empty() {
            let mut parent: Option<Value> = None;
            for frame in escaping_frames {
                if frame_obj::stored_back(&frame).is_none_or(|b| b.is_none()) {
                    let back = parent
                        .get_or_insert_with(|| self.build_frame_object(0, 0))
                        .clone();
                    frame_obj::set_back(&frame, back);
                }
            }
        }
    }

    fn sync_cached_function_locals_at(&mut self, view_index: usize) {
        let (cached_locals, compiler_owned_names) = {
            let view = &self.vm_frame_views[view_index];
            if view.kind != FrameKind::Function {
                return;
            }
            (
                cached_function_locals(self, view_index),
                compiler_owned_frame_local_keys(view),
            )
        };
        let Some(locals) = cached_locals else {
            return;
        };
        let fresh = snapshot_view_locals(self, &self.vm_frame_views[view_index]);
        with_frame_cache_mut(self, view_index, |slot| {
            let cache = slot.as_deref_mut().expect("checked frame cache");
            for key in compiler_owned_names {
                if !cache.function_local_names.contains(&key) {
                    cache.function_local_names.push(key);
                }
            }
            sync_function_frame_locals(&locals, fresh, &mut cache.function_local_names);
        });
    }

    /// Return the persistent locals mapping exposed by `locals()` and used by
    /// argument-omitted `exec()` in the innermost function frame. It outlives
    /// each builtin call so exec-created keys remain visible to every alias in
    /// the same activation.
    pub(crate) fn retain_current_function_locals(&mut self) -> Value {
        let view_index = self
            .vm_frame_views
            .len()
            .checked_sub(1)
            .expect("implicit function exec requires an active frame");
        debug_assert_eq!(self.vm_frame_views[view_index].kind, FrameKind::Function);

        if let Some(locals) = cached_function_locals(self, view_index) {
            self.sync_cached_function_locals_at(view_index);
            with_frame_cache_mut(self, view_index, |slot| {
                slot.as_deref_mut()
                    .expect("cached function locals require a frame cache")
                    .persistent_function_locals = Some(Rc::clone(&locals));
            });
            return Value::dict_shared(locals);
        }

        let snapshot = snapshot_view_locals(self, &self.vm_frame_views[view_index]);
        let function_local_names =
            initial_function_local_names(&self.vm_frame_views[view_index], &snapshot);
        let locals_value = Value::dict(snapshot);
        let locals = Rc::clone(
            locals_value
                .get_dict_rc()
                .expect("function exec locals must be a dict"),
        );
        let weak_locals = Rc::downgrade(&locals);
        with_frame_cache_mut(self, view_index, |slot| {
            if let Some(cache) = slot.as_deref_mut() {
                cache.function_locals = Some(weak_locals);
                cache.persistent_function_locals = Some(locals);
                cache.function_local_names = function_local_names;
            } else {
                *slot = Some(Box::new(VmFrameCache {
                    object: None,
                    function_locals: Some(weak_locals),
                    persistent_function_locals: Some(locals),
                    function_local_names,
                }));
            }
        });
        locals_value
    }

    /// Refresh a live function frame's persistent locals mapping when Python
    /// reads `frame.f_locals`. CPython performs fast-locals synchronization in
    /// the attribute getter, not when the frame object itself is looked up.
    pub(crate) fn refresh_live_frame_locals_for_attribute(&mut self, frame: &Value) {
        let view_index = self
            .vm_frame_views
            .iter()
            .enumerate()
            .rposition(|(view_index, _)| {
                cached_frame_object(self, view_index)
                    .as_ref()
                    .is_some_and(|cached| values_are_identical(cached, frame))
            });
        let Some(view_index) = view_index else {
            return;
        };
        self.sync_cached_function_locals_at(view_index);
    }

    /// True when `view` is an implicit PEP 709 inlined comprehension helper
    /// (`<listcomp>`/`<setcomp>`/`<dictcomp>`), which CPython 3.12 does not
    /// expose as a separate frame to `sys._getframe()` (issue #2993).
    fn view_is_inlined_comp(view: &VmFrameView) -> bool {
        if let Some(func) = view.function.as_ref()
            && let Some(code_any) = func.precompiled_code.as_ref()
            && let Ok(code) = Rc::clone(code_any).downcast::<crate::bytecode::FnCode>()
            && code.is_inlined_comp
        {
            return true;
        }
        if let Some(gen_frame) = view.gen_frame {
            // SAFETY: `gen_frame` is alive while this view is on `vm_frame_views`.
            if unsafe { gen_frame.as_ref() }.code.is_inlined_comp {
                return true;
            }
        }
        false
    }

    /// Translate a Python-visible frame depth `idx` (0 = innermost non-inlined
    /// frame) into its index inside `self.vm_frame_views`.
    fn visible_frame_view_index(&self, idx: usize) -> Option<usize> {
        let mut remaining = idx;
        for (view_index, view) in self.vm_frame_views.iter().enumerate().rev() {
            if Self::view_is_inlined_comp(view) {
                continue;
            }
            if remaining == 0 {
                return Some(view_index);
            }
            remaining -= 1;
        }
        None
    }

    /// Resolve `f_back` on demand for any live frame object (issues #2993,
    /// #3028).
    ///
    /// Returns `None` if `frame` is not currently active on `vm_frame_views`
    /// (unwound, suspended generator, or traceback frame), letting `FrameOps`
    /// return its stored `FrameState.back`.  For active generator frames the
    /// caller is resolved dynamically without mutating `FrameState.back`
    /// (since a generator's caller changes across resumes); for active
    /// non-generator frames the immediate parent is built in O(1) and cached
    /// in `FrameState.back`.
    pub(crate) fn live_frame_back_for_attribute(&mut self, frame: &Value) -> Option<Value> {
        let view_index = self
            .vm_frame_views
            .iter()
            .enumerate()
            .rposition(|(view_index, _)| {
                cached_frame_object(self, view_index)
                    .as_ref()
                    .is_some_and(|cached| values_are_identical(cached, frame))
            })?;
        if view_index == 0 {
            return Some(Value::none());
        }
        let visible_above = self.vm_frame_views[view_index + 1..]
            .iter()
            .filter(|v| !Self::view_is_inlined_comp(v))
            .count();
        if self.vm_frame_views[view_index].gen_frame.is_some() {
            return Some(self.build_frame_object(visible_above + 1, 0));
        }
        if let Some(stored) = frame_obj::stored_back(frame)
            && !stored.is_none()
        {
            return Some(stored);
        }
        let back = self.build_frame_object(visible_above + 1, 0);
        frame_obj::set_back(frame, back.clone());
        Some(back)
    }

    /// Build or reuse the single frame object at `idx` (zero is the innermost
    /// frame).  `f_back` is initialized to `None` and resolved lazily on
    /// attribute access (or on unwind if the frame escapes), keeping
    /// `sys._getframe()` O(1) regardless of stack depth (issue #3028).
    pub(crate) fn build_frame_object(&mut self, idx: usize, lineno: i64) -> Value {
        let Some(view_index) = self.visible_frame_view_index(idx) else {
            return Value::none();
        };

        // A live activation owns one Python frame identity. Upgrade the weak
        // value only on introspection; an ordinary call allocates no side
        // storage. A successful upgrade reconstructs a Value that shares the
        // existing frame state.
        if let Some(frame) = cached_frame_object(self, view_index) {
            refresh_cached_frame_state(&frame, lineno);
            return frame;
        }

        // Initialize `f_back` lazily to `None`; `live_frame_back_for_attribute`
        // materializes the immediate parent on demand and
        // `truncate_vm_frame_caches` links genuinely escaping frames on unwind.
        let back = Value::none();
        let view = &self.vm_frame_views[view_index];

        let code = match &view.function {
            Some(func) => self.build_code_object(func),
            None => {
                if let Some(gen_frame) = view.gen_frame {
                    // SAFETY: `gen_frame` points to the heap-stable
                    // `GeneratorFrame` for the lifetime of this active view on
                    // `vm_frame_views`.
                    let gen_frame = unsafe { gen_frame.as_ref() };
                    self.build_code_from_fncode(
                        &gen_frame.code,
                        gen_frame.fn_name.as_ref(),
                        gen_frame.fn_name.as_ref(),
                        &gen_frame.local_index,
                    )
                } else {
                    // Script / Class frame: synthesise a minimal code object
                    // whose co_name matches CPython (`<module>` for module
                    // scope). Carry the running script's path into
                    // `co_filename` (#2438) so
                    // `sys._getframe().f_code.co_filename` reports the source
                    // file rather than `<unknown>`.
                    let filename = self
                        .script_filename
                        .as_ref()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    code_obj::code_with_loc("<module>".to_string(), 0, Vec::new(), filename, 0)
                }
            }
        };

        let env = view.env.as_ref().unwrap_or(&self.env);
        let globals = self.globals_for_environment(env);
        // f_locals reads this frame's namespace — including for a suspended
        // *outer* frame, whose register file is still live on the call stack
        // (issue #2926; the old code returned an empty dict for every
        // `idx > 0`, so `sys._getframe(1).f_locals` was always `{}`).
        //
        // A module frame does not get a snapshot at all: CPython's module
        // f_locals IS the live module dict, so it must come from the same
        // provider `globals()` uses or the `f_locals is f_globals` identity
        // (and mutation-through-f_locals) would not hold. Class frames likewise
        // expose their live namespace; only function frames retain snapshots.
        let frame_kind = view.kind;
        let (locals, function_local_names) = match frame_kind {
            FrameKind::Script => (self.frame_locals_for_module_environment(env), Vec::new()),
            FrameKind::Class => (class_frame_locals_value(self, view), Vec::new()),
            FrameKind::Function => {
                if let Some(locals) = cached_function_locals(self, view_index) {
                    (Value::dict_shared(locals), Vec::new())
                } else {
                    let snapshot = snapshot_view_locals(self, view);
                    let names = initial_function_local_names(view, &snapshot);
                    (Value::dict(snapshot), names)
                }
            }
        };

        let function_locals = (frame_kind == FrameKind::Function).then(|| {
            Rc::downgrade(
                locals
                    .get_dict_rc()
                    .expect("function frame locals must be a dict"),
            )
        });
        let frame = frame_obj::frame(code, lineno, back, globals, locals);
        let object = pyrust_core::WeakValueCache::new(&frame)
            .expect("frame BuiltinObject must support weak caching");
        install_materialized_frame_cache(
            self,
            view_index,
            object,
            function_locals,
            function_local_names,
        );
        frame
    }

    /// Build the `gi_frame`, `cr_frame`, or `ag_frame` object for a suspended
    /// generator-backed activation, or `Value::none()` when it is exhausted.
    /// The frame's
    /// `f_lineno` is the source line the generator is suspended on (the line of
    /// the `yield` it last paused at); `f_code` is a code object carrying the
    /// generator function's `co_name` / `co_firstlineno` / `co_consts` etc.
    /// (issue #2185).
    ///
    /// `qualname` comes from the generator object rather than the frame: the
    /// writable name pair is stored beside the execution state, not in it
    /// (#2978).
    pub(crate) fn build_generator_frame_object(
        &self,
        frame: &GeneratorFrame,
        qualname: &str,
    ) -> Value {
        if frame.done {
            return Value::none();
        }
        // Current line: a suspended generator stores `pc` as the *resume* point
        // (the instruction after the `Yield`), whose line may already be the
        // next statement.  The line CPython reports is the `yield` it paused at,
        // which is at `pc - 1`.  Scan `[..pc]` (i.e. up to and including the
        // Yield) backward for the last entry that starts a new source line (a
        // `0` entry means "same line as the previous instruction").  When the
        // generator has not started yet (`pc == 0`), CPython reports the `def`
        // line (`first_lineno`).
        let lineno = if frame.pc == 0 {
            frame.code.first_lineno as i64
        } else {
            frame
                .code
                .lineno_table
                .iter()
                .take(frame.pc)
                .rev()
                .copied()
                .find(|&n| n != 0)
                .unwrap_or(frame.code.first_lineno) as i64
        };

        let (cached_frame, retained_locals) = {
            let cache = frame.frame_cache.borrow();
            let cache = cache.as_deref();
            (
                cache
                    .and_then(|cache| cache.object.as_ref())
                    .and_then(pyrust_core::WeakValueCache::upgrade),
                cache.and_then(|cache| {
                    cache.persistent_function_locals.clone().or_else(|| {
                        cache
                            .function_locals
                            .as_ref()
                            .and_then(std::rc::Weak::upgrade)
                    })
                }),
            )
        };
        if let Some(cached_frame) = cached_frame {
            refresh_cached_frame_state(&cached_frame, lineno);
            return cached_frame;
        }

        let code = self.build_code_from_fncode(
            &frame.code,
            frame.fn_name.as_ref(),
            qualname,
            &frame.local_index,
        );
        let locals = retained_locals.unwrap_or_default();
        let frame_object = frame_obj::frame(
            code,
            lineno,
            Value::none(),
            self.globals_for_environment(&frame.saved_env),
            Value::dict_shared(Rc::clone(&locals)),
        );
        let object = pyrust_core::WeakValueCache::new(&frame_object)
            .expect("frame BuiltinObject must support weak caching");
        let function_locals = Rc::downgrade(&locals);
        let mut cache = frame.frame_cache.borrow_mut();
        if let Some(cache) = cache.as_deref_mut() {
            cache.object = Some(object);
            if cache
                .function_locals
                .as_ref()
                .and_then(std::rc::Weak::upgrade)
                .is_none()
            {
                cache.function_locals = Some(function_locals);
            }
        } else {
            *cache = Some(Box::new(VmFrameCache {
                object: Some(object),
                function_locals: Some(function_locals),
                persistent_function_locals: None,
                function_local_names: Vec::new(),
            }));
        }
        frame_object
    }

    /// Build a `code` object directly from a compiled `FnCode` (plus the name /
    /// qualname / local-name map), for callers that hold a `FnCode` but no
    /// `UserFunction` — currently generator `gi_frame` (issue #2185).  Populates
    /// the FnCode-derived attributes (`co_firstlineno`, `co_consts`, `co_names`,
    /// `co_cellvars`, `co_varnames` body locals, `co_stacksize`); the
    /// signature-derived counts (`co_argcount` etc.) are reported as 0 since the
    /// parameter list is not recoverable from the `FnCode` alone.
    fn build_code_from_fncode(
        &self,
        fncode: &crate::bytecode::FnCode,
        name: &str,
        qualname: &str,
        local_index: &std::collections::HashMap<String, crate::bytecode::Reg>,
    ) -> Value {
        let cellvar_set: std::collections::HashSet<&str> =
            fncode.cell_vars.iter().map(|s| s.as_str()).collect();
        // co_varnames: the body locals in register-slot order, excluding cell
        // variables.  (Parameters are also in `local_index`; without the
        // signature we cannot reorder them into CPython's posonly/kwonly groups,
        // so we report all locals in slot order — a best-effort that still lists
        // every name with the right membership.)
        let mut locals: Vec<(u32, &str)> = local_index
            .iter()
            .filter(|(n, _)| !cellvar_set.contains(n.as_str()))
            .map(|(n, &slot)| (slot, n.as_str()))
            .collect();
        locals.sort_by_key(|(slot, _)| *slot);
        let varnames: Vec<Value> = locals.iter().map(|(_, n)| Value::string(n)).collect();
        let nlocals = varnames.len() as i64;

        let mut cellvars: Vec<String> = fncode.cell_vars.iter().cloned().collect();
        cellvars.sort();
        let cellvars: Vec<Value> = cellvars.into_iter().map(Value::string).collect();

        let mut consts: Vec<Value> = Vec::new();
        consts.push(Value::none());
        consts.extend(fncode.consts.iter().filter(|c| !c.is_none()).cloned());
        let names: Vec<Value> = fncode
            .names
            .iter()
            .map(|n| Value::string(n.clone()))
            .collect();

        let mut flags = code_obj::CO_OPTIMIZED | code_obj::CO_NEWLOCALS;
        if fncode.is_generator {
            flags |= code_obj::CO_GENERATOR;
        }
        let filename = self
            .script_filename
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());

        code_obj::CodeBuild {
            name: name.to_string(),
            qualname: qualname.to_string(),
            argcount: 0,
            posonlyargcount: 0,
            kwonlyargcount: 0,
            nlocals,
            stacksize: fncode.num_regs as i64,
            varnames,
            flags,
            filename,
            firstlineno: fncode.first_lineno as i64,
            consts,
            names,
            freevars: Vec::new(),
            cellvars,
        }
        .build()
    }
}
