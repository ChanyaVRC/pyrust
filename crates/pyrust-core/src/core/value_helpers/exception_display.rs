pub fn is_exception_instance(instance: &Rc<RefCell<PyInstance>>) -> bool {
    let class = Rc::clone(&instance.borrow().class);
    class_chain_contains_exception(&class)
}

/// Canonical "is this class an exception?" predicate, shared by the runtime
/// (`raise`/`except` machinery) and `Value::repr`/`Value::str` for
/// `PyInstance`.  Both paths must agree, or `raise X(...)` succeeds while
/// `repr(X(...))` falls back to the default `<X object>` formatting (issue
/// #429).
///
/// `BaseException` is the root of the CPython exception hierarchy (#574).
/// Any class whose base chain reaches the canonical internally-tagged
/// `BaseException` class is treated as an exception class. Python-visible
/// names are deliberately ignored: they are mutable and can be reused by an
/// unrelated user class. See
/// [`crate::interpreter::helpers::install_exception_builtins`] in the
/// `pyrust` crate for where the classes are constructed.
pub fn class_chain_contains_exception(class: &Rc<RefCell<PyClass>>) -> bool {
    class_chain_contains_builtin_exception(class, "BaseException")
}

/// Return whether `class` is the canonical built-in exception named `name`,
/// or a real subclass of it.
///
/// The comparison uses interpreter-owned immutable tags rather than mutable
/// Python-visible class metadata. This is the shared identity boundary for
/// core error matching and interpreter exception special cases.
pub fn class_chain_contains_builtin_exception(class: &Rc<RefCell<PyClass>>, name: &str) -> bool {
    // Walk the base chain by reference — each node is a distinct `RefCell`, so
    // recursing while the current borrow is held never conflicts.  This is on
    // the per-raise hot path (`is_exception_class` runs once per construction);
    let borrowed = class.borrow();
    if borrowed.builtin_exception_name == Some(name) {
        return true;
    }
    if let Some(base) = &borrowed.base
        && class_chain_contains_builtin_exception(base, name)
    {
        return true;
    }
    borrowed
        .extra_bases
        .iter()
        .any(|base| class_chain_contains_builtin_exception(base, name))
}

/// If `instance` is a builtin-subclass carrier (holds a `__builtin_data__`
/// backing) and no class between it and the builtin base defines `__repr__`
/// or `__str__`, return the backing value for repr purposes (issue #2389).
///
/// Core-side renderers (exception-arg formatting: `KeyError: [2]`, OSError
/// filenames, …) cannot invoke user dunders — that needs the interpreter —
/// but the *inherited* case is pure data: CPython inherits the builtin
/// `tp_repr` slot, so the subclass instance reprs as its base value.  A
/// chain that overrides `__repr__`/`__str__` returns `None` and keeps the
/// generic `<module.Class object at 0x…>` form (the override would need
/// interpreter dispatch to honour).
fn instance_backing_for_repr(instance: &Rc<RefCell<PyInstance>>) -> Option<Value> {
    let backing = instance.borrow().attrs.get("__builtin_data__").cloned()?;
    let base_tag = backing_class_tag(&backing)?;
    let mut cursor = Some(Rc::clone(&instance.borrow().class));
    while let Some(class) = cursor {
        let borrowed = class.borrow();
        // Reached the builtin base (or object): its repr IS the backing repr.
        if matches!(
            borrowed.canonical_tag,
            Some(tag) if tag == base_tag || tag == CanonicalClassTag::Object
        ) {
            break;
        }
        if borrowed.attrs.contains_key("__repr__") || borrowed.attrs.contains_key("__str__") {
            return None;
        }
        // Multiple inheritance: any extra base defining the dunder also wins
        // over the builtin slot (it precedes the builtin in the MRO when
        // declared first; conservatively treat any definition as an override).
        if borrowed
            .extra_bases
            .iter()
            .any(|b| class_chain_defines_repr_like(b, base_tag))
        {
            return None;
        }
        cursor = borrowed.base.clone();
    }
    Some(backing)
}

