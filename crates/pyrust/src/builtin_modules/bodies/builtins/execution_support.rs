/// CPython injects `__builtins__` into a caller-supplied globals dict the first
/// time `eval()` or `exec()` is called with it (see `PyEval_EvalCode`).  If the
/// dict does not already contain `"__builtins__"`, insert the builtins module.
///
/// Existing values (including `{}` as a deliberate override) are left alone.
fn inject_builtins_into_globals(globals: &Value) {
    use crate::value::StrKey;
    let already_present = globals
        .dict_with(|d| d.contains_key(&StrKey("__builtins__")))
        .unwrap_or(true); // not a dict — leave it alone
    if !already_present {
        let builtins = crate::interpreter::cached_builtins_module();
        let _ = globals.dict_insert(PyKey::str_from("__builtins__"), builtins);
    }
}

/// Resolve the namespaces an `eval()` / `exec()` call uses when `globals` is
/// omitted or `None` (CPython 3.12 `builtin_eval_impl` / `builtin_exec_impl`):
///
/// - `globals=None` takes the calling frame's globals.
/// - `globals=None, locals=None` additionally takes the calling frame's locals
///   mapping: the live class namespace in a class body, or the function's
///   `f_locals` (fast locals plus cell / free variables, issue #3317) — merged
///   with PEP 709 inlined-comprehension variables when called from inside a
///   list / set / dict comprehension.
/// - An explicit `globals` with `locals=None` is left alone; the dict-namespace
///   runners already use `globals` as both mappings.
///
/// At module scope with both omitted the result stays `(None, None)` so the
/// direct module-namespace runner keeps its existing fast path.
fn resolve_default_exec_namespaces(
    interp: &mut Interpreter,
    globals: &mut Option<Value>,
    locals: &mut Option<Value>,
) {
    use crate::interpreter::FrameKind;
    if globals.is_some() {
        return;
    }
    let at_module_scope = interp
        .vm_frame_views
        .last()
        .is_none_or(|view| view.kind() == FrameKind::Script);
    if at_module_scope {
        if locals.is_some() {
            // `exec(src, None, mapping)` at module scope: the caller's module
            // globals plus the explicit locals (same dict `globals()` returns).
            sync_module_env_to_globals_dict(interp);
            *globals = Some(interp.active_globals_dict());
        }
        return;
    }
    let active_env = Rc::clone(&interp.env);
    *globals = Some(interp.globals_for_environment(&active_env));
    if locals.is_none() {
        *locals = Some(current_locals_value(interp));
    }
}

/// Scan the first two lines of `bytes` for a PEP 263 coding declaration:
/// `^[ \t\f]*#.*?coding[:=][ \t]*([-\w.]+)`
fn detect_pep263_encoding(bytes: &[u8]) -> Option<String> {
    for line in bytes.split(|&b| b == b'\n').take(2) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let trimmed = line
            .iter()
            .position(|&b| !matches!(b, b' ' | b'\t' | 0x0c))
            .map(|i| &line[i..])
            .unwrap_or(&[]);
        if !trimmed.starts_with(b"#") {
            continue;
        }
        let comment = &trimmed[1..];
        for i in 0..comment.len() {
            if comment[i..].starts_with(b"coding:") || comment[i..].starts_with(b"coding=") {
                let mut rest = &comment[i + 7..];
                while let Some((&b, tail)) = rest.split_first() {
                    if b == b' ' || b == b'\t' {
                        rest = tail;
                    } else {
                        break;
                    }
                }
                let end = rest
                    .iter()
                    .position(|&b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')))
                    .unwrap_or(rest.len());
                if end > 0 {
                    return Some(String::from_utf8_lossy(&rest[..end]).into_owned());
                }
            }
        }
    }
    None
}

fn normalize_encoding_name(enc: &str) -> String {
    enc.trim().to_ascii_lowercase().replace('_', "-")
}

