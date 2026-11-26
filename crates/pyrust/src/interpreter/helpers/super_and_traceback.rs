/// Resolve zero-argument `super()` as CPython 3.12 does.
///
/// CPython synthesises a `__class__` cell variable for every function compiled
/// directly inside a class body and implicitly passes `__class__` and the first
/// positional argument (typically `self` or `cls`) to zero-arg `super()`.
///
/// pyrust mirrors this by:
/// - Writing `__class__` into the class env after the class body runs
///   (`Insn::MakeClass` in vm.rs).  Methods capture the same env at
///   `MakeFunction` time, so `__class__` is always reachable through the
///   method's env chain.
/// - `FnCode::is_class_method` — set to `true` only for functions compiled
///   directly inside a class body (`compile_def` when `self.is_class_body`).
///   Nested functions inside methods get `false`.
///
/// Returns `(class_value, self_or_cls_value)` on success.
///
/// Returns `Err(RuntimeError("super(): no arguments"))` when:
/// - Called at module/script scope (no Function frame on the stack).
/// - Called from a plain function or a nested function inside a method
///   (innermost Function frame has `is_class_method == false`).
/// - `__class__` is not reachable in the env chain (should not happen for
///   valid class methods — defensive guard).
/// - Register 0 (first positional arg) is unset.
pub(crate) fn resolve_zero_arg_super(interp: &Interpreter) -> crate::error::Result<(Value, Value)> {
    // The INNERMOST Function frame must itself be a direct class method.
    // CPython's zero-arg super() uses a magic `__class__` cell that is only
    // synthesised for functions compiled directly inside a class body; nested
    // functions (`def inner(): super()` inside a method) do not get that cell
    // and therefore zero-arg super() must raise RuntimeError.
    //
    // We find the innermost Function frame (the one currently executing) and
    // check `is_class_method`.  If it is false we immediately fail — even if
    // some outer frame IS a class method.
    let innermost_fn_frame = interp
        .vm_frame_views
        .iter()
        .rev()
        .find(|v| v.kind() == FrameKind::Function);

    let Some(view) = innermost_fn_frame else {
        // Called at module/script scope — no function frame at all.
        return Err(PyError::runtime("super(): no arguments".to_string()));
    };

    if !view.is_class_method() {
        // Innermost function is not a direct class method (e.g. nested inner
        // function, standalone function).
        return Err(PyError::runtime("super(): no arguments".to_string()));
    }

    // Verify that __class__ is reachable through the env chain.  It is written
    // by MakeClass into the class env after the class body finishes; the method
    // captured the same Rc<RefCell<Environment>> at MakeFunction time.
    let class_val = lookup_name_in_env(&interp.env, "__class__")?;
    let Some(class_val) = class_val else {
        return Err(PyError::runtime("super(): no arguments".to_string()));
    };

    // Read register 0 — the first positional parameter (`self` or `cls`).
    if view.regs_len == 0 {
        return Err(PyError::runtime("super(): no arguments".to_string()));
    }
    // SAFETY: view.regs_ptr is a NonNull<Value> pointing at the frame's
    // register file; the frame is suspended inside call_user_function_expanded
    // (which pushed the VmFrameView) and has not been freed.  The single-
    // element read does not alias any &mut [Value] (PR #646 removed those).
    let first_arg = unsafe { view.regs_ptr.as_ref() };
    if first_arg.is_unset() {
        return Err(PyError::runtime("super(): no arguments".to_string()));
    }

    Ok((class_val, first_arg.clone()))
}