/// Return the canonical class identity corresponding to a primitive backing
/// value stored in `__builtin_data__`.
fn backing_class_tag(value: &Value) -> Option<CanonicalClassTag> {
    Some(match value.kind() {
        ValueKind::None => CanonicalClassTag::NoneType,
        ValueKind::Bool(_) => CanonicalClassTag::Bool,
        ValueKind::Int(_) | ValueKind::BigInt(_) => CanonicalClassTag::Int,
        ValueKind::Float(_) => CanonicalClassTag::Float,
        ValueKind::Str(_) => CanonicalClassTag::Str,
        ValueKind::List(_) => CanonicalClassTag::List,
        ValueKind::Tuple(_) => CanonicalClassTag::Tuple,
        ValueKind::Dict(_) => CanonicalClassTag::Dict,
        ValueKind::Set(_) => CanonicalClassTag::Set,
        ValueKind::Bytes(_) => CanonicalClassTag::Bytes,
        ValueKind::Complex(_, _) => CanonicalClassTag::Complex,
        ValueKind::NotImplemented => CanonicalClassTag::NotImplementedType,
        ValueKind::Ellipsis => CanonicalClassTag::Ellipsis,
        ValueKind::BuiltinObject { ops, .. } => ops.canonical_class_tag()?,
        _ => return None,
    })
}

/// Walk `class` (stopping at the tagged primitive base or tagged `object`)
/// looking for a
/// `__repr__`/`__str__` definition.  Helper for the extra-bases arm of
/// [`instance_backing_for_repr`].
fn class_chain_defines_repr_like(
    class: &Rc<RefCell<PyClass>>,
    base_tag: CanonicalClassTag,
) -> bool {
    let borrowed = class.borrow();
    if matches!(
        borrowed.canonical_tag,
        Some(tag) if tag == base_tag || tag == CanonicalClassTag::Object
    ) {
        return false;
    }
    if borrowed.attrs.contains_key("__repr__") || borrowed.attrs.contains_key("__str__") {
        return true;
    }
    let from_base = borrowed
        .base
        .as_ref()
        .is_some_and(|b| class_chain_defines_repr_like(b, base_tag));
    from_base
        || borrowed
            .extra_bases
            .iter()
            .any(|b| class_chain_defines_repr_like(b, base_tag))
}

fn exception_args(instance: &Rc<RefCell<PyInstance>>) -> Vec<Value> {
    match instance.borrow().attrs.get_slot("args").map(|v| v.kind()) {
        Some(ValueKind::Tuple(args)) => args.to_vec(),
        _ => Vec::new(),
    }
}

