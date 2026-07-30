/// Return the errno-specific OSError subclass `Rc` for a given errno value,
/// mirroring CPython 3.12's `_Py_errnomap` table in `Objects/exceptions.c`.
/// Returns `None` when the errno has no mapped subclass (plain `OSError` is
/// used in that case).  Only called when the constructor class is exactly
/// `OSError`; subclasses are never remapped.
fn oserror_subclass_for_errno(errno: i64) -> Option<Rc<RefCell<PyClass>>> {
    // CPython's _Py_errnomap (Linux errno values):
    //   1  EPERM        → PermissionError
    //   2  ENOENT       → FileNotFoundError
    //   3  ESRCH        → ProcessLookupError
    //   4  EINTR        → InterruptedError
    //  10  ECHILD       → ChildProcessError
    //  11  EAGAIN       → BlockingIOError
    //  13  EACCES       → PermissionError
    //  17  EEXIST       → FileExistsError
    //  20  ENOTDIR      → NotADirectoryError
    //  21  EISDIR       → IsADirectoryError
    //  32  EPIPE        → BrokenPipeError
    // 103  ECONNABORTED → ConnectionAbortedError
    // 104  ECONNRESET   → ConnectionResetError
    // 108  ESHUTDOWN    → BrokenPipeError
    // 110  ETIMEDOUT    → TimeoutError
    // 111  ECONNREFUSED → ConnectionRefusedError
    // 114  EALREADY     → BlockingIOError
    // 115  EINPROGRESS  → BlockingIOError
    let subclass_name = match errno {
        1 | 13 => "PermissionError",
        2 => "FileNotFoundError",
        3 => "ProcessLookupError",
        4 => "InterruptedError",
        10 => "ChildProcessError",
        11 | 114 | 115 => "BlockingIOError",
        17 => "FileExistsError",
        20 => "NotADirectoryError",
        21 => "IsADirectoryError",
        32 | 108 => "BrokenPipeError",
        103 => "ConnectionAbortedError",
        104 => "ConnectionResetError",
        110 => "TimeoutError",
        111 => "ConnectionRefusedError",
        _ => return None,
    };
    EXC_CLASS_CACHE.with(|cache| {
        cache
            .iter()
            .find(|(name, _)| *name == subclass_name)
            .map(|(_, cls)| Rc::clone(cls))
    })
}

/// Read the integer value CPython uses for exact `OSError` errno remapping.
///
/// `PyLong_Check` accepts bool and int subclasses, but this lookup must not run
/// an arbitrary `__index__` method. PyRust stores an int subclass's immutable
/// base value in `__builtin_data__`, so verify its canonical ancestry before
/// reading that backing and leave every other object untouched.
fn oserror_remap_errno(value: &Value) -> Option<i64> {
    let integer = match value.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => value.clone(),
        ValueKind::PyInstance(instance) => {
            let class = Rc::clone(&instance.borrow().class);
            let int_class = canonical_class_by_tag(pyrust_core::CanonicalClassTag::Int);
            if !class_is_subclass_of(&class, &int_class) {
                return None;
            }
            builtin_data_backing(value)?
        }
        _ => return None,
    };
    value_to_bigint(&integer).and_then(|integer| integer.to_i64())
}

pub(crate) fn instantiate_exception(class: Rc<RefCell<PyClass>>, args: Vec<Value>) -> Value {
    // Classify the class against the special built-in exception families in
    // one non-cloning MRO walk (issue #1967). StopIteration is matched against
    // its canonical singleton during that same walk, preserving real subclass
    // handling without granting `.value` to an unrelated same-named class.
    let kinds = classify_exception_class(&class);
    instantiate_exception_with_kinds(class, args, &kinds)
}

