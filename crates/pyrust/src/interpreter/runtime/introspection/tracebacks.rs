impl Interpreter {
    /// Build the Python-visible traceback chain from captured frame metadata
    /// plus the catching frame.
    fn build_traceback_from_snapshot(
        &self,
        captured: &[pyrust_core::FrameInfo],
        catch: TracebackCatchFrame<'_>,
        tail: Value,
    ) -> Value {
        let catch_code = catch.code.build_code_object(self);
        // Build the chain from innermost (tb_next == None) outward, so the
        // outermost node is returned last and links to the rest via tb_next.
        //
        // `tail` is the already-materialised chain that this snapshot's frames
        // are *prepended* onto (issue #2367): when an exception that already
        // carries a traceback is re-raised, CPython prepends the re-raising
        // frame(s) and links the new innermost node's `tb_next` to the old
        // chain — which stays as the tail, same objects (identity contract).
        // For a fresh catch the tail is `None`.
        let mut node = tail;

        // Innermost-last: walk the captured frames from the END (innermost)
        // toward the front (outermost callee).
        for fi in captured.iter().rev() {
            let lineno = fi.lineno.map(|n| n as i64).unwrap_or(0);
            // Carry the frame's own source filename into `f_code.co_filename`
            // (#2438): a reconstructed traceback frame for an imported module's
            // function must report that module's file, not `<unknown>`.
            let co = code_obj::code_with_loc(
                fi.funcname.to_string(),
                0,
                Vec::new(),
                fi.filename.to_string(),
                0,
            );
            // Each captured frame owns the root namespace in which that frame
            // executed.  In particular, an imported callee must not inherit the
            // globals of the frame that happened to catch its exception.
            //
            // A module-scope node keeps CPython's `tb_frame.f_locals is
            // f_globals` identity (issue #2926).  A function frame has already
            // unwound by the time the traceback materialises, so its register
            // file is gone and its locals stay empty.
            let (globals, locals) = match fi.globals.as_ref() {
                Some(owner) if fi.funcname.as_ref() == "<module>" => (
                    self.globals_for_environment(owner.environment()),
                    self.frame_locals_for_module_environment(owner.environment()),
                ),
                Some(owner) => (
                    self.globals_for_environment(owner.environment()),
                    Value::dict(Default::default()),
                ),
                None => (
                    Value::dict(Default::default()),
                    Value::dict(Default::default()),
                ),
            };
            let frame = frame_obj::frame(co, lineno, Value::none(), globals, locals);
            // Carry the captured frame's PEP 657 caret anchor onto the node so a
            // re-raised / chained exception renders the same carets (#2411).
            node = tb_obj::traceback_node_with_col(frame, node, lineno, -1, fi.col_span);
        }

        // Finally, the catching frame as the outermost node.
        let catch_env = catch.globals.environment();
        let catch_globals = self.globals_for_environment(catch_env);
        let catch_locals = if matches!(catch.code, TracebackCatchCodeSnapshot::Module { .. }) {
            self.frame_locals_for_module_environment(catch_env)
        } else {
            Value::dict(Default::default())
        };
        let catch_frame = frame_obj::frame(
            catch_code,
            catch.lineno,
            Value::none(),
            catch_globals,
            catch_locals,
        );
        tb_obj::traceback_node_with_col(catch_frame, node, catch.lineno, -1, catch.col_span)
    }

    /// Build a *deferred* traceback placeholder for an exception being caught.
    ///
    /// Instead of eagerly materialising the (expensive) `traceback` object
    /// chain — which builds a full `code` object for the catching frame plus
    /// a `frame` and two dicts per node, none of which the overwhelming
    /// majority of `try/except` blocks ever read — this captures only the
    /// cheap snapshot the build needs (the `FrameInfo` list, the catching
    /// frame's `UserFunction` `Rc`, and the catch line) and returns a
    /// lightweight placeholder value.
    ///
    /// The first read of `e.__traceback__` materialises the real chain via
    /// [`Self::materialize_deferred_traceback`] and replaces the placeholder, so
    /// the Python-visible behaviour is identical to the eager build (issue
    /// #2351).
    pub(crate) fn build_deferred_traceback(&self, catch_lineno: i64) -> Value {
        self.build_deferred_traceback_with_tail(catch_lineno, Value::none())
    }

    /// Like [`Self::build_deferred_traceback`] but the materialised chain will be
    /// *prepended* onto `tail` (issue #2367).  `tail` is the existing traceback
    /// carried by a re-raised exception — either a real `traceback` chain or a
    /// still-deferred placeholder; it is materialised at read time so the new
    /// innermost node's `tb_next` is a real node with stable identity, matching
    /// CPython's prepend-and-reuse-tail behaviour.
    pub(crate) fn build_deferred_traceback_with_tail(
        &self,
        catch_lineno: i64,
        tail: Value,
    ) -> Value {
        self.build_deferred_traceback_with_tail_impl(catch_lineno, tail, false)
    }

    /// Like [`Self::build_deferred_traceback_with_tail`] but for a **bare**
    /// `raise` (issue #2405).  A bare re-raise re-raises the active exception
    /// *without* adding a traceback node for the re-raising frame itself —
    /// CPython keeps the carried chain and only prepends the genuinely-outer
    /// frames the exception unwinds through *after* the re-raise.  The
    /// re-raising frame is the innermost of the freshly-captured unwind frames
    /// (`captured.last()`, recorded first as the error propagated out), so it
    /// is dropped here; the remaining outer frames prepend onto `tail`.
    pub(crate) fn build_deferred_traceback_with_tail_drop_innermost(
        &self,
        catch_lineno: i64,
        tail: Value,
    ) -> Value {
        self.build_deferred_traceback_with_tail_impl(catch_lineno, tail, true)
    }

    fn build_deferred_traceback_with_tail_impl(
        &self,
        catch_lineno: i64,
        tail: Value,
        drop_innermost: bool,
    ) -> Value {
        let mut captured = pyrust_core::clone_captured_error_frames();
        // Bare re-raise (#2405): the innermost captured frame is the re-raising
        // frame itself, which CPython does not give its own traceback node.
        if drop_innermost && !captured.is_empty() {
            captured.pop();
        }
        let last_view = self.vm_frame_views.last();
        let catch_func = last_view.and_then(|view| view.function.clone());
        // Issue #2445: a generator body has no `UserFunction`; recover the
        // generator's `(funcname, filename)` so the catching frame is attributed
        // to the generator instead of `<module>`.  Issue #2471: the view stores
        // only a pointer to the live `GeneratorFrame`; read the name/filename
        // lazily here on the cold traceback-build path.
        //
        // SAFETY: `gen_frame` is non-null only while the generator frame is on
        // the call stack (pushed/popped in lock-step with the view in
        // `resume_generator_with_exc` / `vm_enter_gen_drive`).  This builder runs
        // synchronously on that same stack while the frame is suspended, so the
        // pointee is alive and no `&mut GeneratorFrame` to it is live.
        let catch_gen_frame = if catch_func.is_none() {
            last_view.and_then(|view| view.gen_frame.map(|p| unsafe { p.as_ref() }))
        } else {
            None
        };
        let catch_gen_info =
            catch_gen_frame.map(|gframe| (gframe.fn_name.clone(), gframe.code.filename.clone()));
        // Capture the namespace owner now, while the catching frame is live.
        // Imported-module execution uses a child Interpreter that may be dropped
        // before `e.__traceback__` is first read, so consulting `self.env` during
        // later materialisation would both select the wrong namespace and fail
        // to keep a failed import's globals alive.
        let catch_env = catch_func
            .as_ref()
            .map(|func| &func.env)
            .or_else(|| catch_gen_frame.map(|gframe| &gframe.saved_env))
            .or_else(|| last_view.and_then(|view| view.env.as_ref()))
            .unwrap_or(&self.env);
        let catch_globals = pyrust_core::FrameGlobals::for_environment(catch_env);
        let catch_code = match (catch_func, catch_gen_info) {
            (Some(function), _) => TracebackCatchCodeSnapshot::Function(function),
            (None, Some((funcname, filename))) => {
                TracebackCatchCodeSnapshot::Generator { funcname, filename }
            }
            (None, None) => TracebackCatchCodeSnapshot::Module {
                // Materialization may happen through another Interpreter after
                // an imported child has been dropped. Retain this source
                // identity instead of consulting the later materializer.
                filename: self
                    .script_filename
                    .clone()
                    .unwrap_or_else(|| std::sync::Arc::from("<unknown>")),
            },
        };
        let state: Box<dyn std::any::Any> = Box::new(DeferredTracebackState {
            frames: captured,
            catch_code,
            catch_lineno,
            catch_globals,
            // The raising instruction's caret anchor was published just before
            // handler dispatch (vm.rs), so it is current here (#2411).
            catch_col_span: pyrust_core::get_current_vm_col_span(),
            tail,
        });
        Value::builtin_object(DEFERRED_TRACEBACK_OPS, state)
    }

    /// If `value` is a deferred-traceback placeholder, materialise it into a real
    /// traceback object chain; otherwise return `None`.  Used by every read site
    /// that may observe an exception's `__traceback__` slot.
    pub(crate) fn materialize_deferred_traceback(&self, value: &Value) -> Option<Value> {
        let ValueKind::BuiltinObject { ops, state } = value.kind() else {
            return None;
        };
        if !pyrust_core::builtin_ops_is::<DeferredTracebackOps>(ops) {
            return None;
        }
        // Clone out the snapshot fields, then drop the borrow before recursing
        // into the tail (which may itself be a deferred placeholder sharing no
        // lock, but keeping the borrow narrow is cleaner).
        let (frames, catch_code, catch_lineno, catch_col_span, catch_globals, tail) = {
            let borrow = state.borrow();
            let s = borrow.downcast_ref::<DeferredTracebackState>()?;
            (
                s.frames.clone(),
                s.catch_code.clone(),
                s.catch_lineno,
                s.catch_col_span,
                s.catch_globals.clone(),
                s.tail.clone(),
            )
        };
        // Materialise the carried tail first (issue #2367) so the prepended
        // frames link to a real chain with stable identity.  A `None` tail (the
        // common fresh-catch case) materialises to itself.
        let tail = self.materialize_deferred_traceback(&tail).unwrap_or(tail);
        Some(self.build_traceback_from_snapshot(
            &frames,
            TracebackCatchFrame {
                code: &catch_code,
                lineno: catch_lineno,
                col_span: catch_col_span,
                globals: &catch_globals,
            },
            tail,
        ))
    }

    /// When `exc` is being re-raised by an explicit `raise e` /
    /// `raise e.with_traceback(tb)` and already carries a traceback, reset the
    /// captured unwind-frame snapshot (issue #2367).
    ///
    /// CPython keeps the carried traceback chain and *prepends* the frames the
    /// re-raise newly unwinds through.  pyrust models that by treating the
    /// carried chain as the tail at the next catch site and rebuilding the
    /// prefix from frames captured *after* this point — so the stale frames of
    /// the original raise must be cleared here, otherwise they would be counted
    /// twice (once in the carried tail, once in the freshly-rebuilt prefix).
    pub(crate) fn reset_captured_frames_if_reraise(&self, exc: &Value) {
        let ValueKind::PyInstance(inst) = exc.kind() else {
            return;
        };
        // The traceback lives in C-style slot storage, independent of any
        // replacement `__dict__`.
        let has_tb = inst
            .borrow()
            .attrs
            .get_cloned_or_slot("__traceback__")
            .is_some_and(|tb| !tb.is_none());
        if has_tb {
            pyrust_core::reset_captured_error_frames();
            return;
        }
        // Issue #2407: a *fresh* exception (no carried `__traceback__`) raised
        // while another exception is being handled (`handled_exc_stack` is
        // non-empty — the same condition that drives implicit-context chaining
        // in `attach_implicit_context`) must start from an empty unwind-frame
        // snapshot.  Otherwise the stale frames captured while the *handled*
        // exception unwound to this catch site would prepend onto the new
        // exception's traceback, producing a spurious trailing frame (the
        // `f`-frame of the in-flight exception) in both the Python-visible
        // `__traceback__` chain and the uncaught stderr formatter.  The new
        // exception's own unwind frames are recorded *after* this point as it
        // propagates out, so resetting here loses nothing.
        if !self.handled_exc_stack.is_empty() {
            pyrust_core::reset_captured_error_frames();
        }
    }

    /// True when `value` is a deferred-traceback placeholder (not yet
    /// materialised).
    pub(crate) fn is_deferred_traceback(value: &Value) -> bool {
        matches!(
            value.kind(),
            ValueKind::BuiltinObject { ops, .. }
                if pyrust_core::builtin_ops_is::<DeferredTracebackOps>(ops)
        )
    }

    /// Compute the `__traceback__` value to store for an exception being caught
    /// at `catch_lineno`, given whatever the exception's `__traceback__` slot
    /// currently holds (`existing`).
    ///
    /// Three cases (issue #2367):
    ///  * `existing` is `None` — a *fresh* exception; build a deferred chain
    ///    from the captured unwind frames (the hot path).
    ///  * `existing` already represents the chain *this same frame* built (the
    ///    `with`/`__exit__` same-frame identity case from issue #2359/#2366):
    ///    return `None` so the caller keeps the existing object unchanged.
    ///  * `existing` holds a carried/re-raised chain from another frame: build a
    ///    new deferred chain whose materialised frames are *prepended* onto the
    ///    existing chain, so CPython's "prepend the re-raising frame, keep the
    ///    old tail" behaviour is reproduced.
    ///
    /// `is_bare_reraise` is set when the in-flight exception reached this catch
    /// via a *bare* `raise` (issue #2405): bare re-raise keeps the carried chain
    /// and prepends only the genuinely-outer frames the exception unwound
    /// through after the re-raise, never adding a node for the re-raising frame
    /// itself (CPython's `raise` semantics).
    ///
    /// Returns `Some(new_value)` to store, or `None` to leave the slot untouched.
    pub(crate) fn caught_traceback_value(
        &self,
        existing: &Value,
        catch_lineno: i64,
        is_bare_reraise: bool,
    ) -> Option<Value> {
        // Fresh exception (slot still the pre-initialised `None`): plain build.
        if existing.is_none() {
            return Some(self.build_deferred_traceback(catch_lineno));
        }
        // Slot already carries a chain (real traceback or deferred placeholder).
        let is_real = pyrust_builtins::traceback::is_traceback(existing);
        let is_deferred = Self::is_deferred_traceback(existing);
        if !is_real && !is_deferred {
            // Some non-traceback value (shouldn't normally happen, but be safe):
            // overwrite with a fresh build, matching the historical behaviour.
            return Some(self.build_deferred_traceback(catch_lineno));
        }
        // Bare `raise` carrying a chain (issue #2405).  The bare re-raise reset
        // the captured-frame snapshot at the raise site (`reset_captured_frames`
        // in `RaiseReRaise`), so the captured frames now hold *only* the frames
        // the exception unwound through *after* the re-raise.  CPython keeps the
        // carried chain unchanged and prepends those outer frames — never adding
        // a node for the re-raising frame itself.
        //  * Caught in the *same* frame (no unwind): nothing was captured — keep
        //    the carried chain exactly (this also preserves the `with`/`__exit__`
        //    same-frame identity contract from #2359/#2366, which re-raises via
        //    the bare form).
        //  * Propagated to an outer frame: prepend the captured frames minus the
        //    innermost (the re-raising frame's own node) onto the carried chain.
        if is_bare_reraise {
            if pyrust_core::captured_error_frames_len() == 0 {
                return None;
            }
            return Some(self.build_deferred_traceback_with_tail_drop_innermost(
                catch_lineno,
                existing.clone(),
            ));
        }
        // Same-frame identity (issue #2359/#2366): a *materialised* chain whose
        // length matches the frames captured for *this* catch was built in this
        // very frame; keep it so the object an inner `with`/`except` saw is
        // identical to the one this `except` reads.  Re-deferring or prepending
        // would mint a distinct head and break that contract.
        if is_real
            && pyrust_builtins::traceback::chain_len(existing)
                == pyrust_core::captured_error_frames_len() + 1
        {
            return None;
        }
        // Explicit `raise e` / `raise e.with_traceback(...)` carried / re-raised
        // across a frame boundary: prepend the new frames onto the existing
        // chain, keeping the old chain as the tail (issue #2367).
        Some(self.build_deferred_traceback_with_tail(catch_lineno, existing.clone()))
    }

    /// Derive the uncaught-exception traceback's *inner* frame list (everything
    /// below the `<module>` frame) from `exc`'s prepended `__traceback__` chain
    /// (issue #2404).
    ///
    /// The stderr formatter normally rebuilds its frame list from the captured
    /// unwind-frame snapshot, but after #2367/#2403 a re-raised exception's
    /// `__traceback__` chain is the authoritative, Python-visible carried frame
    /// list — the snapshot was reset at the re-raise site and so diverges.  When
    /// `exc` carries a `__traceback__` chain, the printed list is:
    ///
    ///   `snapshot_prefix` (the frames the exception unwound through *after* the
    ///   re-raise, captured fresh) ++ the carried `__traceback__` chain.
    ///
    /// This replays at the top level the prepend a hypothetical module-scope
    /// `except` would have done — the re-raise's finalising catch never runs for
    /// an uncaught exception, so the chain itself lacks the re-raise frame's
    /// node; the snapshot supplies it.  `is_bare` drops the innermost snapshot
    /// frame (the bare re-raise's own line gets no node — #2405), mirroring
    /// `caught_traceback_value`.
    ///
    /// Returns `None` for a raw `PyError` variant or a never-caught exception
    /// (`__traceback__` still `None`), so those keep the snapshot path unchanged.
    ///
    /// `filename` is the script path (the tb nodes carry no per-frame filename —
    /// a single-file program shares one); `src` is the script source for
    /// source-line lookup; `snapshot` is the freshly-captured unwind frames
    /// (outermost-first).  The chain is already ordered outermost-first.
    pub(crate) fn uncaught_inner_frames_from_tb(
        &self,
        exc: &Value,
        filename: &std::sync::Arc<str>,
        src: &str,
        snapshot: &[pyrust_core::FrameInfo],
        is_bare: bool,
    ) -> Option<Vec<pyrust_core::FrameInfo>> {
        let ValueKind::PyInstance(inst) = exc.kind() else {
            return None;
        };
        // The traceback lives in C-style slot storage, independent of any
        // replacement `__dict__`.
        let stored = inst.borrow().attrs.get_cloned_or_slot("__traceback__");
        let stored = stored.filter(|tb| !tb.is_none())?;
        // Materialise the deferred placeholder if needed (cold uncaught path).
        let tb = self
            .materialize_deferred_traceback(&stored)
            .unwrap_or(stored);
        let nodes = pyrust_builtins::traceback::walk_frames_with_col(&tb);
        if nodes.is_empty() {
            return None;
        }
        // Source-line lookup: store the line with its own leading indentation
        // *preserved* (only trailing whitespace stripped).  `format_traceback`
        // dedents it for display and uses the leading-whitespace count to rebase
        // the PEP 657 caret anchor (#2411); pre-trimming the start would drop
        // that offset and drop/misplace the caret.
        let resolve = |lineno: Option<u32>| -> Option<std::sync::Arc<str>> {
            let n = lineno?;
            if src.is_empty() {
                return None;
            }
            src.lines()
                .nth((n as usize).saturating_sub(1))
                .map(|l| std::sync::Arc::from(l.trim_end()))
        };
        let mut frames: Vec<pyrust_core::FrameInfo> =
            Vec::with_capacity(snapshot.len() + nodes.len());
        // Prefix: the frames the exception unwound through after the re-raise.
        // For a bare re-raise the innermost (the re-raise's own frame) gets no
        // node, mirroring the catch-site `drop_innermost`.
        let prefix_end = if is_bare && !snapshot.is_empty() {
            snapshot.len() - 1
        } else {
            snapshot.len()
        };
        for fi in &snapshot[..prefix_end] {
            // The snapshot lacks per-frame source text; resolve it from `src`.
            frames.push(pyrust_core::FrameInfo {
                filename: filename.clone(),
                lineno: fi.lineno,
                source_line: resolve(fi.lineno),
                funcname: fi.funcname.clone(),
                // This list is for stderr formatting only; Python-visible frame
                // objects already live in the materialised traceback.
                globals: None,
                col_span: fi.col_span,
            });
        }
        // The carried chain (outermost-first), each node carrying its PEP 657
        // caret anchor recovered from the original capture (#2411).
        for (funcname, lineno, col_span) in nodes {
            let lineno = if lineno > 0 {
                Some(lineno as u32)
            } else {
                None
            };
            frames.push(pyrust_core::FrameInfo {
                filename: filename.clone(),
                lineno,
                source_line: resolve(lineno),
                funcname: std::sync::Arc::from(&funcname[..]),
                globals: None,
                col_span,
            });
        }
        Some(frames)
    }
}

