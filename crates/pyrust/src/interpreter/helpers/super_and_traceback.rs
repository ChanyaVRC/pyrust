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
pub(crate) fn format_exc_chain_prefix(
    interp: &mut crate::Interpreter,
    exc_val: &Value,
    filename: &std::sync::Arc<str>,
    src: &str,
) -> String {
    // Collect (exc_value, is_cause) pairs from innermost to outermost.
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
        // Exception chaining metadata lives in C-style slot storage, physically
        // independent of a replacement `__dict__`.
        let suppress = borrow
            .attrs
            .get_cloned_or_slot("__suppress_context__")
            .and_then(|v| match v.kind() {
                ValueKind::Bool(b) => Some(b),
                _ => None,
            })
            .unwrap_or(false);

        if suppress {
            // raise X from Y: display __cause__ (if not None)
            let cause = borrow.attrs.get_cloned_or_slot("__cause__");
            drop(borrow);
            match cause {
                Some(c) if !matches!(c.kind(), ValueKind::None) => {
                    // Check the predecessor for cycles before pushing it.
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
            // Implicit chaining: display __context__ (if not None)
            let context = borrow.attrs.get_cloned_or_slot("__context__");
            drop(borrow);
            match context {
                Some(c) if !matches!(c.kind(), ValueKind::None) => {
                    // Check the predecessor for cycles before pushing it.
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

    if chain.is_empty() {
        return String::new();
    }

    // chain is innermost-first; reverse to print oldest first.
    chain.reverse();

    let mut out = String::new();
    for (exc, is_cause) in chain {
        out.push_str(&format_chained_exc_block(interp, &exc, filename, src));
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
    let nodes = pyrust_builtins::traceback::walk_frames_with_col(&tb);
    if nodes.is_empty() {
        return None;
    }
    // Source-line lookup matches the main traceback: store the line with its
    // own leading indentation *preserved* (only trailing whitespace stripped).
    // `format_traceback` dedents it for display and uses the leading-whitespace
    // count to rebase the PEP 657 caret anchor (#2411) — pre-trimming the start
    // here would drop that offset and drop/misplace the caret.
    let resolve = |lineno: Option<u32>| -> Option<std::sync::Arc<str>> {
        let n = lineno?;
        if src.is_empty() {
            return None;
        }
        src.lines()
            .nth((n as usize).saturating_sub(1))
            .map(|l| std::sync::Arc::from(l.trim_end()))
    };
    let frames = nodes
        .into_iter()
        .map(|(funcname, lineno, col_span)| {
            let lineno = if lineno > 0 {
                Some(lineno as u32)
            } else {
                None
            };
            pyrust_core::FrameInfo {
                filename: filename.clone(),
                lineno,
                source_line: resolve(lineno),
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
    // Look up __del__ before the scan so we exit early for objects without it.
    let method = match lookup_class_attr(&del_rc.borrow().class, "__del__") {
        Some(m)
            if matches!(
                m.kind(),
                ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
            ) =>
        {
            m
        }
        _ => return,
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
/// deleted cell.  Check the owning environment and the named registers of its
/// corresponding live frame instead.  A closed-over environment has no live
/// frame, so its remaining cell bindings are the complete owning namespace.
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
    for value in target_env.borrow().values.values() {
        if let Some(other_rc) = value.as_py_instance_rc()
            && Rc::ptr_eq(other_rc, del_rc)
        {
            return;
        }
    }

    for view in &interp.vm_frame_views {
        let Some(view_env) = view.env.as_ref() else {
            continue;
        };
        if !Rc::ptr_eq(view_env, target_env) {
            continue;
        }
        for register in view.local_index.values() {
            let index = *register as usize;
            if index >= view.regs_len {
                continue;
            }
            // SAFETY: a live VmFrameView owns a pointer valid for `regs_len`
            // slots until the view is popped.  This helper only reads the
            // suspended outer frame while the inner call is executing.
            let value = unsafe { &*view.regs_ptr.as_ptr().add(index) };
            if let Some(other_rc) = value.as_py_instance_rc()
                && Rc::ptr_eq(other_rc, del_rc)
            {
                return;
            }
        }
    }

    // The deleting inner frame may itself hold another binding (for example a
    // parameter alias).  Preserve the established current-frame/env scan and
    // the single shared finalizer invocation after the owner-specific checks.
    call_del_if_last_binding(interp, val, regs, num_locals);
}
