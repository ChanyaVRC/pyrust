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
            // Function scope: the function's own fastlocals plus any
            // nonlocal bindings.  Matches CPython — `locals()` inside a
            // function includes `nonlocal` names as they live in an
            // enclosing scope but are part of this function's logical
            // local namespace (issue #486).
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
            // Nonlocal bindings: look up each name through the env chain
            // starting from the function's own local env.  Fast-path:
            // skip entirely when the function has no nonlocal names (the
            // common case, zero overhead).
            if let (Some(nonlocal_names), Some(env)) = (&view.nonlocal_names, &view.env) {
                for name in nonlocal_names.iter() {
                    // `lookup_name_in_enclosing_local_env` walks the env
                    // parent chain from `env` upward to find the first
                    // ancestor that declares `name` as a local and holds
                    // its value.  Errors here are internal inconsistencies
                    // (nonlocal declared but no enclosing binding found);
                    // ignore them silently rather than propagating — a
                    // missing nonlocal shouldn't crash `locals()`.
                    if let Ok(Some(val)) = lookup_name_in_enclosing_local_env(env, name) {
                        dict.insert(PyKey::str_from(name), val);
                    }
                }
            }
        }
    }
    dict
}
