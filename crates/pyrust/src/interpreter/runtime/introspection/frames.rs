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

    /// Pop one non-generator frame view and discard any materialized cache for
    /// that activation. Generator views use direct `Vec::pop` on every hot
    /// resume/exit path and own no side-vector slot.
    #[inline]
    pub(crate) fn pop_vm_frame_view(&mut self) -> Option<VmFrameView> {
        let view = self.vm_frame_views.pop()?;
        debug_assert!(
            view.gen_frame.is_none(),
            "generator frame views must use the direct hot-path pop"
        );
        let new_len = self.vm_frame_views.len();
        if self
            .vm_frame_caches
            .as_ref()
            .is_some_and(|caches| caches.len() > new_len)
        {
            self.truncate_vm_frame_caches(new_len);
        }
        Some(view)
    }

    #[cold]
    #[inline(never)]
    fn truncate_vm_frame_caches(&mut self, new_len: usize) {
        let remove_vector = {
            let caches = self
                .vm_frame_caches
                .as_deref_mut()
                .expect("cache vector checked by pop_vm_frame_view");
            caches.truncate(new_len);
            while caches.last().is_some_and(Option::is_none) {
                caches.pop();
            }
            caches.is_empty()
        };
        if remove_vector {
            self.vm_frame_caches = None;
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

    /// Return a live generator frame's current caller when Python reads
    /// `f_back`. Generator frame objects retain `None` in their backing state,
    /// so suspended/finished frames naturally fall back to `None` without any
    /// resume/pop mutation.
    pub(crate) fn live_generator_frame_back_for_attribute(
        &mut self,
        frame: &Value,
    ) -> Option<Value> {
        let view_index = self
            .vm_frame_views
            .iter()
            .enumerate()
            .rposition(|(view_index, view)| {
                view.gen_frame.is_some()
                    && cached_frame_object(self, view_index)
                        .as_ref()
                        .is_some_and(|cached| values_are_identical(cached, frame))
            });
        let view_index = view_index?;
        let depth = self.vm_frame_views.len() - 1 - view_index;
        Some(self.build_frame_object(depth + 1, 0))
    }

    /// Build or reuse the frame object at `idx` (zero is the innermost frame),
    /// including its `f_back` chain.
    pub(crate) fn build_frame_object(&mut self, idx: usize, lineno: i64) -> Value {
        let len = self.vm_frame_views.len();
        if idx >= len {
            return Value::none();
        }
        // `idx` counts from the top (innermost).  Translate to the Vec index.
        let view_index = len - 1 - idx;

        // A live activation owns one Python frame identity. Upgrade the weak
        // value only on introspection; an ordinary call allocates no side
        // storage. A successful upgrade reconstructs a Value that shares the
        // existing frame state.
        if let Some(frame) = cached_frame_object(self, view_index) {
            refresh_cached_frame_state(&frame, lineno);
            return frame;
        }

        // Materialize outer frames through the same per-view caches so a
        // direct depth lookup and this frame's `f_back` share identity. A
        // generator's caller changes between resumes, so its stored back is
        // always `None` and the active caller is supplied by the getter above.
        let back = if self.vm_frame_views[view_index].gen_frame.is_some() {
            Value::none()
        } else {
            self.build_frame_object(idx + 1, 0)
        };
        let view = &self.vm_frame_views[view_index];

        let code = match &view.function {
            Some(func) => self.build_code_object(func),
            None => {
                // Script / Class frame: synthesise a minimal code object whose
                // co_name matches CPython (`<module>` for module scope).  Carry
                // the running script's path into `co_filename` (#2438) so
                // `sys._getframe().f_code.co_filename` reports the source file
                // rather than `<unknown>`.
                let filename = self
                    .script_filename
                    .as_ref()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                code_obj::code_with_loc("<module>".to_string(), 0, Vec::new(), filename, 0)
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