/// Format the exception chain that precedes `exc_val` as a prefix string to
/// prepend to the main traceback output.
///
/// Walks `__cause__` (when `__suppress_context__ == True`) or `__context__`
/// (when `__suppress_context__` is falsy) and collects the chain from innermost
/// (closest to `exc_val`) to outermost.  The chain is then reversed and printed
/// oldest-first, matching CPython's display order.
///
/// Each chained exception is rendered as its own full traceback block — a
/// `Traceback (most recent call last):` header, the `File ...` frames derived
/// from that exception's `__traceback__` chain (issues #2408/#2412), and its
/// `ClassName: msg` line — followed by the connecting banner.  When the chained
/// exception has no `__traceback__` (constructed but never raised), CPython
/// omits the traceback block and prints just the `ClassName: msg` line; we do
/// the same.
///
/// Returns an empty string when there is no visible chain (no `__cause__` /
/// `__context__`, or the chain is suppressed via `raise X from None`).
///
/// `filename`/`src` feed the per-frame `File ...`/source-line rendering; they
/// mirror the values the main traceback uses (a single-file program shares one
/// filename, the tb nodes carry no per-frame filename).
/// Return `true` when `value` is a `BaseExceptionGroup` instance (PEP 654).
pub(crate) fn is_exception_group_value(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::PyInstance(inst)
            if pyrust_core::class_chain_contains_builtin_exception(
                &inst.borrow().class,
                "BaseExceptionGroup",
            )
    )
}

/// Extract the `.exceptions` tuple from a `BaseExceptionGroup` instance.
fn exception_group_children(value: &Value) -> Vec<Value> {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return Vec::new();
    };
    inst.borrow()
        .attrs
        .get_slot("exceptions")
        .and_then(|v| v.as_tuple().map(|t| t.to_vec()))
        .unwrap_or_default()
}

/// Format an exception's closing line plus any PEP 678 `__notes__`.
fn format_single_exc_line_with_notes(interp: &mut crate::Interpreter, value: &Value) -> String {
    let mut line = format_single_exc_line(interp, value);
    if let ValueKind::PyInstance(inst) = value.kind()
        && let Some(notes) = inst.borrow().attrs.get_cloned("__notes__")
    {
        let items = if let Some(tuple) = notes.as_tuple() {
            Some(tuple.to_vec())
        } else {
            notes.as_list().map(|list| list.to_vec())
        };
        if let Some(items) = items {
            for note in &items {
                line.push('\n');
                line.push_str(&note.to_py_str());
            }
        } else {
            line.push('\n');
            line.push_str(&notes.repr_raw());
        }
    }
    line
}

