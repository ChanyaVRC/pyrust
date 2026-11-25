/// Resolve the Python `dict` backing `frame.f_builtins` from a frame's
/// `f_globals` dict (CPython parity: `f_builtins` is the builtins namespace
/// dict, never `f_globals` itself).
pub(super) fn frame_builtins_for_globals(globals: &Value) -> Value {
    let provider = globals
        .as_dict()
        .and_then(|d| d.get(&PyKey::str_from("__builtins__")).cloned())
        .unwrap_or_else(cached_builtins_module);
    match provider.kind() {
        ValueKind::Dict(_) => provider.clone(),
        ValueKind::PyModule(module) => module.borrow_mut().attach_live_namespace_with_dunders(),
        _ => {
            let builtins_mod = cached_builtins_module();
            if let ValueKind::PyModule(module) = builtins_mod.kind() {
                module.borrow_mut().attach_live_namespace_with_dunders()
            } else {
                Value::dict(PyDict::default())
            }
        }
    }
}

fn clamp_lasti_for_code(code: &Value, lasti: i64) -> i64 {
    if lasti < 0 {
        return lasti;
    }
    let positions_len = code_obj::with_code_state_ref(code, |cs| cs.positions.len()).unwrap_or(0);
    if positions_len == 0 {
        -1
    } else {
        let max_lasti = ((positions_len - 1) as i64) * 2;
        lasti.clamp(0, max_lasti)
    }
}

fn refresh_cached_frame_state(frame: &Value, lineno: i64, lasti: Option<i64>) {
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
    if let Some(offset) = lasti {
        state.lasti = clamp_lasti_for_code(&state.code, offset);
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
    if let Some(gen_frame) = view.gen_frame() {
        // SAFETY: `gen_frame` points to the heap-stable `GeneratorFrame` for
        // exactly as long as this active view is on `vm_frame_views`; both VM
        // entry paths pop the view before the frame can move or be dropped.
        // Reconstruct only a shared reference and access only the cold state's
        // `RefCell`. The borrow is scoped to this non-reentrant callback.
        let gen_frame = unsafe { gen_frame.as_ref() };
        gen_frame.with_frame_cache(read)
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
    if let Some(gen_frame) = interp.vm_frame_views[view_index].gen_frame() {
        // SAFETY: same lifetime guarantee as `with_frame_cache`. This never
        // creates a raw-derived `&mut GeneratorFrame`; mutation is confined to
        // the dedicated `RefCell`, whose short borrow ends before this returns.
        let gen_frame = unsafe { gen_frame.as_ref() };
        gen_frame.with_frame_cache_mut(update)
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

fn is_inlined_comprehension_view(view: &VmFrameView) -> bool {
    if let Some(func) = view.function() {
        if let Some(code_any) = func.precompiled_code.as_ref()
            && let Ok(code) = Rc::clone(code_any).downcast::<crate::bytecode::FnCode>()
            && code.is_inlined_comp
        {
            return true;
        }
        return matches!(
            func.name.as_ref(),
            "<listcomp>" | "<setcomp>" | "<dictcomp>"
        );
    }
    if let Some(gen_frame) = view.gen_frame() {
        // SAFETY: `gen_frame` points to the live boxed `GeneratorFrame` for
        // the lifetime of this active view on `vm_frame_views`.
        let gframe = unsafe { gen_frame.as_ref() };
        return gframe.code.is_inlined_comp
            || matches!(
                gframe.fn_name.as_ref(),
                "<listcomp>" | "<setcomp>" | "<dictcomp>"
            );
    }
    false
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
                traced_frame: None,
                trace_last_line: 0,
                trace_last_pc: 0,
                function_locals,
                persistent_function_locals: None,
                function_local_names,
                live_token: Rc::new(()),
            }));
        }
    });
}

