/// Return the `__doc__` string for a built-in type or exception class by name.
/// Returns `None` if the name is not a known built-in; callers should return
/// `Value::none()` in that case to match CPython's behaviour for unknown types.
pub(crate) fn builtin_class_doc(name: &str) -> Option<&'static str> {
    Some(match name {
        // object
        "object" => {
            "The base class of the class hierarchy.\n\nWhen called, it accepts no arguments and returns a new featureless\ninstance that has no instance attributes and cannot be given any.\n"
        }
        // Primitive types
        "int" => {
            "int([x]) -> integer\nint(x, base=10) -> integer\n\nConvert a number or string to an integer, or return 0 if no arguments\nare given.  If x is a number, return x.__int__().  For floating point\nnumbers, this truncates towards zero.\n\nIf x is not a number or if base is given, then x must be a string,\nbytes, or bytearray instance representing an integer literal in the\ngiven base.  The literal can be preceded by '+' or '-' and be surrounded\nby whitespace.  The base defaults to 10.  Valid bases are 0 and 2-36.\nBase 0 means to interpret the base from the string as an integer literal.\n>>> int('0b100', base=0)\n4"
        }
        "str" => {
            "str(object='') -> str\nstr(bytes_or_buffer[, encoding[, errors]]) -> str\n\nCreate a new string object from the given object. If encoding or\nerrors is specified, then the object must expose a data buffer\nthat will be decoded using the given encoding and error handler.\nOtherwise, returns the result of object.__str__() (if defined)\nor repr(object).\nencoding defaults to sys.getdefaultencoding().\nerrors defaults to 'strict'."
        }
        "list" => {
            "Built-in mutable sequence.\n\nIf no argument is given, the constructor creates a new empty list.\nThe argument must be an iterable if specified."
        }
        "dict" => {
            "dict() -> new empty dictionary\ndict(mapping) -> new dictionary initialized from a mapping object's\n    (key, value) pairs\ndict(iterable) -> new dictionary initialized as if via:\n    d = {}\n    for k, v in iterable:\n        d[k] = v\ndict(**kwargs) -> new dictionary initialized with the name=value pairs\n    in the keyword argument list.  For example:  dict(one=1, two=2)"
        }
        "tuple" => {
            "Built-in immutable sequence.\n\nIf no argument is given, the constructor returns an empty tuple.\nIf iterable is specified the tuple is initialized from iterable's items.\n\nIf the argument is a tuple, the return value is the same object."
        }
        "set" => {
            "set() -> new empty set object\nset(iterable) -> new set object\n\nBuild an unordered collection of unique elements."
        }
        "frozenset" => {
            "frozenset() -> empty frozenset object\nfrozenset(iterable) -> frozenset object\n\nBuild an immutable unordered collection of unique elements."
        }
        "bytes" => {
            "bytes(iterable_of_ints) -> bytes\nbytes(string, encoding[, errors]) -> bytes\nbytes(bytes_or_buffer) -> immutable copy of bytes_or_buffer\nbytes(int) -> bytes object of size given by the parameter initialized with null bytes\nbytes() -> empty bytes object\n\nConstruct an immutable array of bytes from:\n  - an iterable yielding integers in range(256)\n  - a text string encoded using the specified encoding\n  - any object implementing the buffer API.\n  - an integer"
        }
        "float" => "Convert a string or number to a floating point number, if possible.",
        "bool" => {
            "bool(x) -> bool\n\nReturns True when the argument x is true, False otherwise.\nThe builtins True and False are the only two instances of the class bool.\nThe class bool is a subclass of the class int, and cannot be subclassed."
        }
        "complex" => {
            "Create a complex number from a real part and an optional imaginary part.\n\nThis is equivalent to (real + imag*1j) where imag defaults to 0."
        }
        // Exception classes
        "BaseException" => "Common base class for all exceptions",
        "Exception" => "Common base class for all non-exit exceptions.",
        "ArithmeticError" => "Base class for arithmetic errors.",
        "LookupError" => "Base class for lookup errors.",
        "ValueError" => "Inappropriate argument value (of correct type).",
        "TypeError" => "Inappropriate argument type.",
        "NameError" => "Name not found globally.",
        "UnboundLocalError" => "Local name referenced before assignment.",
        "AttributeError" => "Attribute not found.",
        "KeyError" => "Mapping key not found.",
        "IndexError" => "Sequence index out of range.",
        "OverflowError" => "Result too large to be represented.",
        "ZeroDivisionError" => "Second argument to a division or modulo operation was zero.",
        "FloatingPointError" => "Floating point operation failed.",
        "RuntimeError" => "Unspecified run-time error.",
        "RecursionError" => "Recursion limit exceeded.",
        "NotImplementedError" => "Method or function hasn't been implemented yet.",
        "AssertionError" => "Assertion failed.",
        "StopIteration" => "Signal the end from iterator.__next__().",
        "EOFError" => "Read beyond end of file.",
        "MemoryError" => "Out of memory.",
        "ImportError" => "Import can't find module, or can't find name in module.",
        "ModuleNotFoundError" => "Module not found.",
        "UnicodeError" => "Unicode related error.",
        "UnicodeEncodeError" => "Unicode encoding error.",
        "UnicodeDecodeError" => "Unicode decoding error.",
        "UnicodeTranslateError" => "Unicode translation error.",
        "BufferError" => "Buffer error.",
        "ReferenceError" => "Weak ref proxy used after referent went away.",
        "SystemError" => {
            "Internal error in the Python interpreter.\n\nPlease report this to the Python maintainer, along with the traceback,\nthe Python version, and the hardware/OS platform and version."
        }
        "StopAsyncIteration" => "Signal the end from iterator.__anext__().",
        "SyntaxError" => "Invalid syntax.",
        "IndentationError" => "Improper indentation.",
        "TabError" => "Improper mixture of spaces and tabs.",
        "OSError" => "Base class for I/O related errors.",
        "FileNotFoundError" => "File not found.",
        "FileExistsError" => "File already exists.",
        "BlockingIOError" => "I/O operation would block.",
        "ChildProcessError" => "Child process error.",
        "InterruptedError" => "Interrupted by signal.",
        "IsADirectoryError" => "Operation doesn't support directories.",
        "NotADirectoryError" => "Operation only works on directories.",
        "PermissionError" => "Not enough permissions.",
        "ProcessLookupError" => "Process not found.",
        "TimeoutError" => "Timeout expired.",
        "ConnectionError" => "Connection error.",
        "BrokenPipeError" => "Broken pipe.",
        "ConnectionAbortedError" => "Connection aborted.",
        "ConnectionRefusedError" => "Connection refused.",
        "ConnectionResetError" => "Connection reset.",
        "UnsupportedOperation" => "Operation not supported on this file type.",
        "Warning" => "Base class for warning categories.",
        "UserWarning" => "Base class for warnings generated by user code.",
        "DeprecationWarning" => "Base class for warnings about deprecated features.",
        "PendingDeprecationWarning" => {
            "Base class for warnings about features which will be deprecated\nin the future."
        }
        "RuntimeWarning" => "Base class for warnings about dubious runtime behavior.",
        "SyntaxWarning" => "Base class for warnings about dubious syntax.",
        "ResourceWarning" => "Base class for warnings about resource usage.",
        "FutureWarning" => {
            "Base class for warnings about constructs that will change semantically\nin the future."
        }
        "ImportWarning" => "Base class for warnings about probable mistakes in module imports.",
        "UnicodeWarning" => {
            "Base class for warnings about Unicode related problems, mostly\nrelated to conversion problems."
        }
        "BytesWarning" => {
            "Base class for warnings about bytes and buffer related problems, mostly\nrelated to conversion from str or comparing to str."
        }
        "EncodingWarning" => "Base class for warnings about encodings.",
        "SystemExit" => "Request to exit from the interpreter.",
        "GeneratorExit" => "Request that a generator exit.",
        "KeyboardInterrupt" => "Program interrupted by user.",
        "BaseExceptionGroup" => "A combination of multiple unrelated exceptions.",
        "ExceptionGroup" => "A combination of multiple unrelated exceptions.",
        _ => return None,
    })
}