/// Everything needed to materialise the outermost catching-frame node.
///
/// Keeping this as one semantic input prevents the traceback builder from
/// accumulating an unrelated positional parameter for every new frame detail.
struct TracebackCatchFrame<'a> {
    code: &'a TracebackCatchCodeSnapshot,
    lineno: i64,
    /// PEP 657 caret anchor of the catching frame's raising instruction.
    col_span: Option<(u32, u32, u32, u32)>,
    globals: &'a pyrust_core::FrameGlobals,
}

/// Stable identity needed to rebuild the catching frame's code object.
///
/// Function and generator frames already own their source metadata. A
/// module-scope catch snapshots its filename explicitly because deferred
/// materialization can run through a different Interpreter after import.
#[derive(Clone)]
enum TracebackCatchCodeSnapshot {
    Function(Rc<UserFunction>),
    Generator {
        funcname: std::sync::Arc<str>,
        filename: std::sync::Arc<str>,
    },
    Module {
        filename: std::sync::Arc<str>,
    },
}

impl TracebackCatchCodeSnapshot {
    fn build_code_object(&self, interpreter: &Interpreter) -> Value {
        match self {
            Self::Function(function) => interpreter.build_code_object(function),
            Self::Generator { funcname, filename } => code_obj::code_with_loc(
                funcname.to_string(),
                0,
                Vec::new(),
                filename.to_string(),
                0,
            ),
            Self::Module { filename } => code_obj::code_with_loc(
                "<module>".to_string(),
                0,
                Vec::new(),
                filename.to_string(),
                0,
            ),
        }
    }
}