impl Interpreter {
    /// Publish the running source line only for frame-introspection built-ins.
    /// Exact API recognition belongs to this domain rather than the opcode loop
    /// or generic method-call router.
    #[inline]
    pub(crate) fn publish_frame_line_for_builtin(
        callable: &Value,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) {
        if matches!(
            callable.kind(),
            ValueKind::BuiltinFunction(name) if name.ends_with("_getframe") || name == "warnings.warn"
        ) {
            pyrust_core::set_current_vm_line(code.line_at(call_site_pc));
            pyrust_core::set_current_vm_pc(call_site_pc as u32);
        }
    }

    /// Resolve the live instruction byte offset (`f_lasti`) for
    /// `self.vm_frame_views[view_index]`.
    fn resolve_view_lasti(&self, view_index: usize, is_innermost: bool) -> i64 {
        let view = &self.vm_frame_views[view_index];
        let regs_ptr = view.regs_ptr.as_ptr();
        if is_innermost {
            return (pyrust_core::get_current_vm_pc() as i64) * 2;
        }
        if let Some(pc) = self
            .tramp_stack
            .iter()
            .rev()
            .find(|frame| frame.saved_regs_ptr() == regs_ptr)
            .map(|frame| frame.saved_pc().saturating_sub(1))
        {
            return (pc as i64) * 2;
        }
        if let Some(pc) = self
            .gen_drive_stack
            .iter()
            .rev()
            .find(|frame| frame.saved_regs_ptr() == regs_ptr)
            .map(|frame| frame.saved_pc().saturating_sub(1))
        {
            return (pc as i64) * 2;
        }
        0
    }

    /// Return `true` if `frame` is currently executing on `self.vm_frame_views`
    /// (used by `frame.clear()` to raise `RuntimeError("cannot clear an executing frame")`).
    pub(crate) fn is_frame_currently_executing(&self, frame: &Value) -> bool {
        self.vm_frame_views
            .iter()
            .enumerate()
            .any(|(view_index, _)| {
                cached_frame_object(self, view_index)
                    .as_ref()
                    .is_some_and(|cached| values_are_identical(cached, frame))
            })
    }

    /// Resolve the live source line for `self.vm_frame_views[view_index]`.
    fn resolve_view_lineno(
        &self,
        view_index: usize,
        is_innermost: bool,
        fallback_lineno: i64,
    ) -> i64 {
        let view = &self.vm_frame_views[view_index];
        let regs_ptr = view.regs_ptr.as_ptr();
        if is_innermost {
            if fallback_lineno != 0 {
                return fallback_lineno;
            }
            let line = pyrust_core::get_current_vm_line();
            if line != 0 {
                return line as i64;
            }
        } else {
            if let Some(line) = self
                .tramp_stack
                .iter()
                .rev()
                .find(|frame| frame.saved_regs_ptr() == regs_ptr)
                .map(|frame| frame.saved_cur_line())
                .filter(|&line| line != 0)
            {
                return line as i64;
            }
            if let Some(line) = self
                .gen_drive_stack
                .iter()
                .rev()
                .find(|frame| frame.saved_regs_ptr() == regs_ptr)
                .map(|frame| frame.saved_cur_line())
                .filter(|&line| line != 0)
            {
                return line as i64;
            }
            if let Some(line) = self
                .cold
                .call_site_lines
                .iter()
                .rev()
                .find(|(ptr, _)| *ptr == regs_ptr)
                .map(|(_, line)| *line)
                .filter(|&line| line != 0)
            {
                return line as i64;
            }
        }
        match (view.function(), view.gen_frame()) {
            (Some(func), _) => func
                .precompiled_code
                .as_deref()
                .and_then(|any| any.downcast_ref::<crate::bytecode::FnCode>())
                .map(|code| code.first_lineno as i64)
                .unwrap_or(fallback_lineno),
            (None, Some(gen_frame)) => unsafe { gen_frame.as_ref() }.code.first_lineno as i64,
            (None, None) => fallback_lineno,
        }
    }