/// Borrowing counterpart of [`key_to_value`].
///
/// Live dict/set walks hold their key order in a snapshot they keep; reading
/// through it avoids copying the whole `PyKey` enum once per yielded item.
pub(crate) fn key_ref_to_value(key: &PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(*v),
        PyKey::Str(v) => v.clone(),
        PyKey::Bool(v) => Value::bool_(*v),
        PyKey::None => Value::none(),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::Float(v) => Value::float_from_bits(*v),
        PyKey::Object { value, .. } => value.clone(),
        _ => key_to_value(key.clone()),
    }
}

pub(crate) fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
        PyKey::Float(v) => Value::float_from_bits(v),
        PyKey::Str(v) => v,
        PyKey::Bool(v) => Value::bool_(v),
        PyKey::None => Value::none(),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::FrozenSet(key) => pyrust_builtins::frozenset::frozenset_key(key),
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Bytes(rc) => Value::bytes((*rc).clone()),
        PyKey::Complex(re, im) => Value::complex(re, im),
        PyKey::Object { value, .. } => value,
    }
}

/// Merge the registers of a single VM frame view into `dict`,
/// dereferencing the view's raw pointer.  Used by
/// [`snapshot_current_locals`].
///
/// SAFETY: `view.regs_ptr` / `view.regs_len` describe a VM frame's
/// register slice.  `Interpreter::vm_frame_views` is pushed
/// immediately before each `run_bytecode_inner` invocation and
/// popped immediately after.  Push/pop sites:
///   * `program_execution::try_exec_vm_script_with_index` — `FrameKind::Script`
///     for the module/script body.
///   * `calls::call_user_function_expanded` — `FrameKind::Function`
///     for both the simple and variadic user-function call paths.
///   * `execution::resume_generator_with_exc` — `FrameKind::Function` for
///     each generator resume (the frame view's regs pointer comes
///     from the heap-allocated `GeneratorFrame::regs`, which is
///     stable across yields).
///
/// Class-body evaluation (`Insn::MakeClass`) publishes a `FrameKind::Class`
/// view so that `locals()` inside a class body returns the partially-built
/// class attrs dict (issue #487).
///
/// The slice is read-only here.
fn merge_frame_view_into_dict(view: &VmFrameView, dict: &mut PyDict) {
    // Stable iteration order: walk the fastlocal slot table sorted by
    // slot index, mirroring the compiler's name-allocation order.
    let mut by_slot: Vec<(usize, &String)> = view
        .local_index
        .iter()
        .map(|(name, &slot)| (slot as usize, name))
        .collect();
    by_slot.sort_by_key(|(slot, _)| *slot);
    for (slot, name) in by_slot {
        if slot >= view.regs_len {
            continue;
        }
        // SAFETY: `view.regs_ptr` is a NonNull pointer to the frame's
        // register file; `slot < view.regs_len` is enforced above.
        //
        // No aliasing UB: as of PR #646, every `run_bytecode*` function
        // accepts `RegSlice` (raw pointer + len) instead of `&mut [Value]`.
        // `RegSlice` carries no LLVM `noalias` attribute, so no exclusive
        // borrow on the allocation is live when this code runs — even in
        // case (c) below where the script frame is the "current" frame.
        //
        // Cases for the frame being read:
        //   (a) A suspended outer frame (e.g. the Script frame when
        //       `locals()` / `globals()` fires from inside a nested
        //       function or class body): the outer frame's `RegSlice`
        //       is on the stack but carries no noalias.  No write races
        //       this read (interpreter is single-threaded).
        //   (b) The current innermost function/class frame — suspended
        //       inside `call_function_expanded` while the builtin runs.
        //       Same reasoning: `RegSlice`, no noalias, no concurrent
        //       writes.
        //   (c) The current Script frame when `locals()` is called at
        //       module scope.  The script frame's dispatch loop holds a
        //       `RegSlice` for the same allocation; forming `&Value` here
        //       does not alias an `&mut [Value]` and is sound.  (This
        //       was the residual UB in the previous `&mut [Value]` design,
        //       now closed by the `RegSlice` change in issue #547.)
        //
        // The `&Value` from `as_ref()` lives only for the duration of the
        // `.clone()` call and does not escape this loop body.
        let val = unsafe { view.regs_ptr.add(slot).as_ref() };
        if !val.is_unset() {
            dict.insert(PyKey::str_from(name), val.clone());
        }
    }
}