/// Render a `BaseExceptionGroup` tree in CPython 3.12's PEP 654 box-drawing
/// format (issue #2993), starting at `depth` (`0` = top-level uncaught group).
///
/// `frames_override` supplies the already-resolved frame list for the outermost
/// uncaught group (`Some(frames)`); nested groups and chained groups derive
/// their frames from their own `__traceback__` slot (`None`).
pub(crate) fn format_exception_group_traceback(
    interp: &mut crate::Interpreter,
    exc: &Value,
    frames_override: Option<&[pyrust_core::FrameInfo]>,
    filename: &std::sync::Arc<str>,
    src: &str,
) -> String {
    let mut out = String::new();
    format_exception_group_at_depth(interp, exc, frames_override, filename, src, 0, &mut out);
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

fn format_exception_group_at_depth(
    interp: &mut crate::Interpreter,
    exc: &Value,
    frames_override: Option<&[pyrust_core::FrameInfo]>,
    filename: &std::sync::Arc<str>,
    src: &str,
    depth: usize,
    out: &mut String,
) {
    use std::fmt::Write as _;
    let bar_indent = "  ".repeat(depth + 1);
    let line_prefix = format!("{bar_indent}| ");
    let owned_frames: Vec<pyrust_core::FrameInfo>;
    let frames: &[pyrust_core::FrameInfo] = match frames_override {
        Some(f) => f,
        None => {
            owned_frames = chained_exc_frames(interp, exc, filename, src).unwrap_or_default();
            &owned_frames
        }
    };
    let exc_line = format_single_exc_line_with_notes(interp, exc);

    if !frames.is_empty() {
        let plain = pyrust_core::format_traceback(frames, &exc_line);
        let mut lines = plain.split('\n');
        if let Some(first) = lines.next() {
            let header = first.replacen(
                "Traceback (most recent call last):",
                "Exception Group Traceback (most recent call last):",
                1,
            );
            if depth == 0 {
                let _ = writeln!(out, "{bar_indent}+ {header}");
            } else {
                let _ = writeln!(out, "{line_prefix}{header}");
            }
        }
        for line in lines {
            let _ = writeln!(out, "{line_prefix}{line}");
        }
    } else {
        for line in exc_line.split('\n') {
            let _ = writeln!(out, "{line_prefix}{line}");
        }
    }

    let children = exception_group_children(exc);
    let child_bar_indent = "  ".repeat(depth + 2);
    let child_prefix = format!("{child_bar_indent}| ");
    for (idx, child) in children.iter().enumerate() {
        let num = idx + 1;
        if idx == 0 {
            let _ = writeln!(
                out,
                "{bar_indent}+-+---------------- {num} ----------------"
            );
        } else {
            let _ = writeln!(
                out,
                "{child_bar_indent}+---------------- {num} ----------------"
            );
        }

        // Format any __cause__ / __context__ chain belonging to `child` first.
        format_exc_chain_prefix_at_depth(interp, child, filename, src, depth + 1, out);

        if is_exception_group_value(child) {
            format_exception_group_at_depth(interp, child, None, filename, src, depth + 1, out);
        } else {
            let block = format_chained_exc_block(interp, child, filename, src);
            for line in block.split('\n') {
                let _ = writeln!(out, "{child_prefix}{line}");
            }
            if idx + 1 == children.len() {
                let _ = writeln!(
                    out,
                    "{child_bar_indent}+------------------------------------"
                );
            }
        }
    }
}

/// Like `format_exc_chain_prefix`, but emits into `out` indented for an
/// ExceptionGroup sub-exception at `depth` (`depth >= 1`).
fn format_exc_chain_prefix_at_depth(
    interp: &mut crate::Interpreter,
    exc_val: &Value,
    filename: &std::sync::Arc<str>,
    src: &str,
    depth: usize,
    out: &mut String,
) {
    use std::fmt::Write as _;
    let chain = collect_exc_chain(exc_val);
    if chain.is_empty() {
        return;
    }
    let line_prefix = format!("{}| ", "  ".repeat(depth + 1));
    for (exc, is_cause) in chain {
        if is_exception_group_value(&exc) {
            format_exception_group_at_depth(interp, &exc, None, filename, src, depth, out);
        } else {
            let block = format_chained_exc_block(interp, &exc, filename, src);
            for line in block.split('\n') {
                let _ = writeln!(out, "{line_prefix}{line}");
            }
        }
        let _ = writeln!(out, "{line_prefix}");
        if is_cause {
            let _ = writeln!(
                out,
                "{line_prefix}The above exception was the direct cause of the following exception:"
            );
        } else {
            let _ = writeln!(
                out,
                "{line_prefix}During handling of the above exception, another exception occurred:"
            );
        }
        let _ = writeln!(out, "{line_prefix}");
    }
}

/// Collect `(exc_value, is_cause)` pairs from oldest to innermost.
fn collect_exc_chain(exc_val: &Value) -> Vec<(Value, bool)> {
    let mut chain: Vec<(Value, bool)> = Vec::new();
    let mut seen: HashSet<*const ()> = HashSet::new();

    let mut current = exc_val.clone();
    loop {
        let ValueKind::PyInstance(inst) = current.kind() else {
            break;
        };
        let raw_ptr = Rc::as_ptr(inst) as *const ();
        if !seen.insert(raw_ptr) {
            break; // cycle guard
        }
        let borrow = inst.borrow();
        let suppress = borrow
            .attrs
            .get_cloned_or_slot("__suppress_context__")
            .and_then(|v| match v.kind() {
                ValueKind::Bool(b) => Some(b),
                _ => None,
            })
            .unwrap_or(false);

        if suppress {
            let cause = borrow.attrs.get_cloned_or_slot("__cause__");
            drop(borrow);
            match cause {
                Some(c) if !matches!(c.kind(), ValueKind::None) => {
                    if let ValueKind::PyInstance(next_inst) = c.kind()
                        && seen.contains(&(Rc::as_ptr(next_inst) as *const ()))
                    {
                        break;
                    }
                    chain.push((c.clone(), true));
                    current = c;
                }
                _ => break,
            }
        } else {
            let context = borrow.attrs.get_cloned_or_slot("__context__");
            drop(borrow);
            match context {
                Some(c) if !matches!(c.kind(), ValueKind::None) => {
                    if let ValueKind::PyInstance(next_inst) = c.kind()
                        && seen.contains(&(Rc::as_ptr(next_inst) as *const ()))
                    {
                        break;
                    }
                    chain.push((c.clone(), false));
                    current = c;
                }
                _ => break,
            }
        }
    }
    chain.reverse();
    chain
}

pub(crate) fn format_exc_chain_prefix(
    interp: &mut crate::Interpreter,
    exc_val: &Value,
    filename: &std::sync::Arc<str>,
    src: &str,
) -> String {
    let chain = collect_exc_chain(exc_val);
    if chain.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for (exc, is_cause) in chain {
        if is_exception_group_value(&exc) {
            out.push_str(&format_exception_group_traceback(
                interp, &exc, None, filename, src,
            ));
        } else {
            out.push_str(&format_chained_exc_block(interp, &exc, filename, src));
        }
        out.push('\n');
        out.push('\n');
        if is_cause {
            out.push_str("The above exception was the direct cause of the following exception:\n");
        } else {
            out.push_str("During handling of the above exception, another exception occurred:\n");
        }
        out.push('\n');
    }
    out
}

/// Render a chained exception as its own full traceback block: a
/// `Traceback (most recent call last):` header, the `File ...` frames derived
/// from the exception's `__traceback__` chain, and the closing `ClassName: msg`
/// line.  When the exception carries no `__traceback__` (it was constructed but
/// never raised), CPython prints just the `ClassName: msg` line with no
/// traceback header — we do the same by delegating to `format_single_exc_line`.
///
/// Frames are derived through the same `walk_frames` / deferred-materialisation
/// path as the main uncaught traceback (`uncaught_inner_frames_from_tb`); a
/// chained exception's stored `__traceback__` is the authoritative, Python-
/// visible frame list (verified to match CPython via `__context__`/`__cause__`
/// introspection).  Unlike the main block, a chained block has no synthetic
/// `<module>` frame prepended — its `<module>` node, when present, is already
/// part of its own `__traceback__` chain.
fn format_chained_exc_block(
    interp: &mut crate::Interpreter,
    exc: &Value,
    filename: &std::sync::Arc<str>,
    src: &str,
) -> String {
    let exc_line = format_single_exc_line(interp, exc);
    let frames = chained_exc_frames(interp, exc, filename, src);
    match frames {
        Some(frames) if !frames.is_empty() => pyrust_core::format_traceback(&frames, &exc_line),
        // No traceback: constructed-but-never-raised exception.  CPython omits
        // the traceback header and prints only the `ClassName: msg` line.
        _ => exc_line,
    }
}

/// Derive a chained exception's frame list from its `__traceback__` chain,
/// materialising the deferred placeholder (cold path) and walking the tb nodes.
/// Returns `None` when the exception has no `__traceback__` (never raised) or
/// the chain walks to an empty node list.
fn chained_exc_frames(
    interp: &mut crate::Interpreter,
    exc: &Value,
    filename: &std::sync::Arc<str>,
    src: &str,
) -> Option<Vec<pyrust_core::FrameInfo>> {
    let ValueKind::PyInstance(inst) = exc.kind() else {
        return None;
    };
    // The traceback lives in C-style slot storage, independent of `__dict__`.
    let stored = inst.borrow().attrs.get_cloned_or_slot("__traceback__");
    let stored = stored.filter(|tb| !tb.is_none())?;
    // Materialise the deferred placeholder if needed (cold uncaught path).
    let tb = interp
        .materialize_deferred_traceback(&stored)
        .unwrap_or(stored);
    let nodes = pyrust_builtins::traceback::walk_frames_with_loc(&tb);
    if nodes.is_empty() {
        return None;
    }
    let frames = nodes
        .into_iter()
        .map(|(node_filename, funcname, lineno, col_span)| {
            let lineno = if lineno > 0 {
                Some(lineno as u32)
            } else {
                None
            };
            let source_line = crate::interpreter::resolve_frame_source_line(
                &node_filename,
                lineno,
                filename.as_ref(),
                src,
            );
            pyrust_core::FrameInfo {
                filename: std::sync::Arc::from(node_filename),
                lineno,
                source_line,
                funcname: std::sync::Arc::from(&funcname[..]),
                // Reconstructed only for formatted chained-exception output.
                globals: None,
                // PEP 657 caret anchor recovered from the chained exception's
                // traceback chain (#2411).
                col_span,
                locals: None,
                frame: None,
            }
        })
        .collect();
    Some(frames)
}

/// Format a single exception value as `"ClassName: msg"` (or just `"ClassName"`
/// when the message is empty).  Used by `format_exc_chain_prefix`.
fn format_single_exc_line(interp: &mut crate::Interpreter, value: &Value) -> String {
    match value.kind() {
        ValueKind::PyInstance(inst) => {
            let class_name = inst.borrow().class.borrow().name.clone();
            // Dispatch arg __repr__/__str__ overrides like CPython's
            // traceback printer (issue #2390 review); a raising dunder
            // falls back to the data-only renderer.
            let inst_rc = Rc::clone(inst);
            let cls = Rc::clone(&inst_rc.borrow().class);
            let msg =
                crate::interpreter::exception_str_with_dispatch(interp, value, &inst_rc, &cls)
                    .unwrap_or_else(|_| value.to_py_str());
            if msg.is_empty() {
                class_name
            } else {
                format!("{class_name}: {msg}")
            }
        }
        _ => format!("Uncaught exception: {}", value.repr_raw()),
    }
}

/// Resolve the `__del__` class slot using `PyClass::del_slot_cache` so the
/// overwhelming majority of classes without `__del__` skip the MRO hash walk
/// completely (issue #2987).  When a `__del__` slot *is* present, any slot
/// value (plain function, staticmethod, classmethod, property,
/// cached_property, user descriptor, callable instance, or non-callable) is
/// returned so `invoke_class_method` binds/dispatches it.
#[inline]
fn resolve_del_slot(class: &Rc<RefCell<PyClass>>) -> Option<Value> {
    {
        let borrowed = class.borrow();
        let version = borrowed.mutation_version.get();
        if let Some((cached_version, cached_epoch, has_del)) = borrowed.del_slot_cache.get()
            && pyrust_core::class_cache_stamp_matches(version, cached_version, cached_epoch)
            && !has_del
        {
            return None;
        }
    }
    resolve_del_slot_slow(class)
}

#[cold]
#[inline(never)]
fn resolve_del_slot_slow(class: &Rc<RefCell<PyClass>>) -> Option<Value> {
    let method = lookup_class_attr(class, "__del__");
    let has_del = method.is_some();
    let borrowed = class.borrow();
    if let Some((version, epoch)) = pyrust_core::class_cache_stamp(borrowed.mutation_version.get())
    {
        borrowed.del_slot_cache.set(Some((version, epoch, has_del)));
    }
    method
}

/// Release a binding removed by `del`, then run any `__del__` that became due.
///
/// Dropping `val` may free the last reference to an instance whose class
/// defines `__del__`; the core drop hook queues such instances instead of
/// freeing them (issue #3341), and the queue is run here so the finalizer
/// fires before the statement after `del`, as in CPython.
#[inline]
pub(crate) fn drop_deleted_binding(interp: &mut Interpreter, val: Value) {
    drop(val);
    if pyrust_core::has_pending_finalizers() {
        interp.run_pending_finalizers();
    }
}

impl Interpreter {
    /// Note that a class gained a `__del__` attribute: from now on dying
    /// instances are offered to the finalizer queue. Instances of the class
    /// may already exist (`Cls.__del__ = f`), uncounted and possibly held by
    /// a dead temporary, so the per-step safepoint runs at least once.
    #[cold]
    pub(crate) fn arm_finalizers(&mut self) {
        pyrust_core::arm_finalizers();
        self.sync_vm_hooks_flags();
        self.start_finalizer_polling();
    }

    /// Switch the per-step `__del__` safepoint on. When it was off, dead
    /// temporaries were not being released, so it starts by releasing the
    /// current frame's.
    fn start_finalizer_polling(&mut self) {
        let polling = crate::interpreter::execution::VM_HOOK_FINALIZER;
        if self.vm_hooks_flags & polling == 0 {
            self.cold.finalizer_scrubbed_regs = 0;
            self.vm_hooks_flags |= polling;
        }
    }

    /// A class without a cached construction plan is being instantiated: when
    /// it defines `__del__`, count the instance and start polling the
    /// finalizer queue before every instruction, so its `__del__` runs as
    /// soon as the last reference goes away.
    #[cold]
    #[inline(never)]
    pub(crate) fn note_possible_finalizable_instance(&mut self, class: &Rc<RefCell<PyClass>>) {
        if pyrust_core::finalizers_armed() && resolve_del_slot(class).is_some() {
            pyrust_core::note_finalizable_instance();
            self.start_finalizer_polling();
        }
    }

    /// Call `__del__` on every instance queued by the core drop hooks.
    ///
    /// Runs until the queue is empty: a finalizer may drop further instances.
    /// Each queued instance is already marked finalized, so a finalizer that
    /// resurrects its instance is not called again (PEP 442).
    #[cold]
    #[inline(never)]
    pub(crate) fn run_pending_finalizers(&mut self) {
        while pyrust_core::has_pending_finalizers() {
            for instance in pyrust_core::take_pending_finalizers() {
                self.run_finalizer(instance);
            }
        }
    }

    /// Interpreter shutdown: finalize what is still alive, as CPython does when
    /// it tears down `__main__`.
    ///
    /// The globals of `__main__` are rebound to `None` in CPython's
    /// `_PyModule_ClearDict` order (names with a single leading underscore
    /// first, then the rest, each in definition order) and the finalizers that
    /// become due run after each one. Nothing happens, and no global is
    /// touched, unless some class defined `__del__`. Afterwards finalizers are
    /// switched off, so objects freed during process teardown stay silent.
    pub fn run_exit_finalizers(&mut self) {
        if !pyrust_core::finalizers_armed() {
            return;
        }
        self.run_pending_finalizers();
        if let Ok(main_module) = self.load_module("__main__") {
            let names = self.module_global_names(&main_module);
            for private_pass in [true, false] {
                for name in &names {
                    let is_dunder = name.starts_with("__");
                    let is_private = name.starts_with('_') && !is_dunder;
                    if is_dunder || is_private != private_pass {
                        continue;
                    }
                    let _ = self.assign_attr(main_module.clone(), name, Value::none());
                    self.run_pending_finalizers();
                }
            }
        }
        self.run_pending_finalizers();
        pyrust_core::disarm_finalizers();
        self.sync_vm_hooks_flags();
    }

    /// Names bound in `module`'s namespace, in definition order.
    fn module_global_names(&mut self, module: &Value) -> Vec<String> {
        let Ok(namespace) = self.get_attr(module, "__dict__") else {
            return Vec::new();
        };
        let arg = [ExpandedCallArg {
            name: None,
            value: namespace,
        }];
        let Ok(keys) = self.call_function_expanded(Value::builtin_function("list"), &arg) else {
            return Vec::new();
        };
        let Some(keys) = keys.as_list() else {
            return Vec::new();
        };
        keys.iter()
            .filter_map(|key| key.as_str().map(str::to_string))
            .collect()
    }

    /// Call `instance.__del__()`, reporting an exception through
    /// `sys.unraisablehook` instead of propagating it (CPython's
    /// `slot_tp_finalize`).
    fn run_finalizer(&mut self, instance: Value) {
        let Some(class) = instance
            .as_py_instance_rc()
            .map(|rc| Rc::clone(&rc.borrow().class))
        else {
            return;
        };
        let Some(method) = resolve_del_slot(&class) else {
            return;
        };
        // A finalizer runs between two instructions of unrelated code, and
        // possibly while an exception is unwinding: keep that code's traceback
        // state out of the finalizer's reach, and restore it afterwards.
        let outer_frames = pyrust_core::take_captured_error_frames();
        let outer_line = pyrust_core::get_current_vm_line();
        let outer_pc = pyrust_core::get_current_vm_pc();
        let outer_span = pyrust_core::get_current_vm_col_span();
        if let Err(err) = invoke_class_method(self, method.clone(), instance, &[]) {
            let frames = pyrust_core::take_captured_error_frames().unwrap_or_default();
            self.report_unraisable(err, &frames, None, method);
        }
        pyrust_core::reset_captured_error_frames();
        for frame in outer_frames.into_iter().flatten().rev() {
            pyrust_core::record_traceback_frame(frame);
        }
        pyrust_core::set_current_vm_line(outer_line);
        pyrust_core::set_current_vm_pc(outer_pc);
        pyrust_core::set_current_vm_col_span(outer_span);
    }

    /// Report an exception that cannot be raised to any caller, as CPython's
    /// `PyErr_WriteUnraisable` does: build the `sys.unraisablehook` argument
    /// and call the hook. `frames` are the traceback frames of `err`.
    pub(crate) fn report_unraisable(
        &mut self,
        err: PyError,
        frames: &[pyrust_core::FrameInfo],
        err_msg: Option<&str>,
        object: Value,
    ) {
        let error_line = self.format_error_line(&err);
        let Ok(exc_value) = self.materialize_pyerror(err) else {
            return;
        };
        let exc_type = match exc_value.as_py_instance_rc() {
            Some(rc) => Value::py_class(Rc::clone(&rc.borrow().class)),
            None => Value::none(),
        };
        let exc_traceback = match self.get_attr(&exc_value, "__traceback__") {
            Ok(traceback) if !traceback.is_none() => traceback,
            _ if frames.is_empty() => Value::none(),
            _ => self.build_traceback_nodes_from_frames(frames, Value::none()),
        };
        let mut attrs = pyrust_core::InstanceAttrs::new();
        attrs.insert("exc_type", exc_type);
        attrs.insert("exc_value", exc_value);
        attrs.insert("exc_traceback", exc_traceback);
        attrs.insert("err_msg", err_msg.map_or_else(Value::none, Value::string));
        attrs.insert("object", object.clone());
        let class = PyClass {
            name: "UnraisableHookArgs".to_string(),
            qualname: "UnraisableHookArgs".to_string(),
            ..Default::default()
        };
        let unraisable = Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::new(RefCell::new(class)),
            attrs,
        })));

        let hook = self
            .load_module("sys")
            .and_then(|sys_mod| self.get_attr(&sys_mod, "unraisablehook"));
        if let Ok(hook) = hook
            && !matches!(hook.kind(), ValueKind::BuiltinFunction(_))
        {
            let arg = [ExpandedCallArg {
                name: None,
                value: unraisable,
            }];
            if self.call_function_expanded(hook, &arg).is_ok() {
                return;
            }
            // A failing custom hook must not hide the original report.
            let _ = pyrust_core::take_captured_error_frames();
        }

        let object_repr = crate::interpreter::render_value_repr(self, &object)
            .unwrap_or_else(|_| object.repr_raw());
        let header = match err_msg {
            Some(msg) => format!("{msg}: {object_repr}"),
            None => format!("Exception ignored in: {object_repr}"),
        };
        let body = if frames.is_empty() {
            format!("{error_line}\n")
        } else {
            let mut text = pyrust_core::format_traceback(frames, &error_line);
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text
        };
        let message = format!("{header}\n{body}");
        let wrote = if let Ok(sys_mod) = self.load_module("sys")
            && let Ok(stream) = self.get_attr(&sys_mod, "stderr")
            && !stream.is_none()
            && let Ok(write_fn) = self.get_attr(&stream, "write")
        {
            let arg = [ExpandedCallArg {
                name: None,
                value: Value::string(message.as_str()),
            }];
            self.call_function_expanded(write_fn, &arg).is_ok()
        } else {
            false
        };
        if !wrote {
            eprint!("{message}");
        }
    }
}