/// Decode raw Python source `bytes` / `bytearray` honoring UTF-8 BOM and PEP 263
/// `# coding:` declarations.
fn decode_source_bytes(interp: &mut Interpreter, raw: &[u8]) -> Result<String> {
    let (has_bom, body) = if let Some(rest) = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        (true, rest)
    } else {
        (false, raw)
    };
    let declared = detect_pep263_encoding(body);
    if has_bom
        && let Some(ref enc) = declared
        && !matches!(
            normalize_encoding_name(enc).as_str(),
            "utf-8" | "utf8" | "utf-8-sig"
        )
    {
        return Err(PyError::named(
            "SyntaxError",
            "encoding problem: utf-8".to_string(),
        ));
    }
    let encoding = declared.unwrap_or_else(|| "utf-8".to_string());
    let norm = normalize_encoding_name(&encoding);
    let decoded = match norm.as_str() {
        "utf-8" | "utf8" | "utf-8-sig" => std::str::from_utf8(body)
            .map(str::to_owned)
            .map_err(|e| {
                let bad = body.get(e.valid_up_to()).copied().unwrap_or(0);
                PyError::named(
                    "SyntaxError",
                    format!(
                        "Non-UTF-8 code starting with '\\x{bad:02x}' in file on line 1, \
                         but no encoding declared; see https://peps.python.org/pep-0263/ for details"
                    ),
                )
            })?,
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" => {
            body.iter().map(|&b| b as char).collect()
        }
        "ascii" | "us-ascii" => {
            if let Some(&bad) = body.iter().find(|&&b| b >= 0x80) {
                return Err(PyError::named(
                    "SyntaxError",
                    format!("encoding problem: ascii (byte 0x{bad:02x})"),
                ));
            }
            std::str::from_utf8(body).unwrap_or("").to_owned()
        }
        _ => {
            let bytes_val = Value::bytes(body.to_vec());
            let decode_fn = interp.get_attr(&bytes_val, "decode")?;
            let decoded_val = interp
                .call_function_expanded(
                    decode_fn,
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(encoding.clone()),
                    }],
                )
                .map_err(|e| {
                    if e.class_name_is("LookupError") {
                        PyError::named("SyntaxError", format!("unknown encoding: {encoding}"))
                    } else if e.class_name_is("UnicodeDecodeError") {
                        PyError::named("SyntaxError", format!("encoding problem: {encoding}"))
                    } else {
                        e
                    }
                })?;
            decoded_val
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| PyError::named("SyntaxError", "encoding did not return str"))?
        }
    };
    if decoded.contains('\0') {
        return Err(PyError::named(
            "SyntaxError",
            "source code string cannot contain null bytes".to_string(),
        ));
    }
    Ok(decoded)
}

/// Extract source text from `str`, `bytes`, or `bytearray` (with PEP 263
/// decoding for bytes-like inputs).
fn extract_source_text(
    interp: &mut Interpreter,
    val: &Value,
    fn_name: &str,
    expected_desc: &str,
) -> Result<String> {
    if let Some(s) = val.as_str() {
        if s.contains('\0') {
            return Err(PyError::named(
                "SyntaxError",
                "source code string cannot contain null bytes".to_string(),
            ));
        }
        return Ok(s.to_owned());
    }
    if let ValueKind::Bytes(b) = val.kind() {
        return decode_source_bytes(interp, b);
    }
    if let Some(bytes) = crate::interpreter::try_extract_buffer_bytes(interp, val)? {
        return decode_source_bytes(interp, &bytes);
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "{fn_name}() arg 1 must be a {expected_desc}, not '{}'",
            value_type_name_str(val),
        ),
    ))
}

/// Extract `filename` for `compile()` from `str`, `bytes`, `bytearray`, or
/// `os.PathLike` (`__fspath__`).
fn extract_compile_filename(interp: &mut Interpreter, val: &Value) -> Result<String> {
    if let Some(s) = val.as_str() {
        return Ok(s.to_owned());
    }
    if let ValueKind::Bytes(b) = val.kind() {
        return Ok(String::from_utf8_lossy(b).into_owned());
    }
    if let Some(bytes) = crate::interpreter::try_extract_buffer_bytes(interp, val)? {
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    if let Ok(method) = interp.get_attr(val, "__fspath__") {
        let result = interp.call_function_expanded(method, &[])?;
        if let Some(s) = result.as_str() {
            return Ok(s.to_owned());
        }
        if let ValueKind::Bytes(b) = result.kind() {
            return Ok(String::from_utf8_lossy(b).into_owned());
        }
        return Err(PyError::named(
            "TypeError",
            format!(
                "expected {}.__fspath__() to return str or bytes, not {}",
                value_type_name_str(val),
                value_type_name_str(&result),
            ),
        ));
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "compile() arg 2 must be a string, bytes or os.PathLike object, not {}",
            value_type_name_str(val),
        ),
    ))
}