/// Like [`instantiate_exception`] but takes a pre-computed [`ExcClassKinds`].
/// `construct_exception_instance` already classifies the class once to handle
/// keyword args / argument validation; threading that result through here
/// avoids a second redundant MRO classification walk per constructed exception.
pub(crate) fn instantiate_exception_with_kinds(
    class: Rc<RefCell<PyClass>>,
    args: Vec<Value>,
    kinds: &ExcClassKinds,
) -> Value {
    // The common case (a plain built-in exception) sets exactly two attributes
    // — `args` and `__traceback__` — so reserve for them up front to avoid the
    // Vec growth realloc that would otherwise happen on the second insert.
    let mut attrs = InstanceAttrs::with_slot_capacity(2);
    // CPython 3.12: StopIteration.__init__ sets self.value = args[0] if args else None.
    let is_stop_iteration = kinds.stop_iteration;
    let is_syntax_error = kinds.syntax_error;
    // OSError is the canonical name; IOError and EnvironmentError are aliases that
    // share the same Rc, so checking for "OSError" in the chain suffices.
    let is_os_error = kinds.os_error;
    let is_system_exit = kinds.system_exit;
    // Decode wins over encode wins over translate, matching the original
    // short-circuiting precedence.
    let is_unicode_decode_error = kinds.unicode_decode_error;
    let is_unicode_encode_error = !is_unicode_decode_error && kinds.unicode_encode_error;
    let is_unicode_translate_error =
        !is_unicode_decode_error && !is_unicode_encode_error && kinds.unicode_translate_error;
    // CPython 3.12: NameError (and its subclass UnboundLocalError) have a `.name`
    // attribute.  User-constructed instances (`NameError('msg')`) have `name = None`.
    // Interpreter-raised instances set the name via `instantiate_name_error` instead.
    let is_name_error = kinds.name_error;
    // CPython 3.12: ImportError (and its subclass ModuleNotFoundError) have `.name`
    // and `.path` attributes.  User-constructed instances (`ImportError('msg')`) have
    // both set to `None`.  Interpreter-raised instances set them via
    // `instantiate_import_error` instead.
    let is_import_error = kinds.import_error;
    // CPython 3.12: AttributeError has `.name` and `.obj` attributes.
    // User-constructed instances (`AttributeError('msg')`) have both set to `None`.
    // Interpreter-raised instances set them via `instantiate_attribute_error` instead.
    let is_attribute_error = kinds.attribute_error;
    // PEP 654 (Python 3.11+): BaseExceptionGroup / ExceptionGroup.
    // Both have `.message` (str) and `.exceptions` (tuple of exceptions).
    let is_base_exception_group = kinds.base_exception_group;
    // Pass `&str` keys (not `String`): `insert` interns the key into a shared
    // `Rc<str>`, so a temporary `String` per key per raise would be allocated
    // only to be dropped immediately.
    attrs.insert_slot("args", Value::tuple(args.clone()));
    // CPython 3.12: every BaseException instance has __traceback__ initialised
    // to None at __new__ time.  The VM's handle_vm_error overwrites it with a
    // real traceback object once the exception propagates through a frame.
    attrs.insert_slot("__traceback__", Value::none());
    if is_stop_iteration {
        let val = args.first().cloned().unwrap_or_else(Value::none);
        attrs.insert_slot("value", val);
    } else if is_system_exit {
        // CPython 3.12 SystemExit.__init__: code = args[0] if 1 arg, tuple(args) if
        // multiple args, None if no args.  For the multi-arg case CPython sets
        // self.code = self.args (the same object), so clone the already-inserted
        // args tuple to share the same obj_id and preserve `e.code is e.args`.
        let code = match args.len() {
            0 => Value::none(),
            1 => args[0].clone(),
            _ => attrs.get_slot("args").cloned().unwrap_or_else(Value::none),
        };
        attrs.insert_slot("code", code);
    } else if is_syntax_error {
        // CPython 3.12 SyntaxError.__init__: always initialise all structured
        // attributes.  With 1 arg: msg = args[0], rest = None.  With 2 args
        // where args[1] is a sequence of at least 4 elements, unpack:
        //   (filename, lineno, offset, text[, end_lineno, end_offset])
        // CPython accepts any sequence (tuple OR list) for args[1]; it iterates
        // and unpacks it.  Callers are responsible for raising TypeError when
        // args[1] is a non-sequence or has the wrong number of elements
        // (call_class_expanded validates before reaching here).
        let msg = args.first().cloned().unwrap_or_else(Value::none);
        let mut filename = Value::none();
        let mut lineno = Value::none();
        let mut offset = Value::none();
        let mut text = Value::none();
        let mut end_lineno = Value::none();
        let mut end_offset = Value::none();
        if args.len() >= 2 {
            // Accept both tuple and list — CPython's SyntaxError.__init__ iterates
            // the second argument regardless of its concrete type.
            let items_opt: Option<Vec<Value>> = args[1]
                .as_tuple()
                .map(|s| s.to_vec())
                .or_else(|| args[1].as_list().map(|s| s.to_vec()));
            if let Some(items) = items_opt {
                if items.len() >= 4 {
                    filename = items[0].clone();
                    lineno = items[1].clone();
                    offset = items[2].clone();
                    text = items[3].clone();
                }
                if items.len() >= 6 {
                    end_lineno = items[4].clone();
                    end_offset = items[5].clone();
                }
            }
        }
        attrs.insert_slot("msg", msg);
        attrs.insert_slot("filename", filename);
        attrs.insert_slot("lineno", lineno);
        attrs.insert_slot("offset", offset);
        attrs.insert_slot("text", text);
        attrs.insert_slot("end_lineno", end_lineno);
        attrs.insert_slot("end_offset", end_offset);
        // CPython 3.12 also initialises print_file_and_line (always None for
        // user-constructed instances; only set by the C compile-phase injector).
        attrs.insert_slot("print_file_and_line", Value::none());
    } else if is_os_error {
        // CPython 3.12 OSError.__init__: populate errno/strerror/filename/filename2.
        // With 0 or 1 args: all None.  With 2 args: errno=args[0], strerror=args[1].
        // With 3 args: additionally filename=args[2].
        // With 5 args: args[3]=winerror (ignored on non-Windows), args[4]=filename2.
        if args.len() >= 2 {
            attrs.insert_slot("errno", args[0].clone());
            attrs.insert_slot("strerror", args[1].clone());
            // CPython 3.12: OSError.__init__ always sets self.args = (errno, strerror)
            // regardless of how many positional arguments were supplied.  The filename
            // (and filename2) are stored as dedicated instance attributes, not in args.
            attrs.insert_slot("args", Value::tuple(vec![args[0].clone(), args[1].clone()]));
        } else {
            attrs.insert_slot("errno", Value::none());
            attrs.insert_slot("strerror", Value::none());
        }
        attrs.insert_slot("filename", args.get(2).cloned().unwrap_or_else(Value::none));
        // filename2 is set by the 5-arg form: OSError(errno, strerror, fname, winerror, fname2)
        attrs.insert_slot(
            "filename2",
            args.get(4).cloned().unwrap_or_else(Value::none),
        );
        // CPython 3.12 OSError.__new__ remaps to an errno-specific subclass when
        // called as exactly OSError(errno, strerror[, ...]) where there are at
        // least 2 args and the first is an integer.  Single-arg calls (e.g.
        // OSError(2)) are NOT remapped.  Subclasses (FileNotFoundError, …) are
        // also not remapped — only the plain OSError call triggers the lookup.
        if args.len() >= 2
            && class.borrow().builtin_exception_name == Some("OSError")
            && let Some(errno_int) = oserror_remap_errno(&args[0])
            && let Some(subclass) = oserror_subclass_for_errno(errno_int)
        {
            return Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class: subclass,
                attrs,
            })));
        }
    } else if is_unicode_decode_error || is_unicode_encode_error || is_unicode_translate_error {
        // CPython 3.12: UnicodeDecodeError(encoding, object, start, end, reason)
        //               UnicodeEncodeError(encoding, object, start, end, reason)
        //               UnicodeTranslateError(object, start, end, reason)
        // Arg count validation is done in call_class_expanded before reaching here.
        // We set attributes from args when the right number are present.
        unicode_exc_set_attrs(
            &mut attrs,
            &args,
            is_unicode_decode_error || is_unicode_encode_error,
        );
    }
    if is_name_error {
        // CPython 3.12: user-constructed NameError (and UnboundLocalError) instances
        // always have a `.name` attribute, defaulting to `None`.  Interpreter-raised
        // instances set the name via `instantiate_name_error` with the actual identifier.
        attrs.insert_slot("name", Value::none());
    }
    if is_import_error {
        // CPython 3.12: user-constructed ImportError (and ModuleNotFoundError) instances
        // always have `.name` and `.path` attributes, both defaulting to `None`.
        // Interpreter-raised instances set them via `instantiate_import_error`.
        attrs.insert_slot("msg", args.first().cloned().unwrap_or_else(Value::none));
        attrs.insert_slot("name", Value::none());
        attrs.insert_slot("path", Value::none());
    }
    if is_attribute_error {
        // CPython 3.12: user-constructed AttributeError instances always have `.name`
        // and `.obj` attributes, both defaulting to `None`.  Interpreter-raised
        // instances set them via `instantiate_attribute_error` with the actual values.
        attrs.insert_slot("name", Value::none());
        attrs.insert_slot("obj", Value::none());
    }
    if is_base_exception_group {
        // PEP 654: BaseExceptionGroup(message, exceptions).
        // Set `.message` = args[0] (str) and `.exceptions` = args[1] as a tuple.
        // args[0] defaults to "" and args[1] defaults to an empty tuple on bad input,
        // but CPython validates in __new__; we set what we have.
        let message = args
            .first()
            .cloned()
            .unwrap_or_else(|| Value::string(String::new()));
        let exceptions_raw = args.get(1).cloned().unwrap_or_else(|| Value::tuple(vec![]));
        // Normalise the exceptions to a tuple (accept list too).
        let exceptions = if exceptions_raw.as_tuple().is_some() {
            exceptions_raw
        } else if let Some(lst) = exceptions_raw.as_list() {
            Value::tuple(lst.to_vec())
        } else {
            Value::tuple(vec![])
        };
        attrs.insert_slot("message", message);
        attrs.insert_slot("exceptions", exceptions);
    }
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate an `OSError` (or subclass) with the structured attributes that
/// CPython 3.12 sets when an OS error is raised from a real OS operation:
/// `errno`, `strerror`, `filename` (and `filename2 = None`).
///
/// The `args` tuple is set to `(errno, strerror)` to match CPython 3.12
/// behaviour (the 2-arg form).  The `class` must already be the correct
/// subclass (`FileNotFoundError`, `PermissionError`, etc.).
pub(crate) fn instantiate_os_error(
    class: Rc<RefCell<PyClass>>,
    errno: i64,
    strerror: String,
    filename: Option<String>,
    filename2: Option<String>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    let errno_val = Value::int(errno);
    let strerror_val = Value::string(strerror);
    attrs.insert_slot(
        "args",
        Value::tuple(vec![errno_val.clone(), strerror_val.clone()]),
    );
    attrs.insert_slot("errno", errno_val);
    attrs.insert_slot("strerror", strerror_val);
    attrs.insert_slot(
        "filename",
        filename.map(Value::string).unwrap_or_else(Value::none),
    );
    attrs.insert_slot(
        "filename2",
        filename2.map(Value::string).unwrap_or_else(Value::none),
    );
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate an `ImportError` or `ModuleNotFoundError` with `.name` and
/// `.path` instance attributes, matching CPython 3.12 `ImportError.__init__`.
///
/// `class_name` must be `"ImportError"` or `"ModuleNotFoundError"`.
/// `message` becomes `args[0]`.
/// `module_name` is stored as `.name`; if `None`, `.name` is set to `None`.
/// `.path` is always `None` (pyrust has no physical package paths).
pub(crate) fn instantiate_import_error(
    class: Rc<RefCell<PyClass>>,
    message: String,
    module_name: Option<String>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert_slot("args", Value::tuple(vec![Value::string(&message)]));
    attrs.insert_slot("msg", Value::string(message));
    let name_val = match module_name {
        Some(n) => Value::string(n),
        None => Value::none(),
    };
    attrs.insert_slot("name", name_val);
    attrs.insert_slot("path", Value::none());
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate a `NameError` or `UnboundLocalError` with the `.name` instance
/// attribute set, matching CPython 3.12 parity.
///
/// CPython 3.12: when the interpreter raises `NameError` for a missing
/// identifier, it stores the identifier string as `self.name`.  User-
/// constructed instances (e.g. `NameError('msg')`) have `name = None`.
/// `UnboundLocalError.name` is always `None` in CPython 3.12.
///
/// `name` is stored as `.name`; pass `None` for `UnboundLocalError` or when
/// the identifier is not available.
pub(crate) fn instantiate_name_error(
    class: Rc<RefCell<PyClass>>,
    message: String,
    name: Option<String>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert_slot("args", Value::tuple(vec![Value::string(message)]));
    attrs.insert_slot("__traceback__", Value::none());
    let name_val = match name {
        Some(n) => Value::string(n),
        None => Value::none(),
    };
    attrs.insert_slot("name", name_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate an `AttributeError` with the `.name` and `.obj` instance
/// attributes set, matching CPython 3.12 parity.
///
/// CPython 3.12: when the interpreter raises `AttributeError` for a missing
/// attribute, it stores the attribute name as `self.name` and the receiver
/// object as `self.obj`.  User-constructed instances (e.g.
/// `AttributeError('msg')`) have both set to `None`.
///
/// `name` is stored as `.name`; pass `None` when not available.
/// `obj` is stored as `.obj`; pass `None` when not available.
pub(crate) fn instantiate_attribute_error(
    class: Rc<RefCell<PyClass>>,
    message: String,
    name: Option<String>,
    obj: Option<Value>,
) -> Value {
    let mut attrs = InstanceAttrs::new();
    attrs.insert_slot("args", Value::tuple(vec![Value::string(message)]));
    attrs.insert_slot("__traceback__", Value::none());
    attrs.insert_slot("name", name.map(Value::string).unwrap_or_else(Value::none));
    attrs.insert_slot("obj", obj.unwrap_or_else(Value::none));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Set the five Unicode-exception structured attributes (`encoding`, `object`,
/// `start`, `end`, `reason`) on an already-allocated `attrs` map from a
/// positional argument list.
///
/// Used by both `instantiate_exception` (for user-constructed calls) and
/// `base_exception_init` (for `super().__init__(...)` in subclasses).
///
/// `has_encoding` is `true` for `UnicodeDecodeError`/`UnicodeEncodeError`
/// (which take 5 args: encoding, object, start, end, reason) and `false`
/// for `UnicodeTranslateError` (4 args: object, start, end, reason).
///
/// If the arg count doesn't match the expected signature, this function is a
/// no-op — arg count validation is the caller's responsibility.
pub(crate) fn unicode_exc_set_attrs(attrs: &mut InstanceAttrs, args: &[Value], has_encoding: bool) {
    if has_encoding {
        if args.len() != 5 {
            return;
        }
        attrs.insert_slot("encoding", args[0].clone());
        attrs.insert_slot("object", args[1].clone());
        attrs.insert_slot("start", args[2].clone());
        attrs.insert_slot("end", args[3].clone());
        attrs.insert_slot("reason", args[4].clone());
    } else {
        if args.len() != 4 {
            return;
        }
        attrs.insert_slot("encoding", Value::none());
        attrs.insert_slot("object", args[0].clone());
        attrs.insert_slot("start", args[1].clone());
        attrs.insert_slot("end", args[2].clone());
        attrs.insert_slot("reason", args[3].clone());
    }
}

/// Instantiate a `UnicodeDecodeError` with its five structured attributes set
/// from the raw Rust data produced by an internal decoding operation (e.g.
/// `bytes.decode()`).  Used by the VM when materialising a
/// `PyError::UnicodeDecodeError` variant.
pub(crate) fn instantiate_unicode_decode_error(
    class: Rc<RefCell<PyClass>>,
    encoding: String,
    object: Vec<u8>,
    start: usize,
    end: usize,
    reason: String,
) -> Value {
    let enc_val = Value::string(&encoding);
    let obj_val = Value::bytes(object);
    let start_val = Value::int(start as i64);
    let end_val = Value::int(end as i64);
    let reason_val = Value::string(&reason);
    let mut attrs = InstanceAttrs::new();
    attrs.insert_slot(
        "args",
        Value::tuple(vec![
            enc_val.clone(),
            obj_val.clone(),
            start_val.clone(),
            end_val.clone(),
            reason_val.clone(),
        ]),
    );
    attrs.insert_slot("__traceback__", Value::none());
    attrs.insert_slot("encoding", enc_val);
    attrs.insert_slot("object", obj_val);
    attrs.insert_slot("start", start_val);
    attrs.insert_slot("end", end_val);
    attrs.insert_slot("reason", reason_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Instantiate a `UnicodeEncodeError` with its five structured attributes set
/// from the raw Rust data produced by an internal encoding operation (e.g.
/// `str.encode()`).  Used by the VM when materialising a
/// `PyError::UnicodeEncodeError` variant.
pub(crate) fn instantiate_unicode_encode_error(
    class: Rc<RefCell<PyClass>>,
    encoding: String,
    object: String,
    start: usize,
    end: usize,
    reason: String,
) -> Value {
    let enc_val = Value::string(&encoding);
    let obj_val = Value::string(&object);
    let start_val = Value::int(start as i64);
    let end_val = Value::int(end as i64);
    let reason_val = Value::string(&reason);
    let mut attrs = InstanceAttrs::new();
    attrs.insert_slot(
        "args",
        Value::tuple(vec![
            enc_val.clone(),
            obj_val.clone(),
            start_val.clone(),
            end_val.clone(),
            reason_val.clone(),
        ]),
    );
    attrs.insert_slot("__traceback__", Value::none());
    attrs.insert_slot("encoding", enc_val);
    attrs.insert_slot("object", obj_val);
    attrs.insert_slot("start", start_val);
    attrs.insert_slot("end", end_val);
    attrs.insert_slot("reason", reason_val);
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Ordered list of `(python_name, class_rc)` pairs for all 31 built-in
/// exception classes, built once per thread.  Both `install_exception_builtins`
/// and `ExcClasses::from_cache` clone the `Rc`s from here instead of
/// reconstructing the exception hierarchy on every `Interpreter::default()`.
type ExcClassEntry = (&'static str, Rc<RefCell<PyClass>>);

#[cold]
fn build_exc_classes() -> Vec<ExcClassEntry> {
    // CPython 3.12 hierarchy (single-inheritance model):
    //   BaseException
    //     Exception
    //       ArithmeticError → OverflowError, ZeroDivisionError, FloatingPointError
    //       LookupError → IndexError, KeyError
    //       ValueError → UnicodeError → UnicodeEncodeError / UnicodeDecodeError
    //       RuntimeError → RecursionError, NotImplementedError
    //       TypeError, NameError → UnboundLocalError
    //       AssertionError, AttributeError, EOFError, StopIteration, SyntaxError
    //         → IndentationError → TabError
    //       MemoryError, ImportError → ModuleNotFoundError
    //       OSError → BlockingIOError, ChildProcessError, FileExistsError,
    //                 FileNotFoundError, InterruptedError, IsADirectoryError,
    //                 NotADirectoryError, PermissionError, ProcessLookupError,
    //                 TimeoutError, io.UnsupportedOperation
    //                 ConnectionError → BrokenPipeError, ConnectionAbortedError,
    //                                   ConnectionRefusedError, ConnectionResetError
    //       Warning → UserWarning, DeprecationWarning, PendingDeprecationWarning,
    //                 RuntimeWarning, SyntaxWarning, ResourceWarning, FutureWarning,
    //                 ImportWarning, UnicodeWarning, BytesWarning, EncodingWarning
    //     SystemExit, GeneratorExit, KeyboardInterrupt (direct BaseException children)
    let mk = |name: &'static str, base: Option<Rc<RefCell<PyClass>>>| {
        let mut class_data = PyClass::new(name, name, base.clone(), IndexMap::new());
        class_data.builtin_exception_name = Some(name);
        let class = Rc::new(RefCell::new(class_data));
        if let Some(b) = base {
            b.borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(&class));
        }
        class
    };
    let base_exception = mk("BaseException", None);
    // Install `add_note` (Python 3.11+ — issue #1067) on BaseException so that
    // every exception subclass inherits it via `lookup_class_attr`.
    {
        const ADD_NOTE_NAME: &str = "BaseException.add_note";
        base_exception.borrow_mut().attrs.insert(
            "add_note".to_string(),
            Value::builtin_function(ADD_NOTE_NAME),
        );
    }
    // Issue #1112: install `BaseException.__init__` so that `super().__init__(…)`
    // in a user-defined exception subclass resolves via MRO lookup and updates
    // `.args` (and `.value` for StopIteration) on the already-constructed instance.
    base_exception.borrow_mut().attrs.insert(
        "__init__".to_string(),
        Value::builtin_function("BaseException.__init__"),
    );
    // Issue #1441: install `with_traceback` on BaseException so every exception
    // subclass inherits it.  Sets __traceback__ and returns self.
    base_exception.borrow_mut().attrs.insert(
        "with_traceback".to_string(),
        Value::builtin_function("BaseException.with_traceback"),
    );
    // Issue #2361: install `__reduce__`/`__reduce_ex__` so every exception
    // subclass reduces to `(type, args[, __dict__])` — matching CPython and
    // making `copy`/`deepcopy` drop the traceback (#2360).
    base_exception.borrow_mut().attrs.insert(
        "__reduce__".to_string(),
        Value::builtin_function("BaseException.__reduce__"),
    );
    base_exception.borrow_mut().attrs.insert(
        "__reduce_ex__".to_string(),
        Value::builtin_function("BaseException.__reduce_ex__"),
    );
    let exception = mk("Exception", Some(Rc::clone(&base_exception)));
    let arithmetic_error = mk("ArithmeticError", Some(Rc::clone(&exception)));
    let lookup_error = mk("LookupError", Some(Rc::clone(&exception)));
    let runtime_error = mk("RuntimeError", Some(Rc::clone(&exception)));
    let type_error = mk("TypeError", Some(Rc::clone(&exception)));
    let value_error = mk("ValueError", Some(Rc::clone(&exception)));
    let name_error = mk("NameError", Some(Rc::clone(&exception)));
    let assertion_error = mk("AssertionError", Some(Rc::clone(&exception)));
    let stop_iteration = mk("StopIteration", Some(Rc::clone(&exception)));
    let attribute_error = mk("AttributeError", Some(Rc::clone(&exception)));
    let syntax_error = mk("SyntaxError", Some(Rc::clone(&exception)));
    let memory_error = mk("MemoryError", Some(Rc::clone(&exception)));
    let import_error = mk("ImportError", Some(Rc::clone(&exception)));
    let os_error = mk("OSError", Some(Rc::clone(&exception)));
    let overflow_error = mk("OverflowError", Some(Rc::clone(&arithmetic_error)));
    let zero_division_error = mk("ZeroDivisionError", Some(Rc::clone(&arithmetic_error)));
    let floating_point_error = mk("FloatingPointError", Some(Rc::clone(&arithmetic_error)));
    let index_error = mk("IndexError", Some(Rc::clone(&lookup_error)));
    let key_error = mk("KeyError", Some(Rc::clone(&lookup_error)));
    let recursion_error = mk("RecursionError", Some(Rc::clone(&runtime_error)));
    let not_implemented_error = mk("NotImplementedError", Some(Rc::clone(&runtime_error)));
    let unbound_local_error = mk("UnboundLocalError", Some(Rc::clone(&name_error)));
    let unicode_error = mk("UnicodeError", Some(Rc::clone(&value_error)));
    let module_not_found_error = mk("ModuleNotFoundError", Some(Rc::clone(&import_error)));
    let file_not_found_error = mk("FileNotFoundError", Some(Rc::clone(&os_error)));
    let file_exists_error = mk("FileExistsError", Some(Rc::clone(&os_error)));
    let blocking_io_error = mk("BlockingIOError", Some(Rc::clone(&os_error)));
    let child_process_error = mk("ChildProcessError", Some(Rc::clone(&os_error)));
    let interrupted_error = mk("InterruptedError", Some(Rc::clone(&os_error)));
    let is_a_directory_error = mk("IsADirectoryError", Some(Rc::clone(&os_error)));
    let not_a_directory_error = mk("NotADirectoryError", Some(Rc::clone(&os_error)));
    let permission_error = mk("PermissionError", Some(Rc::clone(&os_error)));
    let process_lookup_error = mk("ProcessLookupError", Some(Rc::clone(&os_error)));
    let timeout_error = mk("TimeoutError", Some(Rc::clone(&os_error)));
    let connection_error = mk("ConnectionError", Some(Rc::clone(&os_error)));
    let broken_pipe_error = mk("BrokenPipeError", Some(Rc::clone(&connection_error)));
    let connection_aborted_error = mk("ConnectionAbortedError", Some(Rc::clone(&connection_error)));
    let connection_refused_error = mk("ConnectionRefusedError", Some(Rc::clone(&connection_error)));
    let connection_reset_error = mk("ConnectionResetError", Some(Rc::clone(&connection_error)));
    // CPython: io.UnsupportedOperation inherits from both OSError and ValueError
    // (multiple inheritance).  pyrust uses single-inheritance; we pick OSError
    // as the primary base since that is the first in CPython's MRO and what most
    // user code catches (`except OSError`).  The class is registered under both
    // "io.UnsupportedOperation" (the dotted name used by raise sites) and
    // "UnsupportedOperation" (the bare name printed in tracebacks).
    let unsupported_operation = mk("UnsupportedOperation", Some(Rc::clone(&os_error)));
    // Python 3.3+: IOError and EnvironmentError are aliases for OSError.
    let io_error = Rc::clone(&os_error);
    let environment_error = Rc::clone(&os_error);
    let indentation_error = mk("IndentationError", Some(Rc::clone(&syntax_error)));
    let tab_error = mk("TabError", Some(Rc::clone(&indentation_error)));
    let warning = mk("Warning", Some(Rc::clone(&exception)));
    let user_warning = mk("UserWarning", Some(Rc::clone(&warning)));
    let deprecation_warning = mk("DeprecationWarning", Some(Rc::clone(&warning)));
    let pending_deprecation_warning = mk("PendingDeprecationWarning", Some(Rc::clone(&warning)));
    let runtime_warning = mk("RuntimeWarning", Some(Rc::clone(&warning)));
    let syntax_warning = mk("SyntaxWarning", Some(Rc::clone(&warning)));
    let resource_warning = mk("ResourceWarning", Some(Rc::clone(&warning)));
    let future_warning = mk("FutureWarning", Some(Rc::clone(&warning)));
    let import_warning = mk("ImportWarning", Some(Rc::clone(&warning)));
    let unicode_warning = mk("UnicodeWarning", Some(Rc::clone(&warning)));
    let bytes_warning = mk("BytesWarning", Some(Rc::clone(&warning)));
    let encoding_warning = mk("EncodingWarning", Some(Rc::clone(&warning)));
    let unicode_encode_error = mk("UnicodeEncodeError", Some(Rc::clone(&unicode_error)));
    let unicode_decode_error = mk("UnicodeDecodeError", Some(Rc::clone(&unicode_error)));
    let unicode_translate_error = mk("UnicodeTranslateError", Some(Rc::clone(&unicode_error)));
    let buffer_error = mk("BufferError", Some(Rc::clone(&exception)));
    let reference_error = mk("ReferenceError", Some(Rc::clone(&exception)));
    let system_error = mk("SystemError", Some(Rc::clone(&exception)));
    let stop_async_iteration = mk("StopAsyncIteration", Some(Rc::clone(&exception)));
    let eof_error = mk("EOFError", Some(Rc::clone(&exception)));
    let system_exit = mk("SystemExit", Some(Rc::clone(&base_exception)));
    let generator_exit = mk("GeneratorExit", Some(Rc::clone(&base_exception)));
    let keyboard_interrupt = mk("KeyboardInterrupt", Some(Rc::clone(&base_exception)));
    // PEP 654 (Python 3.11+): BaseExceptionGroup and ExceptionGroup.
    // BaseExceptionGroup(message, exceptions) — accepts any BaseException subclass.
    // ExceptionGroup(message, exceptions)    — only accepts Exception subclasses;
    //   inherits from both BaseExceptionGroup (primary) and Exception (extra base).
    let base_exception_group = mk("BaseExceptionGroup", Some(Rc::clone(&base_exception)));
    // PEP 654: install `derive`, `subgroup`, and `split` on BaseExceptionGroup
    // so every group subclass inherits them.  These are intercepted in
    // `call_function_expanded` (they need interpreter access to call user
    // predicates / a subclass's overridden `derive`).
    {
        let mut beg = base_exception_group.borrow_mut();
        beg.attrs.insert(
            "derive".to_string(),
            Value::builtin_function("BaseExceptionGroup.derive"),
        );
        beg.attrs.insert(
            "subgroup".to_string(),
            Value::builtin_function("BaseExceptionGroup.subgroup"),
        );
        beg.attrs.insert(
            "split".to_string(),
            Value::builtin_function("BaseExceptionGroup.split"),
        );
    }
    // ExceptionGroup uses multiple inheritance: primary base = BaseExceptionGroup,
    // extra base = Exception.  Build it manually so we can set extra_bases.
    let exception_group = Rc::new(RefCell::new(PyClass {
        extra_bases: vec![Rc::clone(&exception)],
        builtin_exception_name: Some("ExceptionGroup"),
        ..PyClass::new(
            "ExceptionGroup",
            "ExceptionGroup",
            Some(Rc::clone(&base_exception_group)),
            IndexMap::new(),
        )
    }));
    base_exception_group
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&exception_group));
    exception
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&exception_group));
    vec![
        ("BaseException", base_exception),
        ("Exception", exception),
        ("ArithmeticError", arithmetic_error),
        ("OverflowError", overflow_error),
        ("ZeroDivisionError", zero_division_error),
        ("FloatingPointError", floating_point_error),
        ("LookupError", lookup_error),
        ("IndexError", index_error),
        ("KeyError", key_error),
        ("RuntimeError", runtime_error),
        ("RecursionError", recursion_error),
        ("NotImplementedError", not_implemented_error),
        ("TypeError", type_error),
        ("ValueError", value_error),
        ("NameError", name_error),
        ("UnboundLocalError", unbound_local_error),
        ("AssertionError", assertion_error),
        ("EOFError", eof_error),
        ("StopIteration", stop_iteration),
        ("AttributeError", attribute_error),
        ("SyntaxError", syntax_error),
        ("IndentationError", indentation_error),
        ("TabError", tab_error),
        ("MemoryError", memory_error),
        ("ImportError", import_error),
        ("ModuleNotFoundError", module_not_found_error),
        ("UnicodeError", unicode_error),
        ("UnicodeEncodeError", unicode_encode_error),
        ("UnicodeDecodeError", unicode_decode_error),
        ("UnicodeTranslateError", unicode_translate_error),
        ("BufferError", buffer_error),
        ("ReferenceError", reference_error),
        ("SystemError", system_error),
        ("StopAsyncIteration", stop_async_iteration),
        ("OSError", os_error),
        ("IOError", io_error),
        ("EnvironmentError", environment_error),
        ("FileNotFoundError", file_not_found_error),
        ("FileExistsError", file_exists_error),
        ("BlockingIOError", blocking_io_error),
        ("ChildProcessError", child_process_error),
        ("InterruptedError", interrupted_error),
        ("IsADirectoryError", is_a_directory_error),
        ("NotADirectoryError", not_a_directory_error),
        ("PermissionError", permission_error),
        ("ProcessLookupError", process_lookup_error),
        ("TimeoutError", timeout_error),
        ("ConnectionError", connection_error),
        ("BrokenPipeError", broken_pipe_error),
        ("ConnectionAbortedError", connection_aborted_error),
        ("ConnectionRefusedError", connection_refused_error),
        ("ConnectionResetError", connection_reset_error),
        ("io.UnsupportedOperation", Rc::clone(&unsupported_operation)),
        ("UnsupportedOperation", unsupported_operation),
        ("Warning", warning),
        ("UserWarning", user_warning),
        ("DeprecationWarning", deprecation_warning),
        ("PendingDeprecationWarning", pending_deprecation_warning),
        ("RuntimeWarning", runtime_warning),
        ("SyntaxWarning", syntax_warning),
        ("ResourceWarning", resource_warning),
        ("FutureWarning", future_warning),
        ("ImportWarning", import_warning),
        ("UnicodeWarning", unicode_warning),
        ("BytesWarning", bytes_warning),
        ("EncodingWarning", encoding_warning),
        ("SystemExit", system_exit),
        ("GeneratorExit", generator_exit),
        ("KeyboardInterrupt", keyboard_interrupt),
        ("BaseExceptionGroup", base_exception_group),
        ("ExceptionGroup", exception_group),
    ]
}

thread_local! {
    /// Per-thread cache of all built-in exception class `Rc`s.
    /// Built once per thread; each `Interpreter::default()` call clones the
    /// `Rc`s (O(1) reference-count bumps) instead of allocating fresh
    /// `Rc<RefCell<PyClass>>` objects for the full hierarchy.
    static EXC_CLASS_CACHE: Vec<ExcClassEntry> = build_exc_classes();
}

/// Look up a built-in exception class by name, using the thread-local cache.
/// Called by `resolve_builtin` to service `LoadGlobal("TypeError")` etc.
/// without inserting exception classes into the module env at startup.
/// Triggers `EXC_CLASS_CACHE` initialisation on the very first call.
pub(crate) fn lookup_exc_class(name: &str) -> Option<Rc<RefCell<PyClass>>> {
    EXC_CLASS_CACHE.with(|cache| {
        cache
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, cls)| Rc::clone(cls))
    })
}

/// Build the `ExcClasses` map from the thread-local cache.  Called once
/// per interpreter (lazily, on first exception raise or class lookup).
///
/// Insertion ordered (issue #2918): `prepare_builtins_module` publishes the
/// exception classes into `builtins` by iterating this map, so a hashed map
/// made the tail of `vars(builtins)` differ run to run.  `EXC_CLASS_CACHE` is
/// an ordered `Vec` declaring the hierarchy roots first, and that order carries
/// through to the module namespace.
pub(crate) fn build_exc_class_map() -> indexmap::IndexMap<&'static str, Rc<RefCell<PyClass>>> {
    EXC_CLASS_CACHE.with(|cache| {
        let mut map = indexmap::IndexMap::with_capacity(cache.len());
        for (name, cls) in cache {
            map.insert(*name, Rc::clone(cls));
        }
        map
    })
}
