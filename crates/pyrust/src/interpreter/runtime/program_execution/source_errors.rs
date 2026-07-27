/// Convert a `PyError::Lex` / `PyError::Parse` produced during source parsing
/// into the Python exception CPython raises for it.  The raw message is used
/// directly (the `"Lex error: "` / `"Parse error: "` `Display` prefixes are
/// stripped) so the text matches CPython.  Indentation failures map to the
/// `IndentationError` subclass of `SyntaxError`, matching CPython 3.12's type
/// (e.g. `too many levels of indentation`, issue #2221); everything else is a
/// plain `SyntaxError`.
fn lex_parse_to_exc(e: PyError) -> PyError {
    let msg = match e {
        PyError::Lex(s) | PyError::Parse(s) => s,
        other => other.to_string(),
    };
    if is_indentation_message(&msg) {
        pyrust_core::py_err!("IndentationError", msg)
    } else {
        pyrust_core::py_err!("SyntaxError", msg)
    }
}

/// Whether a lexer/parser error message describes an indentation failure that
/// CPython reports as `IndentationError` rather than a bare `SyntaxError`.
fn is_indentation_message(msg: &str) -> bool {
    msg == "too many levels of indentation"
}

/// Resolve the source-line text for a captured traceback frame (issue #2428).
///
/// CPython echoes the (dedented) source line under *every* `File` header in an
/// uncaught traceback.  The captured-frame snapshot only carries `(filename,
/// lineno)` — recording the text at unwind time would put a per-frame line scan
/// on the hot path — so this runs on the cold print path instead.
///
/// - Frames in the running script (`frame_file == script_file`) resolve from the
///   `script_src` we already hold in memory.
/// - Frames in another file fall back to a `linecache`-style disk read, matching
///   CPython's `traceback` module (which re-reads source from disk).
/// - Pseudo filenames (`<stdin>`, `<string>`, `<unknown>`, …) and the no-line
///   case yield `None`, so the formatter omits the source echo for them.
///
/// The returned text is fully dedented (leading whitespace stripped); the
/// formatter re-indents it to a fixed four spaces, matching CPython.
fn resolve_frame_source_line(
    frame_file: &str,
    lineno: Option<u32>,
    script_file: &str,
    script_src: &str,
) -> Option<std::sync::Arc<str>> {
    let n = lineno? as usize;
    if n == 0 {
        return None;
    }
    // Preserve the line's own leading indentation (strip only trailing ws):
    // `format_traceback` dedents for display and uses the leading-whitespace
    // count to rebase the PEP 657 caret anchor (#2411).  Pre-trimming the start
    // would drop that offset and drop/misplace the caret.
    let pick = |text: &str| -> Option<std::sync::Arc<str>> {
        text.lines()
            .nth(n - 1)
            .map(|l| std::sync::Arc::from(l.trim_end()))
    };
    if frame_file == script_file {
        if script_src.is_empty() {
            return None;
        }
        return pick(script_src);
    }
    // A different file: skip `<…>` pseudo filenames (no real file on disk) and
    // read the line from disk as CPython's linecache would.
    if frame_file.starts_with('<') {
        return None;
    }
    let contents = std::fs::read_to_string(frame_file).ok()?;
    pick(&contents)
}

/// Synthesize the `<string>` traceback frame for an exception raised inside
/// `exec`/`eval`'d code (issue #2245).  CPython reports such errors with a
/// `File "<string>", line N, in <module>` frame, where N is the 1-based line
/// inside the exec'd source.  The inner VM dispatch loop has already recorded
/// the current line into `CURRENT_VM_LINE` (now that the exec'd bytecode
/// carries a `lineno_table`), so read it back and push a module-scope frame at
/// the front of the captured chain.  The frame carries no `source_line`: the
/// exec'd string is not a file, so CPython prints no source text for it.
///
/// Only records on error; the no-error path skips it entirely.
///
/// `filename` is the exec'd code object's `co_filename`: `<string>` for
/// `exec`/`eval` of a raw source string, or the path passed to `compile(...,
/// filename, ...)` when a pre-compiled code object is run (#2438).
fn record_exec_string_frame(
    interp: &Interpreter,
    vm_result: &Result<Value>,
    filename: &std::sync::Arc<str>,
) {
    if vm_result.is_err() {
        let lineno = match pyrust_core::get_current_vm_line() {
            0 => None,
            n => Some(n),
        };
        pyrust_core::record_traceback_frame(pyrust_core::FrameInfo {
            filename: filename.clone(),
            lineno,
            source_line: None,
            funcname: std::sync::Arc::from("<module>"),
            globals: Some(pyrust_core::FrameGlobals::for_environment(&interp.env)),
            col_span: None,
        });
    }
}