/// `(code, own_env, defining_env)` — see [`frame_cell_context`].
type FrameCellContext = (Rc<crate::bytecode::FnCode>, Option<EnvRef>, EnvRef);

/// The code object, own env, and defining env of a `Function` frame view.
///
/// A natively-called or trampolined frame carries its `UserFunction`; a
/// generator resume carries a pointer to the live `GeneratorFrame` instead, and
/// the same three pieces come off that.  `own_env` is the environment holding
/// *this* frame's cells (present only when the callee needed a local env);
/// `defining_env` is the closure's captured environment, where its free
/// variables resolve.
fn frame_cell_context(view: &VmFrameView) -> Option<FrameCellContext> {
    if let Some(function) = &view.function {
        let rc = function.precompiled_code.as_ref()?;
        let code = Rc::clone(rc).downcast::<crate::bytecode::FnCode>().ok()?;
        Some((code, view.env.clone(), Rc::clone(&function.env)))
    } else {
        // SAFETY: `gen_frame` is non-null only while the generator frame is on
        // the call stack — pushed/popped in lock-step with this view by
        // `resume_generator_with_exc` / `vm_enter_gen_drive`.  `locals()` runs
        // synchronously on that stack while the frame is suspended inside a
        // builtin call, so the pointee is alive and no `&mut GeneratorFrame`
        // to it is live (the same reasoning `build_deferred_traceback` uses).
        let gframe = unsafe { view.gen_frame?.as_ref() };
        // A generator's cells live in the env it captured at creation, which is
        // also where its free variables resolve from.
        Some((
            Rc::clone(&gframe.code),
            Some(Rc::clone(&gframe.saved_env)),
            Rc::clone(&gframe.saved_env),
        ))
    }
}