/// Return `true` if `val` is an `ast.AST` instance (walking MRO without
/// eagerly importing `ast` for non-instance values).
fn is_ast_node(val: &Value) -> bool {
    let ValueKind::PyInstance(inst) = val.kind() else {
        return false;
    };
    let class = Rc::clone(&inst.borrow().class);
    let mut stack = vec![class];
    while let Some(cls) = stack.pop() {
        let b = cls.borrow();
        if b.name == "AST"
            && b.attrs
                .get("__module__")
                .and_then(|v| v.as_str())
                .is_some_and(|m| m == "ast")
        {
            return true;
        }
        if let Some(ref base) = b.base {
            stack.push(Rc::clone(base));
        }
        for eb in &b.extra_bases {
            stack.push(Rc::clone(eb));
        }
    }
    false
}

/// Return `true` if `cls` inherits from a class named `target_name` in module `ast`.
fn instance_is_ast_subclass(val: &Value, target_name: &str) -> bool {
    let ValueKind::PyInstance(inst) = val.kind() else {
        return false;
    };
    let class = Rc::clone(&inst.borrow().class);
    let mut stack = vec![class];
    while let Some(cls) = stack.pop() {
        let b = cls.borrow();
        if b.name == target_name
            && b.attrs
                .get("__module__")
                .and_then(|v| v.as_str())
                .is_some_and(|m| m == "ast")
        {
            return true;
        }
        if let Some(ref base) = b.base {
            stack.push(Rc::clone(base));
        }
        for eb in &b.extra_bases {
            stack.push(Rc::clone(eb));
        }
    }
    false
}

/// Extract optional top-level statement line numbers from an `ast.Module` /
/// `ast.Interactive` / `ast.Expression` node so `compile(ast_node, ...)` keeps
/// user-assigned / `ast.increment_lineno()` line numbers.
fn extract_ast_linenos(interp: &mut Interpreter, node: &Value, mode: &str) -> Vec<u32> {
    let Ok(body) = interp.get_attr(node, "body") else {
        return Vec::new();
    };
    if mode == "eval" {
        if let Ok(ln_val) = interp.get_attr(&body, "lineno")
            && let Some(n) = ln_val.as_int()
            && n > 0
        {
            return vec![n as u32];
        }
        return Vec::new();
    }
    let items: Vec<Value> = match body.kind() {
        ValueKind::List(l) => l.iter().cloned().collect(),
        ValueKind::Tuple(t) => t.to_vec(),
        _ => return Vec::new(),
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let ln = interp
            .get_attr(&item, "lineno")
            .ok()
            .and_then(|v| v.as_int())
            .filter(|&n| n > 0)
            .unwrap_or(0) as u32;
        out.push(ln);
    }
    out
}

/// Detect whether the active caller scope has `from __future__ import annotations`
/// active (for `compile(..., dont_inherit=False)`).
fn calling_frame_has_future_annotations(interp: &mut Interpreter) -> bool {
    let globals = interp.active_globals_dict();
    let Some(ann) = globals
        .as_dict()
        .and_then(|d| d.get(&PyKey::str_from("annotations")).cloned())
    else {
        return false;
    };
    if let ValueKind::PyInstance(inst) = ann.kind() {
        let b = inst.borrow();
        if b.class.borrow().name == "_Feature"
            && b.attrs
                .get("compiler_flag")
                .and_then(|v| v.as_int())
                .is_some_and(|f| (f & 0x100_0000) != 0)
        {
            return true;
        }
    }
    false
}