fn format_exception_args(args: &[Value], repr_mode: bool) -> String {
    match args {
        [] => String::new(),
        [value] => {
            if repr_mode {
                value.repr_raw()
            } else {
                value.to_py_str()
            }
        }
        _ => {
            let inner = args
                .iter()
                .map(|value| value.repr_raw())
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
    }
}

/// Format a codepoint the way CPython formats it in Unicode error `__str__`
/// messages: `\xHH` for U+00xx, `\uHHHH` for U+xxxx, `\UHHHHHHHH` for wider.
fn format_codepoint_escape(cp: u32) -> String {
    if cp < 0x100 {
        format!("\\x{cp:02x}")
    } else if cp < 0x10000 {
        format!("\\u{cp:04x}")
    } else {
        format!("\\U{cp:08x}")
    }
}

/// Build the `__str__` message for a `UnicodeDecodeError` from its five
/// structured attributes.  Matches CPython 3.12 `UnicodeDecodeError_str`.
pub fn format_unicode_decode_str(
    encoding: &str,
    object: &[u8],
    start: usize,
    end: usize,
    reason: &str,
) -> String {
    if end == start + 1 {
        let byte = object.get(start).copied().unwrap_or(0);
        format!("'{encoding}' codec can't decode byte 0x{byte:02x} in position {start}: {reason}")
    } else {
        format!(
            "'{encoding}' codec can't decode bytes in position {start}-{}: {reason}",
            end - 1
        )
    }
}

/// Build the `__str__` message for a `UnicodeEncodeError` from its five
/// structured attributes.  Matches CPython 3.12 `UnicodeEncodeError_str`.
pub fn format_unicode_encode_str(
    encoding: &str,
    object: &str,
    start: usize,
    end: usize,
    reason: &str,
) -> String {
    // Iterate via cesu8_codepoints: the object may hold lone surrogates (the
    // very case that raised this error), and str::chars() would abort on them
    // in debug builds.
    let cps: Vec<u32> = cesu8_codepoints(object).collect();
    if end == start + 1 {
        let cp = cps.get(start).copied().unwrap_or(0);
        let esc = format_codepoint_escape(cp);
        format!("'{encoding}' codec can't encode character '{esc}' in position {start}: {reason}")
    } else {
        format!(
            "'{encoding}' codec can't encode characters in position {start}-{}: {reason}",
            end - 1
        )
    }
}

/// Build the `__str__` message for a `UnicodeTranslateError` from its four
/// structured attributes.  Matches CPython 3.12 `UnicodeTranslateError_str`.
pub fn format_unicode_translate_str(
    object: &str,
    start: usize,
    end: usize,
    reason: &str,
) -> String {
    let cps: Vec<u32> = cesu8_codepoints(object).collect();
    if end == start + 1 {
        let cp = cps.get(start).copied().unwrap_or(0);
        let esc = format_codepoint_escape(cp);
        format!("can't translate character '{esc}' in position {start}: {reason}")
    } else {
        format!(
            "can't translate characters in position {start}-{}: {reason}",
            end - 1
        )
    }
}

/// Read a `usize`-valued instance attribute (e.g. `start`/`end` on the
/// `Unicode*Error` types): present, an int, coerced via `as usize`.
fn attr_usize(b: &PyInstance, name: &str) -> Option<usize> {
    b.attrs
        .get_slot(name)
        .and_then(|v| v.as_int())
        .map(|i| i as usize)
}

/// Read a `String`-valued instance attribute (e.g. `encoding`/`reason` on the
/// `Unicode*Error` types): present and a `str`.
fn attr_string(b: &PyInstance, name: &str) -> Option<String> {
    b.attrs
        .get_slot(name)
        .and_then(|v| v.as_str().map(str::to_owned))
}

fn exception_to_string(instance: &Rc<RefCell<PyInstance>>) -> String {
    let args = exception_args(instance);
    // CPython's `KeyError.__str__` always uses repr of the single arg, so
    // `str(KeyError('x'))` returns `"'x'"` (one level of quoting).  All other
    // exception classes use `str()` of the arg (no extra quoting).
    let is_key_error = class_chain_contains_builtin_exception(&instance.borrow().class, "KeyError");

    // CPython's `OSError.__str__` (and all subclasses) formats as
    // "[Errno N] strerror" or "[Errno N] strerror: repr(filename)" when
    // the instance was constructed with 2+ args (i.e. errno/strerror C slots
    // were initialised by `OSError.__init__`).  The format is used regardless
    // of whether those attributes were subsequently set to None from Python —
    // CPython's `OSError_str` (Objects/exceptions.c) checks the C member
    // pointers for NULL (never-initialised) rather than for Py_None
    // (explicitly-set-to-None), and `args.len() >= 2` is the pyrust proxy for
    // "the C slots were initialised".
    // With the 5-arg form, if filename2 is also non-None: "... -> repr(filename2)".
    if class_chain_contains_builtin_exception(&instance.borrow().class, "OSError")
        && args.len() >= 2
    {
        let borrowed = instance.borrow();
        let errno_val = borrowed.attrs.get_slot("errno");
        let strerror_val = borrowed.attrs.get_slot("strerror");
        let filename_val = borrowed.attrs.get_slot("filename");
        let filename2_val = borrowed.attrs.get_slot("filename2");
        if let (Some(errno), Some(strerror)) = (errno_val, strerror_val) {
            let base = format!("[Errno {}] {}", errno.to_py_str(), strerror.to_py_str());
            match filename_val {
                Some(fname) if !fname.is_none() => {
                    let with_fname = format!("{}: {}", base, fname.repr_raw());
                    match filename2_val {
                        Some(fname2) if !fname2.is_none() => {
                            return format!("{} -> {}", with_fname, fname2.repr_raw());
                        }
                        _ => return with_fname,
                    }
                }
                _ => return base,
            }
        }
    }

    // CPython's `SyntaxError.__str__` (and subclasses IndentationError,
    // TabError) formats as:
    //   "{msg} ({filename}, line {lineno})"  — when both filename (str) and
    //                                           lineno (int) are set
    //   "{msg} ({filename})"                 — filename set, lineno not an int
    //   "{msg} (line {lineno})"              — lineno set, filename not a str
    //   "{msg}"                              — neither set
    // The structured attrs (`.msg`, `.filename`, `.lineno`) are used, not
    // the raw `args` tuple, matching CPython's `SyntaxError_str` in
    // Objects/exceptions.c.
    if class_chain_contains_builtin_exception(&instance.borrow().class, "SyntaxError") {
        let borrowed = instance.borrow();
        let msg_str = borrowed
            .attrs
            .get_slot("msg")
            .map(|v| v.to_py_str())
            .unwrap_or_else(|| "None".to_owned());
        let filename_str = borrowed
            .attrs
            .get_slot("filename")
            .and_then(|v| v.as_str().map(str::to_owned));
        let lineno_int = borrowed.attrs.get_slot("lineno").and_then(|v| v.as_int());
        return match (filename_str, lineno_int) {
            (Some(fname), Some(lineno)) => format!("{msg_str} ({fname}, line {lineno})"),
            (Some(fname), None) => format!("{msg_str} ({fname})"),
            (None, Some(lineno)) => format!("{msg_str} (line {lineno})"),
            (None, None) => msg_str,
        };
    }

    // CPython's UnicodeDecodeError.__str__ derives the message from the five
    // structured attributes, not from args.  Only format this way when all
    // five attributes are set (i.e. the exception was properly constructed).
    if class_chain_contains_builtin_exception(&instance.borrow().class, "UnicodeDecodeError") {
        let borrowed = instance.borrow();
        let enc = attr_string(&borrowed, "encoding");
        let obj = borrowed.attrs.get_slot("object").and_then(|v| {
            if let ValueKind::Bytes(rc) = v.kind() {
                Some(rc.as_ref().clone())
            } else {
                None
            }
        });
        let start = attr_usize(&borrowed, "start");
        let end = attr_usize(&borrowed, "end");
        let reason = attr_string(&borrowed, "reason");
        if let (Some(enc), Some(obj), Some(start), Some(end), Some(reason)) =
            (enc, obj, start, end, reason)
        {
            return format_unicode_decode_str(&enc, &obj, start, end, &reason);
        }
    }

    // CPython's UnicodeEncodeError.__str__ derives the message from the five
    // structured attributes.
    if class_chain_contains_builtin_exception(&instance.borrow().class, "UnicodeEncodeError") {
        let borrowed = instance.borrow();
        let enc = attr_string(&borrowed, "encoding");
        let obj = attr_string(&borrowed, "object");
        let start = attr_usize(&borrowed, "start");
        let end = attr_usize(&borrowed, "end");
        let reason = attr_string(&borrowed, "reason");
        if let (Some(enc), Some(obj), Some(start), Some(end), Some(reason)) =
            (enc, obj, start, end, reason)
        {
            return format_unicode_encode_str(&enc, &obj, start, end, &reason);
        }
    }

    // CPython's UnicodeTranslateError.__str__ derives the message from four
    // structured attributes (no encoding).
    if class_chain_contains_builtin_exception(&instance.borrow().class, "UnicodeTranslateError") {
        let borrowed = instance.borrow();
        let obj = attr_string(&borrowed, "object");
        let start = attr_usize(&borrowed, "start");
        let end = attr_usize(&borrowed, "end");
        let reason = attr_string(&borrowed, "reason");
        if let (Some(obj), Some(start), Some(end), Some(reason)) = (obj, start, end, reason) {
            return format_unicode_translate_str(&obj, start, end, &reason);
        }
    }

    // CPython's `BaseExceptionGroup.__str__` formats as
    // "%s (%zd sub-exception%s)" % (message, n, "s" if n != 1 else "") —
    // i.e. the `.message` string followed by the count of direct
    // sub-exceptions (NOT recursive leaf count), with plural "s" when n != 1.
    if class_chain_contains_builtin_exception(&instance.borrow().class, "BaseExceptionGroup") {
        let borrowed = instance.borrow();
        let message = borrowed
            .attrs
            .get_slot("message")
            .map(|v| v.to_py_str())
            .unwrap_or_default();
        let n = borrowed
            .attrs
            .get_slot("exceptions")
            .and_then(|v| v.as_tuple().map(|t| t.len()))
            .unwrap_or(0);
        let plural = if n == 1 { "" } else { "s" };
        return format!("{message} ({n} sub-exception{plural})");
    }

    format_exception_args(&args, is_key_error)
}

fn exception_repr(instance: &Rc<RefCell<PyInstance>>) -> String {
    let class_name = instance.borrow().class.borrow().name.clone();
    let args = exception_args(instance);
    if args.is_empty() {
        format!("{class_name}()")
    } else {
        // CPython's BaseException.__repr__ renders all args comma-separated
        // inside the class-name parens: `ExcName(repr(a0), repr(a1), ...)`.
        // Do NOT use `format_exception_args` here — its multi-arg branch wraps
        // in an extra pair of parens (`"(a, b)"`), which produces the wrong
        // `ExcName((a, b))` instead of `ExcName(a, b)`.
        let inner = args
            .iter()
            .map(|v| v.repr_raw())
            .collect::<Vec<_>>()
            .join(", ");
        format!("{class_name}({inner})")
    }
}