/// Merge a function frame's **cell** and **free** variables into `dict`.
///
/// CPython's `locals()` for a function frame is `co_varnames` + `co_cellvars` +
/// `co_freevars`; the register walk in [`merge_frame_view_into_dict`] covers
/// only the first (issue #3024).  A cell var — a local captured by a nested
/// function — is stored in the frame's own env rather than its register file,
/// so its fastlocal slot stays unset and the register walk skips it; a free var
/// lives in an enclosing function's env entirely.
///
/// Both groups are appended after the registers, each sorted by name: that is
/// the order CPython's `dictbytype` gives `co_cellvars` / `co_freevars`, and it
/// is also the only stable order available here — `FnCode::cell_vars` is
/// collected through a `HashSet`.  Names that are unbound (a cell not yet
/// assigned, or `del`-eted) are omitted, as CPython omits an empty cell.
#[inline]
fn merge_frame_cells_into_dict(view: &VmFrameView, dict: &mut PyDict) {
    // Gate for the common frame, inlined into the caller so a plain function
    // costs no call: one with no cell vars needs no local env, so `env` is
    // `None`, and one defined at module scope has no enclosing function scope
    // to read free variables from.  Such a frame has neither group, so it never
    // touches its code object.
    if view.env.is_none()
        && view.gen_frame.is_none()
        && !matches!(&view.function, Some(f) if f.env.borrow().parent.is_some())
    {
        return;
    }
    merge_frame_cells_into_dict_slow(view, dict);
}