    /// Resolve caller `(filename, lineno, module_name, globals)` for
    /// `warnings.warn(..., stacklevel=...)` using active frame views and saved
    /// call-site lines. Returns `None` when `stacklevel` walks past the top of
    /// the call stack (so `warnings.warn` falls back to `("sys", 1, "sys")`).
    #[cold]
    #[inline(never)]
    pub(crate) fn resolve_warning_caller(
        &mut self,
        stacklevel: isize,
    ) -> Option<(String, i64, String, Value)> {
        let depth = if stacklevel <= 1 {
            0usize
        } else {
            (stacklevel - 1) as usize
        };
        let view_index = self.visible_view_index_for_depth(depth)?;
        let lineno = self.resolve_view_lineno(view_index, depth == 0, 0);
        let lineno = if lineno != 0 { lineno } else { 1 };
        let view = &self.vm_frame_views[view_index];

        let filename = match (view.function(), view.gen_frame()) {
            (Some(func), _) => func
                .precompiled_code
                .as_deref()
                .and_then(|any| any.downcast_ref::<crate::bytecode::FnCode>())
                .map(|code| code.filename.to_string())
                .unwrap_or_else(|| {
                    self.cold
                        .script_filename
                        .as_ref()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "<string>".to_string())
                }),
            (None, Some(gen_frame)) => unsafe { gen_frame.as_ref() }.code.filename.to_string(),
            (None, None) => {
                if view.kind() == FrameKind::Class
                    && let Some((_, filename)) = class_frame_identity(self, view)
                {
                    filename.to_string()
                } else {
                    self.cold
                        .script_filename
                        .as_ref()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "<string>".to_string())
                }
            }
        };

        let env = view
            .env()
            .or_else(|| {
                view.gen_frame()
                    .map(|gen_frame| &unsafe { gen_frame.as_ref() }.function.env)
            })
            .unwrap_or(&self.env);
        let globals = self.globals_for_environment(env);
        let module = globals
            .as_dict()
            .and_then(|d| d.get(&PyKey::str_from("__name__")).cloned())
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "<string>".to_string());
        Some((filename, lineno, module, globals))
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
        let top_index = self.vm_frame_views.len().checked_sub(1)?;
        // Flush any cached function-locals mapping (from `sys._getframe()`,
        // `locals()`, or a caught traceback) one final time while this
        // activation's register file is still live (issue #3029).
        if self
            .vm_frame_caches
            .as_ref()
            .is_some_and(|caches| caches.len() > top_index && caches[top_index].is_some())
        {
            self.sync_cached_function_locals_at(top_index);
        }
        let view = self.vm_frame_views.pop()?;
        debug_assert!(
            view.gen_frame().is_none(),
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
        let cached = frame.with_frame_cache(|cache| {
            cache
                .and_then(|cache| cache.object.as_ref())
                .and_then(pyrust_core::WeakValueCache::upgrade)
        });
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
            for mut cache in caches.drain(new_len..).flatten() {
                // Drop any activation-owned trace frame pin first so only
                // genuinely user-escaped frame objects survive upgrade.
                cache.traced_frame = None;
                if let Some(frame) = cache
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

    /// Flush a generator activation's cached `f_locals` dict before suspending
    /// or completing, if that generator was ever introspected or caught an
    /// exception (issue #3029). Ordinary un-introspected generators pay only
    /// one `Option::is_some` check.
    #[inline]
    pub(crate) fn sync_top_generator_frame_cache_if_present(&mut self) {
        let Some(view_index) = self.vm_frame_views.len().checked_sub(1) else {
            return;
        };
        let has_cache = self.vm_frame_views[view_index]
            .gen_frame()
            .is_some_and(|gen_frame| unsafe { gen_frame.as_ref() }.has_frame_cache());
        if has_cache {
            self.sync_cached_function_locals_at(view_index);
        }
    }

    /// Translate logical `_getframe` depth (skipping PEP 709 inlined
    /// comprehension views) to an index in `self.vm_frame_views`.
    pub(super) fn visible_view_index_for_depth(&self, depth: usize) -> Option<usize> {
        let mut remaining = depth;
        for (idx, view) in self.vm_frame_views.iter().enumerate().rev() {
            if is_inlined_comprehension_view(view) {
                continue;
            }
            if remaining == 0 {
                return Some(idx);
            }
            remaining -= 1;
        }
        None
    }

    fn enclosing_visible_view_index(&self, view_index: usize) -> Option<usize> {
        self.vm_frame_views[..view_index]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, view)| !is_inlined_comprehension_view(view))
            .map(|(idx, _)| idx)
    }

    fn ensure_live_frame_cache_token(&mut self, view_index: usize) -> std::rc::Weak<()> {
        with_frame_cache_mut(self, view_index, |slot| {
            let cache = slot.get_or_insert_with(|| {
                Box::new(VmFrameCache {
                    object: None,
                    traced_frame: None,
                    trace_last_line: 0,
                    trace_last_pc: 0,
                    function_locals: None,
                    persistent_function_locals: None,
                    function_local_names: Vec::new(),
                    live_token: Rc::new(()),
                })
            });
            Rc::downgrade(&cache.live_token)
        })
    }

    pub(super) fn frame_cache_matches_token(
        &self,
        view_index: usize,
        token: &std::rc::Weak<()>,
    ) -> bool {
        let Some(expected) = token.upgrade() else {
            return false;
        };
        if view_index >= self.vm_frame_views.len() {
            return false;
        }
        with_frame_cache(self, view_index, |cache| {
            cache.is_some_and(|cache| Rc::ptr_eq(&cache.live_token, &expected))
        })
    }

    /// Capture `(locals, live_token)` for a catching frame at catch time so
    /// deferred tracebacks preserve live identity while the frame runs and
    /// preserve final locals if the exception escapes the catching frame
    /// (issue #3029).
    pub(super) fn capture_catching_frame_locals_and_token(
        &mut self,
        view_index: usize,
    ) -> (Option<Value>, std::rc::Weak<()>) {
        match self.vm_frame_views[view_index].kind() {
            FrameKind::Script => (None, self.ensure_live_frame_cache_token(view_index)),
            FrameKind::Class => {
                let locals = class_frame_locals_value(self, &self.vm_frame_views[view_index]);
                let token = self.ensure_live_frame_cache_token(view_index);
                (Some(locals), token)
            }
            FrameKind::Function => {
                if let Some(locals) = cached_function_locals(self, view_index) {
                    self.sync_cached_function_locals_at(view_index);
                    let token = self.ensure_live_frame_cache_token(view_index);
                    return (Some(Value::dict_shared(locals)), token);
                }
                let snapshot = snapshot_view_locals(self, &self.vm_frame_views[view_index]);
                let function_local_names =
                    initial_function_local_names(&self.vm_frame_views[view_index], &snapshot);
                let locals_value = Value::dict(snapshot);
                let weak_locals = Rc::downgrade(
                    locals_value
                        .get_dict_rc()
                        .expect("function frame locals must be a dict"),
                );
                let token = with_frame_cache_mut(self, view_index, |slot| {
                    if let Some(cache) = slot.as_deref_mut() {
                        cache.function_locals = Some(weak_locals);
                        cache.function_local_names = function_local_names;
                        Rc::downgrade(&cache.live_token)
                    } else {
                        let live_token = Rc::new(());
                        let weak_token = Rc::downgrade(&live_token);
                        *slot = Some(Box::new(VmFrameCache {
                            object: None,
                            traced_frame: None,
                            trace_last_line: 0,
                            trace_last_pc: 0,
                            function_locals: Some(weak_locals),
                            persistent_function_locals: None,
                            function_local_names,
                            live_token,
                        }));
                        weak_token
                    }
                });
                (Some(locals_value), token)
            }
        }
    }

    /// Capture `(locals, cached_frame)` for the innermost active frame right
    /// before it unwinds on an exception (issue #3029).
    #[cold]
    #[inline(never)]
    pub(crate) fn capture_unwinding_traceback_frame(
        &mut self,
        lineno: Option<u32>,
    ) -> (Option<Value>, Option<Value>) {
        let Some(view_index) = self.vm_frame_views.len().checked_sub(1) else {
            return (None, None);
        };
        let kind = self.vm_frame_views[view_index].kind();
        if kind == FrameKind::Script {
            return (None, None);
        }
        if kind == FrameKind::Function {
            self.sync_cached_function_locals_at(view_index);
        }
        if let Some(frame) = cached_frame_object(self, view_index) {
            refresh_cached_frame_state(&frame, lineno.map(|n| n as i64).unwrap_or(0), None);
            return (None, Some(frame));
        }
        let locals = match kind {
            FrameKind::Function => {
                if let Some(locals) = cached_function_locals(self, view_index) {
                    Value::dict_shared(locals)
                } else {
                    Value::dict(snapshot_view_locals(self, &self.vm_frame_views[view_index]))
                }
            }
            FrameKind::Class => class_frame_locals_value(self, &self.vm_frame_views[view_index]),
            FrameKind::Script => unreachable!("Script handled above"),
        };
        (Some(locals), None)
    }

    fn sync_cached_function_locals_at(&mut self, view_index: usize) {
        let (cached_locals, compiler_owned_names) = {
            let view = &self.vm_frame_views[view_index];
            if view.kind() != FrameKind::Function {
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
        debug_assert_eq!(self.vm_frame_views[view_index].kind(), FrameKind::Function);

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
                    traced_frame: None,
                    trace_last_line: 0,
                    trace_last_pc: 0,
                    function_locals: Some(weak_locals),
                    persistent_function_locals: Some(locals),
                    function_local_names,
                    live_token: Rc::new(()),
                }));
            }
        });
        locals_value
    }

    /// Return the cached live frame object for `view_index` without allocating
    /// a new one (used by `sys.settrace` line/exception/return hooks).
    pub(crate) fn cached_frame_object_for_trace(&self, view_index: usize) -> Option<Value> {
        with_frame_cache(self, view_index, |cache| {
            cache.and_then(|c| {
                c.traced_frame.clone().or_else(|| {
                    c.object
                        .as_ref()
                        .and_then(pyrust_core::WeakValueCache::upgrade)
                })
            })
        })
    }

    /// Pin `frame` strongly inside `view_index`'s `VmFrameCache` so per-frame
    /// `f_trace` state persists for the duration of the activation.
    pub(crate) fn pin_traced_frame_in_cache(&mut self, view_index: usize, frame: Value) {
        with_frame_cache_mut(self, view_index, |slot| {
            if let Some(cache) = slot.as_deref_mut() {
                cache.traced_frame = Some(frame);
            }
        });
    }

    /// Resolve `(frame, local_trace)` for a `'line'` trace event on
    /// `view_index` when `cur_line` changed or looped back (`pc < trace_last_pc`)
    /// and `f_trace` is non-`None` with `f_trace_lines == true`, updating
    /// `trace_last_line`, `trace_last_pc`, and `frame.f_lineno`.
    pub(crate) fn prepare_line_trace_frame(
        &mut self,
        view_index: usize,
        cur_line: u32,
        pc: usize,
    ) -> Option<(Value, Value)> {
        let mut should_emit = false;
        let frame = with_frame_cache_mut(self, view_index, |slot| {
            let cache = slot.as_deref_mut()?;
            let frame = cache.traced_frame.clone().or_else(|| {
                cache
                    .object
                    .as_ref()
                    .and_then(pyrust_core::WeakValueCache::upgrade)
            })?;
            if cur_line != 0 && (cur_line != cache.trace_last_line || pc < cache.trace_last_pc) {
                cache.trace_last_line = cur_line;
                cache.trace_last_pc = pc;
                should_emit = true;
            } else {
                cache.trace_last_pc = pc;
            }
            Some(frame)
        })?;
        if !should_emit || !frame_obj::get_trace_lines(&frame) {
            return None;
        }
        let local_trace = frame_obj::get_trace(&frame)?;
        if local_trace.is_none() {
            return None;
        }
        frame_obj::set_lineno(&frame, cur_line as i64);
        frame_obj::set_lasti(&frame, (pc as i64) * 2);
        Some((frame, local_trace))
    }

    /// Resolve `(frame, local_trace)` for a `'return'` trace event on
    /// `view_index` when `f_trace` is non-`None`.
    pub(crate) fn active_frame_local_trace(&self, view_index: usize) -> Option<(Value, Value)> {
        let frame = self.cached_frame_object_for_trace(view_index)?;
        let local_trace = frame_obj::get_trace(&frame)?;
        if local_trace.is_none() {
            return None;
        }
        Some((frame, local_trace))
    }

    /// Update `frame.f_trace` (and pin `frame` in `view_index`'s cache when
    /// `pin_view` is `Some` and `trace` is non-`None`).
    pub(crate) fn set_frame_trace_for_vm(
        &mut self,
        frame: &Value,
        pin_view: Option<usize>,
        trace: Value,
    ) {
        let should_pin = !trace.is_none();
        frame_obj::set_trace(frame, trace);
        if should_pin && let Some(view_index) = pin_view {
            self.pin_traced_frame_in_cache(view_index, frame.clone());
        }
    }

    /// Resolve `(frame, local_trace, (exc_type, exc_val, exc_tb))` for an
    /// `'exception'` trace event on `view_index`, or `None` if `view_index` is
    /// not locally traced.
    pub(crate) fn prepare_exception_trace_event(
        &mut self,
        view_index: usize,
        err: &PyError,
        raise_line: u32,
    ) -> Result<Option<(Value, Value, Value)>> {
        let Some((frame, local_trace)) = self.active_frame_local_trace(view_index) else {
            return Ok(None);
        };
        if raise_line != 0 {
            frame_obj::set_lineno(&frame, raise_line as i64);
        }
        let exc_val = self.materialize_pyerror(err.clone())?;
        let exc_type = value_class(&exc_val);
        let existing_tb = self.exception_traceback_value(&exc_val);
        let tb_node = tb_obj::traceback_node(frame.clone(), existing_tb, raise_line as i64, -1);
        let exc_tuple = Value::tuple(vec![exc_type, exc_val, tb_node]);
        Ok(Some((frame, local_trace, exc_tuple)))
    }

    /// When Python code assigns `frame.f_trace = func` on a live frame, pin or
    /// unpin that frame in its `VmFrameCache`.
    pub(crate) fn update_live_frame_trace_pin(&mut self, frame: &Value, pin: bool) {
        let view_index = self
            .vm_frame_views
            .iter()
            .enumerate()
            .rposition(|(view_index, _)| {
                cached_frame_object(self, view_index)
                    .as_ref()
                    .is_some_and(|cached| values_are_identical(cached, frame))
            });
        if let Some(view_index) = view_index {
            with_frame_cache_mut(self, view_index, |slot| {
                if let Some(cache) = slot.as_deref_mut() {
                    cache.traced_frame = pin.then(|| frame.clone());
                }
            });
        }
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

    /// Resolve `f_back` on demand for any live frame object (issues #2993,
    /// #3028, #3029).
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
        if self.vm_frame_views[view_index].gen_frame().is_some() {
            return Some(
                self.enclosing_visible_view_index(view_index)
                    .map(|parent_idx| self.build_frame_object_at_view(parent_idx, 0))
                    .unwrap_or_else(Value::none),
            );
        }
        if let Some(stored) = frame_obj::stored_back(frame)
            && !stored.is_none()
        {
            return Some(stored);
        }
        let back = self
            .enclosing_visible_view_index(view_index)
            .map(|parent_idx| self.build_frame_object_at_view(parent_idx, 0))
            .unwrap_or_else(Value::none);
        frame_obj::set_back(frame, back.clone());
        Some(back)
    }

    /// Build or reuse the single frame object at `idx` (zero is the innermost
    /// visible frame, skipping PEP 709 inlined comprehensions).  `f_back` is
    /// initialized to `None` and resolved lazily on attribute access (or on
    /// unwind if the frame escapes), keeping `sys._getframe()` O(1) regardless
    /// of stack depth (issue #3028).
    pub(crate) fn build_frame_object(&mut self, idx: usize, lineno: i64) -> Value {
        let Some(view_index) = self.visible_view_index_for_depth(idx) else {
            return Value::none();
        };
        let effective_lineno = if idx == 0 {
            lineno
        } else {
            self.resolve_view_lineno(view_index, false, lineno)
        };
        self.build_frame_object_at_view(view_index, effective_lineno)
    }

    /// Build or reuse the Python frame object for the concrete `VmFrameView`
    /// at `view_index`.
    pub(super) fn build_frame_object_at_view(&mut self, view_index: usize, lineno: i64) -> Value {
        // A live activation owns one Python frame identity. Upgrade the weak
        // value only on introspection; an ordinary call allocates no side
        // storage. A successful upgrade reconstructs a Value that shares the
        // existing frame state.
        let is_innermost = self.visible_view_index_for_depth(0) == Some(view_index);
        let lasti = self.resolve_view_lasti(view_index, is_innermost);
        if let Some(frame) = cached_frame_object(self, view_index) {
            refresh_cached_frame_state(&frame, lineno, Some(lasti));
            return frame;
        }

        // Initialize `f_back` lazily to `None`; `live_frame_back_for_attribute`
        // materializes the immediate parent on demand and
        // `truncate_vm_frame_caches` links genuinely escaping frames on unwind.
        let back = Value::none();
        let view = &self.vm_frame_views[view_index];

        let (code, effective_lineno) = match (view.function(), view.gen_frame()) {
            (Some(func), _) => (self.build_code_object(func), lineno),
            (None, Some(gen_frame)) => {
                // Running generator / coroutine / async-generator frame
                // (including `yield from` / `await` delegation frames,
                // issues #2980, #3029): reuse the originating function's
                // shared code object.
                // SAFETY: `gen_frame` points to the heap-stable
                // `GeneratorFrame` for the full lifetime of this active
                // `VmFrameView`.
                let gen_frame = unsafe { gen_frame.as_ref() };
                let line = if lineno != 0 {
                    lineno
                } else {
                    gen_frame.code.first_lineno as i64
                };
                (self.build_code_object(&gen_frame.function), line)
            }
            (None, None) => {
                let regs_ptr = view.regs_ptr.as_ptr();
                let saved_code_ptr = self
                    .tramp_stack
                    .iter()
                    .rev()
                    .find(|frame| frame.saved_regs_ptr() == regs_ptr)
                    .map(|frame| frame.saved_code_ptr())
                    .or_else(|| {
                        self.gen_drive_stack
                            .iter()
                            .rev()
                            .find(|frame| frame.saved_regs_ptr() == regs_ptr)
                            .map(|frame| frame.saved_code_ptr())
                    });
                let (name, filename) = if view.kind() == FrameKind::Class
                    && let Some((class_name, filename)) = class_frame_identity(self, view)
                {
                    (class_name.to_string(), filename.to_string())
                } else {
                    // Script frame: synthesise a code object whose co_name
                    // matches CPython (`<module>` for module scope) and carry
                    // the running script's path into `co_filename` (#2438).
                    let filename = self
                        .cold
                        .script_filename
                        .as_ref()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    ("<module>".to_string(), filename)
                };
                if let Some(code_ptr) = saved_code_ptr {
                    // SAFETY: `saved_code_ptr` points at the suspended caller's
                    // live `FnCode` for the full lifetime of the frame on
                    // `tramp_stack` / `gen_drive_stack`.
                    let fncode = unsafe { &*code_ptr };
                    let firstlineno = fncode.first_lineno as i64;
                    let (lines, positions) = build_code_lines_and_positions(fncode, firstlineno);
                    let linetable = code_obj::encode_linetable(&lines, firstlineno);
                    let co_filename = if !fncode.filename.is_empty() {
                        fncode.filename.to_string()
                    } else {
                        filename
                    };
                    let co = code_obj::CodeBuild {
                        qualname: name.clone(),
                        name,
                        argcount: 0,
                        posonlyargcount: 0,
                        kwonlyargcount: 0,
                        nlocals: 0,
                        stacksize: fncode.num_regs as i64,
                        varnames: Vec::new(),
                        flags: 0,
                        filename: co_filename,
                        firstlineno,
                        consts: fncode.consts.clone(),
                        names: fncode
                            .names
                            .iter()
                            .map(|n| Value::string(n.as_str()))
                            .collect(),
                        freevars: Vec::new(),
                        cellvars: Vec::new(),
                        code_bytes: Vec::new(),
                        linetable,
                        exceptiontable: Vec::new(),
                        lines,
                        positions,
                        raw_code: None,
                        exec_meta: None,
                    }
                    .build();
                    (co, lineno)
                } else {
                    (
                        code_obj::code_with_loc(name, 0, Vec::new(), filename, 0),
                        lineno,
                    )
                }
            }
        };

        let env = view
            .env()
            .or_else(|| {
                view.gen_frame()
                    .map(|gen_frame| &unsafe { gen_frame.as_ref() }.function.env)
            })
            .unwrap_or(&self.env);
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
        let frame_kind = view.kind();
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
        let builtins = frame_builtins_for_globals(&globals);
        let lasti = clamp_lasti_for_code(&code, lasti);
        let frame = frame_obj::frame(
            code,
            effective_lineno,
            back,
            globals,
            locals,
            builtins,
            lasti,
        );
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

    /// Return the live frame object for a currently-running generator,
    /// coroutine, or async generator by matching its stable heap frame address
    /// against active `vm_frame_views` without borrowing its checked-out state
    /// cell (issue #2980).  Shares the `VmFrameCache` on `GeneratorFrame` so
    /// `g.gi_frame is sys._getframe()` holds inside the running body.
    pub(crate) fn build_running_generator_frame_object(
        &mut self,
        cell: &Rc<pyrust_core::GeneratorCell>,
    ) -> Option<Value> {
        let target = cell.frame_ptr()?;
        let view_index = self.vm_frame_views.iter().rposition(|view| {
            view.gen_frame()
                .is_some_and(|frame| frame.as_ptr().cast::<()>() == target.as_ptr())
        })?;
        let lineno = if view_index + 1 == self.vm_frame_views.len() {
            pyrust_core::get_current_vm_line() as i64
        } else {
            0
        };
        Some(self.build_frame_object_at_view(view_index, lineno))
    }

    /// Build the `gi_frame`, `cr_frame`, or `ag_frame` object for a suspended
    /// generator-backed activation, or `Value::none()` when it is exhausted.
    /// The frame's `f_lineno` is the source line the generator is suspended on
    /// (the line of the `yield` it last paused at); `f_code` is the shared
    /// code object of the originating function carrying its immutable
    /// compile-time `co_name` / `co_qualname` / `co_firstlineno` / `co_consts`
    /// (issues #2185, #2980, #2981).
    pub(crate) fn build_generator_frame_object(&self, frame: &GeneratorFrame) -> Value {
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
            let line = frame.code.line_at(frame.pc - 1);
            if line != 0 {
                line as i64
            } else {
                frame.code.first_lineno as i64
            }
        };

        let (cached_frame, retained_locals) = frame.with_frame_cache(|cache| {
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
        });
        let gen_lasti = if frame.pc == 0 {
            -1
        } else {
            ((frame.pc - 1) as i64) * 2
        };
        if let Some(cached_frame) = cached_frame {
            refresh_cached_frame_state(&cached_frame, lineno, Some(gen_lasti));
            return cached_frame;
        }

        let code = self.build_code_object(&frame.function);
        let locals = retained_locals.unwrap_or_default();
        let globals = self.globals_for_environment(&frame.function.env);
        let builtins = frame_builtins_for_globals(&globals);
        let frame_object = frame_obj::frame(
            code,
            lineno,
            Value::none(),
            globals,
            Value::dict_shared(Rc::clone(&locals)),
            builtins,
            gen_lasti,
        );
        let object = pyrust_core::WeakValueCache::new(&frame_object)
            .expect("frame BuiltinObject must support weak caching");
        let function_locals = Rc::downgrade(&locals);
        frame.with_frame_cache_mut(|cache| {
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
                    traced_frame: None,
                    trace_last_line: 0,
                    trace_last_pc: 0,
                    function_locals: Some(function_locals),
                    persistent_function_locals: None,
                    function_local_names: Vec::new(),
                    live_token: Rc::new(()),
                }));
            }
        });
        frame_object
    }
}