/// Internal type name for the deferred-traceback placeholder.  Not user-visible
/// (every read path materialises the placeholder before it can be inspected),
/// but distinct from the real `"traceback"` type so the materialisation
/// interceptor can recognise it.
pub(crate) const DEFERRED_TRACEBACK_NAME: &str = "<deferred traceback>";
pub(crate) const DEFERRED_TRACEBACK_OPS: &DeferredTracebackOps = &DeferredTracebackOps;

/// Cheap snapshot carried by a deferred-traceback placeholder until the
/// traceback object is first read (issue #2351).
pub(crate) struct DeferredTracebackState {
    frames: Vec<pyrust_core::FrameInfo>,
    catch_code: TracebackCatchCodeSnapshot,
    catch_lineno: i64,
    /// Strong owner of the catching frame's root namespace.  Tracebacks retain
    /// frame globals even after an imported child Interpreter is dropped.
    catch_globals: pyrust_core::FrameGlobals,
    /// PEP 657 caret anchor of the instruction that raised in the catching frame
    /// (issue #2411), captured at catch time from `get_current_vm_col_span`.
    /// Carried so a same-frame raise (e.g. `1/0` caught then chained) keeps its
    /// fine-grained caret when the deferred chain is materialised.
    catch_col_span: Option<(u32, u32, u32, u32)>,
    /// Existing traceback chain this snapshot's frames are prepended onto when
    /// materialised (issue #2367).  `None` for a fresh catch; a real or deferred
    /// traceback for a re-raised / carried exception.
    tail: Value,
}

