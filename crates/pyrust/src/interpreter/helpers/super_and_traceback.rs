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
        .find(|v| v.kind == FrameKind::Function);

    let Some(view) = innermost_fn_frame else {
        // Called at module/script scope — no function frame at all.
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    };

    if !view.is_class_method {
        // Innermost function is not a direct class method (e.g. nested inner
        // function, standalone function).
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    }

    // Verify that __class__ is reachable through the env chain.  It is written
    // by MakeClass into the class env after the class body finishes; the method
    // captured the same Rc<RefCell<Environment>> at MakeFunction time.
    let class_val = lookup_name_in_env(&interp.env, "__class__")?;
    let Some(class_val) = class_val else {
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    };

    // Read register 0 — the first positional parameter (`self` or `cls`).
    if view.regs_len == 0 {
        return Err(PyError::Runtime("super(): no arguments".to_string()));
    }
    // SAFETY: view.regs_ptr is a NonNull<Value> pointing at the frame's
    // register file; the frame is suspended inside call_user_function_expanded
    // (which pushed the VmFrameView) and has not been freed.  The single-
    // element read does not alias any &mut [Value] (PR #646 removed those).
    let first_arg = unsafe { view.regs_ptr.as_ref() };
    if first_arg.is_unset() {
        return Err(PyError::Runtime("super(): no arguments".to_string()));
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

/// Call `__del__` on `val` if it is a `PyInstance` with a `__del__` method
/// AND no other Python-visible binding to the same instance still exists.
///
/// "Python-visible" bindings are:
///   - Named local-variable registers (indices `0..num_locals`).  Compiler
///     temporaries (index `>= num_locals`) are not Python names and must
///     not prevent `__del__` from firing.  The deleted register has already
///     been cleared to `Value::unset()` by the caller, so it naturally
///     produces no match during the scan.
///   - `interp.env.borrow().values`: global / nonlocal / cell-var bindings.
///
/// This deliberately ignores interpreter-internal state (reusable argument
/// buffers, inline caches, etc.) because those are implementation details
/// invisible to Python code, mirroring CPython's refcount semantics where
/// only Python-level references keep an object alive.
///
/// If `__del__` raises an exception, a CPython-format warning is printed to
/// stderr but the exception is not propagated to the caller (issue #1797).
/// Resolve the `__del__` class slot using `PyClass::del_slot_cache` so
/// scope-exit on the overwhelming majority of classes without `__del__` skips
/// the MRO hash walk completely (issue #2987).  When a `__del__` slot *is*
/// present, any slot value (plain function, staticmethod, classmethod,
/// property, cached_property, user descriptor, callable instance, or
/// non-callable) is returned so `invoke_class_method` binds/dispatches it.
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

pub(crate) fn call_del_if_last_binding(
    interp: &mut Interpreter,
    val: Value,
    regs: &RegSlice,
    num_locals: usize,
) {
    // Uninitialised register slots (Value::unset()) are not Python values.
    if val.is_unset() {
        return;
    }
    let del_rc = match val.as_py_instance_rc() {
        Some(rc) => rc,
        None => return,
    };
    // Look up __del__ via the class-level fast gate before the binding scan so
    // classes without __del__ exit immediately with zero MRO walk.
    let Some(method) = resolve_del_slot(&del_rc.borrow().class) else {
        return;
    };
    // Scan named local-variable registers (0..num_locals) for another binding.
    // Registers >= num_locals are compiler temporaries — not Python variables.
    let scan_limit = num_locals.min(regs.len());
    for i in 0..scan_limit {
        // Skip uninitialised register slots — Value::unset() is not a Python value.
        let r = &regs[i];
        if r.is_unset() {
            continue;
        }
        if let Some(other_rc) = r.as_py_instance_rc()
            && Rc::ptr_eq(other_rc, del_rc)
        {
            return; // another named local still holds the instance
        }
    }
    // Scan env.values for a Python-level binding (globals / nonlocals / cells).
    for v in interp.env.borrow().values.values() {
        if let Some(other_rc) = v.as_py_instance_rc()
            && Rc::ptr_eq(other_rc, del_rc)
        {
            return; // a global/nonlocal/cell var still holds the instance
        }
    }
    // No other Python-visible binding — invoke __del__.
    let class_name = del_rc.borrow().class.borrow().name.clone();
    let instance = Value::py_instance(Rc::clone(del_rc));
    drop(val); // release our reference before calling __del__
    // CPython prints a warning to stderr but does not propagate __del__
    // exceptions to the caller (issue #1797).
    if let Err(e) = invoke_class_method(interp, method, instance, &[]) {
        eprintln!("Exception ignored in: <function {}.__del__>", class_name);
        // For a Raised instance, format as "ClassName: msg" (CPython parity)
        // using format_single_exc_line, which calls to_py_str() on the
        // instance — matching CPython's `ValueError: oops` output.
        // For other PyError variants, Display already formats as "ClassName: msg".
        match &e {
            PyError::Raised(v) => eprintln!("{}", format_single_exc_line(interp, v)),
            _ => eprintln!("{}", e),
        }
    }
}

/// Call `__del__` after deleting a binding from an enclosing function cell.
///
/// Unlike [`call_del_if_last_binding`], the active `regs` belong to the inner
/// function executing `nonlocal del`, not to the activation that owns the
/// deleted cell. Check the target environment and every live frame's named
/// registers and environment chain: a caller outside the cell-owning frame may
/// still hold the same instance. A closed-over environment has no live frame,
/// so the target environment must also be checked directly.
pub(crate) fn call_del_if_last_binding_in_env(
    interp: &mut Interpreter,
    val: Value,
    target_env: &EnvRef,
    regs: &RegSlice,
    num_locals: usize,
) {
    if val.is_unset() {
        return;
    }
    let del_rc = match val.as_py_instance_rc() {
        Some(rc) => rc,
        None => return,
    };
    if resolve_del_slot(&del_rc.borrow().class).is_none() {
        return;
    }

    fn observe_direct_binding(
        value: &Value,
        del_rc: &Rc<RefCell<PyInstance>>,
        target_alias_is_visible: &mut bool,
        visible_owner_worklist: &mut Vec<Rc<GeneratorCell>>,
        discovered_owner_ids: &mut HashSet<*const GeneratorCell>,
    ) {
        if value.is_unset() {
            return;
        }
        if value
            .as_py_instance_rc()
            .is_some_and(|other_rc| Rc::ptr_eq(other_rc, del_rc))
        {
            *target_alias_is_visible = true;
        }
        if let ValueKind::Generator(owner) = value.kind()
            && discovered_owner_ids.insert(Rc::as_ptr(owner))
        {
            visible_owner_worklist.push(Rc::clone(owner));
        }
    }

    fn scan_environment_chain(
        root: &EnvRef,
        del_rc: &Rc<RefCell<PyInstance>>,
        target_alias_is_visible: &mut bool,
        visible_owner_worklist: &mut Vec<Rc<GeneratorCell>>,
        discovered_owner_ids: &mut HashSet<*const GeneratorCell>,
        visited_environments: &mut HashSet<*const RefCell<Environment>>,
    ) {
        let mut current = Some(Rc::clone(root));
        while let Some(env) = current {
            if !visited_environments.insert(Rc::as_ptr(&env)) {
                // This environment and its complete parent chain were already
                // visited from another root.
                break;
            }
            let bindings = env.borrow();
            for value in bindings.values.values() {
                observe_direct_binding(
                    value,
                    del_rc,
                    target_alias_is_visible,
                    visible_owner_worklist,
                    discovered_owner_ids,
                );
            }
            current = bindings.parent.clone();
        }
    }

    // Collect the direct Python binding roots once. PyRust can retain a
    // generator Rc in unnamed compiler temporaries after `del generator`; that
    // implementation-only reference must not make its suspended registers
    // visible. Generator/coroutine objects found in direct named bindings form
    // a bounded work queue: each visible suspended frame contributes its own
    // named registers and environment cells, which may reveal another frame.
    // Container/item reachability remains outside this binding-based model.
    let mut target_alias_is_visible = false;
    let mut visible_owner_worklist = Vec::new();
    let mut discovered_owner_ids = HashSet::new();
    let mut visited_environments = HashSet::new();

    scan_environment_chain(
        target_env,
        del_rc,
        &mut target_alias_is_visible,
        &mut visible_owner_worklist,
        &mut discovered_owner_ids,
        &mut visited_environments,
    );
    scan_environment_chain(
        &interp.env,
        del_rc,
        &mut target_alias_is_visible,
        &mut visible_owner_worklist,
        &mut discovered_owner_ids,
        &mut visited_environments,
    );
    for view in &interp.vm_frame_views {
        if let Some(env) = view.env.as_ref() {
            scan_environment_chain(
                env,
                del_rc,
                &mut target_alias_is_visible,
                &mut visible_owner_worklist,
                &mut discovered_owner_ids,
                &mut visited_environments,
            );
        }
        if let Some(gen_frame) = view.gen_frame {
            // SAFETY: this is the same shared-reference reconstruction used by
            // `with_frame_cache`. `VmFrameView::gen_frame` is heap-stable while
            // the live view remains on the stack and is popped before the
            // GeneratorFrame can move or be dropped.
            let gen_frame = unsafe { gen_frame.as_ref() };
            scan_environment_chain(
                &gen_frame.saved_env,
                del_rc,
                &mut target_alias_is_visible,
                &mut visible_owner_worklist,
                &mut discovered_owner_ids,
                &mut visited_environments,
            );
        }
    }

    let scan_limit = num_locals.min(regs.len());
    for value in regs.iter().take(scan_limit) {
        observe_direct_binding(
            value,
            del_rc,
            &mut target_alias_is_visible,
            &mut visible_owner_worklist,
            &mut discovered_owner_ids,
        );
    }
    for view in &interp.vm_frame_views {
        for register in view.local_index.values() {
            let index = *register as usize;
            if index >= view.regs_len {
                continue;
            }
            // SAFETY: a live VmFrameView owns a pointer valid for `regs_len`
            // slots until the view is popped. This helper only reads named
            // registers while the corresponding frame is active.
            observe_direct_binding(
                unsafe { &*view.regs_ptr.as_ptr().add(index) },
                del_rc,
                &mut target_alias_is_visible,
                &mut visible_owner_worklist,
                &mut discovered_owner_ids,
            );
        }
    }

    if target_alias_is_visible {
        return;
    }
    while let Some(owner) = visible_owner_worklist.pop() {
        let Ok(state) = owner.try_borrow() else {
            // Active frames are represented by VmFrameView and were scanned
            // above; the checked-out cell itself has no frame to inspect.
            continue;
        };
        if let Some(asend) = state.downcast_ref::<AsyncGenASend>() {
            if discovered_owner_ids.insert(Rc::as_ptr(&asend.agen)) {
                visible_owner_worklist.push(Rc::clone(&asend.agen));
            }
            continue;
        }
        let Some(frame) = state.downcast_ref::<GeneratorFrame>() else {
            continue;
        };
        if frame.done {
            continue;
        }

        scan_environment_chain(
            &frame.saved_env,
            del_rc,
            &mut target_alias_is_visible,
            &mut visible_owner_worklist,
            &mut discovered_owner_ids,
            &mut visited_environments,
        );
        for register in frame.local_index.values() {
            if let Some(value) = frame.regs.get(*register as usize) {
                observe_direct_binding(
                    value,
                    del_rc,
                    &mut target_alias_is_visible,
                    &mut visible_owner_worklist,
                    &mut discovered_owner_ids,
                );
            }
        }
        if target_alias_is_visible {
            return;
        }
    }

    // The deleting inner frame may itself hold another binding (for example a
    // parameter alias).  Preserve the established current-frame/env scan and
    // the single shared finalizer invocation after the live-frame checks.
    call_del_if_last_binding(interp, val, regs, num_locals);
}