#[inline(never)]
fn merge_frame_cells_into_dict_slow(view: &VmFrameView, dict: &mut PyDict) {
    let Some((code, own_env, defining_env)) = frame_cell_context(view) else {
        return;
    };

    // Cell variables: this frame's own captured locals, read out of the env the
    // callee was given.
    if !code.cell_vars.is_empty()
        && let Some(env) = own_env.as_ref()
    {
        let mut names: smallvec::SmallVec<[&str; 4]> =
            code.cell_vars.iter().map(String::as_str).collect();
        names.sort_unstable();
        let bindings = env.borrow();
        for name in names {
            if let Some(value) = bindings.values.get(name)
                && !value.is_unset()
            {
                dict.insert(PyKey::str_from(name), value.clone());
            }
        }
    }

    // Free variables: the names this frame reads from an enclosing *function*
    // scope, together with the ones it declares `nonlocal` (issue #486).
    // CPython puts both in `co_freevars`, so they form one sorted group — a
    // `nonlocal` name that this body only *writes* compiles to no read at all
    // and so never reaches the candidate set below.
    let mut frees: smallvec::SmallVec<[(&str, Value); 4]> = smallvec::SmallVec::new();
    if let (Some(nonlocal_names), Some(env)) = (&view.nonlocal_names, &view.env) {
        for name in nonlocal_names.iter() {
            // `lookup_name_in_enclosing_local_env` walks the env parent chain
            // from `env` upward to find the first ancestor that declares `name`
            // as a local and holds its value.  Errors here are internal
            // inconsistencies (nonlocal declared but no enclosing binding
            // found); ignore them silently rather than propagating — a missing
            // nonlocal shouldn't crash `locals()`.
            if let Ok(Some(value)) = lookup_name_in_enclosing_local_env(env, name) {
                frees.push((name.as_str(), value));
            }
        }
    }
    // The candidate set is every name the body reads through the env chain; a
    // candidate is a free variable exactly when it resolves to a binding in an
    // enclosing *function* scope, which is the same rule `__closure__` /
    // `co_freevars` apply (issue #2106), so the two always agree.  One pass over
    // the chain probing every candidate at each level, rather than one walk per
    // candidate: a body reads far more globals and builtins (which resolve
    // nowhere) than free variables, and each of those walks the whole chain.
    let candidates = code.free_var_candidates();
    if !candidates.is_empty() {
        // An explicit `global x` targets the module env even when an enclosing
        // function happens to bind `x` too.  A body that declares one always
        // gets a local env carrying the declarations, so an absent env means
        // none exist — and that env is the only thing a generator frame can
        // consult, having no `UserFunction` to ask.
        let declared_global = own_env.map(|env| Rc::clone(&env.borrow().global_names));
        let mut current = Some(defining_env);
        while let Some(env) = current {
            let bindings = env.borrow();
            let parent = bindings.parent.clone();
            // The root namespace is not a function scope: a name bound there is
            // a module global, never a free variable.  A function defined at
            // module scope stops on its first step.
            if parent.is_none() {
                break;
            }
            for name in candidates {
                // The innermost binding wins, and a `nonlocal` name resolved
                // above is already at its own binding scope.
                if frees.iter().any(|(seen, _)| *seen == name.as_str()) {
                    continue;
                }
                let Some(value) = bindings.values.get(name.as_str()) else {
                    continue;
                };
                if value.is_unset() {
                    continue;
                }
                // A name this frame binds itself is a local or a cell, never
                // free.  Checked here rather than up front because both lookups
                // hash the name, and only a resolved candidate reaches them.
                if view.local_index.contains_key(name)
                    || declared_global.as_ref().is_some_and(|g| g.contains(name))
                {
                    continue;
                }
                frees.push((name.as_str(), value.clone()));
            }
            drop(bindings);
            current = parent;
        }
    }
    frees.sort_by(|a, b| a.0.cmp(b.0));
    frees.dedup_by(|a, b| a.0 == b.0);
    for (name, value) in frees {
        dict.insert(PyKey::str_from(name), value);
    }
}