pub(crate) struct DeferredTracebackOps;

impl pyrust_core::BuiltinTypeOps for DeferredTracebackOps {
    fn type_name(&self) -> &'static str {
        DEFERRED_TRACEBACK_NAME
    }
}

/// Collect the distinct names referenced via `LoadGlobal` in `fncode` and,
/// recursively, in every nested code object (`fn_protos`).  A free variable
/// read only inside a comprehension/genexpr, `lambda`, or nested `def`
/// compiles its `LoadGlobal` into the nested body, so the enclosing
/// function's free-variable set must include those names too (issue #2106).
/// Names are de-duplicated via `seen`; the caller applies the env-resolution
/// filter that distinguishes a true free variable from a module global or a
/// nested body's own local.
fn collect_loadglobal_names(
    fncode: &crate::bytecode::FnCode,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<String>,
) {
    for insn in &fncode.insns {
        // A free-variable read is `LoadGlobal` for a true global / module-scope
        // capture, or `LoadCell` for a function-scope cell / `nonlocal` (issue
        // #2339).  Both must feed the candidate set so `__closure__` /
        // `co_freevars` still see cell reads now routed through `LoadCell`.
        let name_idx = match insn {
            crate::bytecode::Insn::LoadGlobal(_, idx) | crate::bytecode::Insn::LoadCell(_, idx) => {
                *idx
            }
            _ => continue,
        };
        if let Some(name) = fncode.names.get(name_idx as usize)
            && seen.insert(name.clone())
        {
            out.push(name.clone());
        }
    }
    for proto in &fncode.fn_protos {
        collect_loadglobal_names(&proto.code, seen, out);
    }
}