/// Take a snapshot of the innermost VM frame's local namespace
/// (issue #389: backing for `locals()`).  Reads the top of
/// `Interpreter::vm_frame_views` regardless of kind — at module scope
/// the top entry IS the `Script` frame (so `locals()` == `globals()`,
/// matching CPython parity), and inside a function it's the
/// `Function` frame.  Falls back to the current env's `values` map
/// when no frame is published (e.g. evaluating in a non-VM context).
pub(crate) fn snapshot_current_locals(interp: &Interpreter) -> PyDict {
    match interp.vm_frame_views.last() {
        Some(view) => snapshot_view_locals(interp, view),
        None => {
            // No active VM frame: fall back to env.values.
            let mut dict: PyDict = PyDict::default();
            for (k, v) in interp.env.borrow().values.iter() {
                dict.insert(PyKey::str_from(k), v.clone());
            }
            dict
        }
    }
}

/// Take a snapshot of one frame view's local namespace.
///
/// Frame introspection (`sys._getframe(n).f_locals`) needs the same namespace
/// walk `locals()` performs, but for a suspended *outer* frame — reading a
/// caller's register file is exactly the "case (a)" the safety note on
/// [`merge_frame_view_into_dict`] covers (issue #2926).
///
/// `#[inline]` so `locals()` keeps the single-call shape it had before the walk
/// was split out of [`snapshot_current_locals`].
#[inline]
pub(crate) fn snapshot_view_locals(interp: &Interpreter, view: &VmFrameView) -> PyDict {
    let mut dict: PyDict = PyDict::default();
    match view.kind {
        FrameKind::Script => {
            // Module scope: include the module env (built-in
            // classes + already-spilled bindings) so the user sees the
            // same complete view as `globals()`.  The root's ordered
            // materialisation walk covers both the env bindings and the live
            // script registers, so module-scope `locals()` has exactly the
            // key order `globals()` has (issue #2903).
            //
            // The view's own root is authoritative: an outer script frame may
            // belong to a different module than the frame that is running now.
            let me = module_env(view.env.as_ref().unwrap_or(&interp.env));
            let pairs = me.borrow().namespace_materialization_snapshot();
            for (name, value) in pairs {
                dict.insert(PyKey::str_from(&name), value);
            }
        }
        FrameKind::Class => {
            // Class-body scope (issue #487): return the partially-built class
            // attrs dict — i.e. the fastlocal registers of the class body,
            // filtered to names that have been assigned so far.  CPython
            // returns the class namespace dict (which becomes `__dict__`).
            // We do NOT include the module env here, matching CPython:
            // `locals()` inside a class body is the class namespace, not
            // the module globals.
            merge_frame_view_into_dict(view, &mut dict);
        }
        FrameKind::Function => {
            // Function scope: CPython's `co_varnames` + `co_cellvars` +
            // `co_freevars` — the function's own fastlocals, the locals a
            // nested function captured, and the names it reads from (or
            // declares `nonlocal` against) an enclosing scope.
            //
            // The frame view's `local_index` enumerates exactly the
            // names the compiler allocated for THIS function call, so
            // `merge_frame_view_into_dict` covers the fastlocal subset.
            // We deliberately do NOT also walk `interp.env.values`: when
            // the callee did not need its own local env (the
            // `needs_local_env == false` path in
            // `call_user_function_expanded`), `interp.env` points at
            // the function's *defining* env, and walking that would leak
            // enclosing-scope names into the snapshot.
            merge_frame_view_into_dict(view, &mut dict);
            // Cell, free, and `nonlocal` bindings (issues #486 / #3024): none
            // of them live in the register file, so the walk above cannot see
            // them.  Skipped without touching the code object for the common
            // frame that has none.
            merge_frame_cells_into_dict(view, &mut dict);
        }
    }
    dict
}
