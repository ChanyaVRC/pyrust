/// Convert a `PyError::Lex` / `PyError::Parse` produced during source parsing
/// into the Python exception CPython raises for it.  The raw message is used
/// directly (the `"Lex error: "` / `"Parse error: "` `Display` prefixes are
/// stripped) so the text matches CPython.  Indentation failures map to the
/// `IndentationError` subclass of `SyntaxError`, matching CPython 3.12's type
/// (e.g. `too many levels of indentation`, issue #2221); everything else is a
/// plain `SyntaxError`.
///
/// This variant has no source to resolve a location against (REPL input, MIR
/// lowering), so a located [`pyrust_core::PyErrorKind::Syntax`] error drops
/// its location; [`resolve_syntax_error`] keeps it.
pub(crate) fn lex_parse_to_exc(e: PyError) -> PyError {
    let msg = match e.into_kind() {
        pyrust_core::PyErrorKind::Lex(s) | pyrust_core::PyErrorKind::Parse(s) => s,
        pyrust_core::PyErrorKind::Syntax(info) => info.msg,
        other => return PyError::new(other),
    };
    match syntax_error_class(&msg) {
        "TabError" => pyrust_core::py_err!("TabError", msg),
        "IndentationError" => pyrust_core::py_err!("IndentationError", msg),
        _ => pyrust_core::py_err!("SyntaxError", msg),
    }
}

/// The `SyntaxError` subclass CPython raises for a lexer/parser message.
fn syntax_error_class(msg: &str) -> &'static str {
    if msg == "inconsistent use of tabs and spaces in indentation" {
        "TabError"
    } else if is_indentation_message(msg) {
        "IndentationError"
    } else {
        "SyntaxError"
    }
}

/// Whether a lexer/parser error message describes an indentation failure that
/// CPython reports as `IndentationError` rather than a bare `SyntaxError`.
fn is_indentation_message(msg: &str) -> bool {
    matches!(
        msg,
        "too many levels of indentation"
            | "unindent does not match any outer indentation level"
            | "inconsistent indentation"
    )
}

/// The `compile()` mode a source string is parsed in, which decides two of
/// CPython's syntax-error location rules (#3402): only `exec` input gets a
/// newline appended before tokenizing, and only `exec` errors read their
/// `text` back from the named file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceMode {
    Exec,
    Eval,
    Single,
}

/// The source a lexer/parser error is resolved against (#3402).
#[derive(Debug, Clone, Copy)]
pub(crate) struct SyntaxErrorSource<'a> {
    /// The text CPython's tokenizer sees.
    pub(crate) source: &'a str,
    pub(crate) filename: &'a str,
    pub(crate) mode: SourceMode,
    /// The part of `source` pyrust stripped before lexing (`eval` trims its
    /// input): the parsed text's lines shift down by the newlines in it, and
    /// its first line shifts right by the chars after its last newline.
    pub(crate) parsed_prefix: &'a str,
}

/// Resolve a lexer/parser error into the `SyntaxError` (or `IndentationError`
/// / `TabError`) CPython 3.12 raises for it, with `filename`, `lineno`,
/// `offset`, `text`, `end_lineno` and `end_offset` populated (#3402).
///
/// Lex/parse errors without a recorded location keep the old location-less
/// form; every other error passes through unchanged.
pub(crate) fn resolve_syntax_error(e: PyError, src: SyntaxErrorSource<'_>) -> PyError {
    let mut info = match e.into_kind() {
        pyrust_core::PyErrorKind::Syntax(info) => info,
        other => return lex_parse_to_exc(PyError::new(other)),
    };
    if info.class_name == "SyntaxError" {
        info.class_name = syntax_error_class(&info.msg);
    }
    let Some(pyrust_core::SyntaxErrorPending::Text {
        rule,
        newline_token,
    }) = info.pending.take()
    else {
        return lex_parse_to_exc(PyError::new(pyrust_core::PyErrorKind::Syntax(info)));
    };

    let (line_shift, col_shift) = prefix_shift(src.parsed_prefix);
    let shift = |lineno: Option<i64>, offset: Option<i64>| match (lineno, offset) {
        (Some(1), Some(offset)) if offset > 0 => (Some(1 + line_shift), Some(offset + col_shift)),
        (Some(lineno), offset) => (Some(lineno + line_shift), offset),
        other => other,
    };
    (info.lineno, info.offset) = shift(info.lineno, info.offset);
    (info.end_lineno, info.end_offset) = shift(info.end_lineno, info.end_offset);

    let lines: Vec<&str> = src.source.split_inclusive('\n').collect();
    let lineno = info.lineno.unwrap_or(0);
    let line = usize::try_from(lineno)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| lines.get(i).copied());
    // Only `exec` input gets a newline appended, so in `eval` / `single` mode
    // an error on the implicit NEWLINE at the end of the input has no column.
    if newline_token && src.mode != SourceMode::Exec && !line.is_some_and(|l| l.ends_with('\n')) {
        info.offset = Some(0);
        info.end_offset = Some(0);
    }
    let without_newline = |l: &str| l.trim_end_matches('\n').trim_end_matches('\r').to_string();
    info.text = Some(match rule {
        pyrust_core::SyntaxTextRule::TokenizerLine => line.map(without_newline).unwrap_or_default(),
        pyrust_core::SyntaxTextRule::ParserLine { tokenizer_lineno } => {
            let tokenizer_lineno = match tokenizer_lineno {
                0 => lines.len() as i64,
                n => i64::from(n) + line_shift,
            };
            let from_file = (src.mode == SourceMode::Exec)
                .then(|| program_text_line(src.filename, lineno))
                .flatten();
            match (from_file, line) {
                (Some(text), _) => text,
                (None, Some(l)) if lineno >= tokenizer_lineno => {
                    let mut text = without_newline(l);
                    if l.ends_with('\n') || src.mode == SourceMode::Exec {
                        text.push('\n');
                    }
                    text
                }
                (None, Some(l)) => without_newline(l),
                (None, None) => String::new(),
            }
        }
    });
    info.filename = Some(src.filename.to_string());
    PyError::new(pyrust_core::PyErrorKind::Syntax(info))
}

/// `(line shift, first-line column shift)` for text that followed `prefix`.
fn prefix_shift(prefix: &str) -> (i64, i64) {
    let line_shift = prefix.matches('\n').count() as i64;
    let tail = prefix.rsplit('\n').next().unwrap_or("");
    (line_shift, tail.chars().count() as i64)
}

/// Line `lineno` (with its newline) of the file `filename`, as CPython's
/// `_PyErr_ProgramDecodedTextObject` reads it for a parser error in `exec`
/// mode: universal newlines, a UTF-8 BOM dropped on line 1, undecodable bytes
/// replaced.  `None` for pseudo filenames (`<string>`), unreadable files and
/// lines past the end — the caller then falls back to the source buffer.
fn program_text_line(filename: &str, lineno: i64) -> Option<String> {
    if filename.starts_with('<') || lineno < 1 {
        return None;
    }
    let bytes = std::fs::read(filename).ok()?;
    let contents = String::from_utf8_lossy(&bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let line = contents.split_inclusive('\n').nth((lineno - 1) as usize)?;
    let line = if lineno == 1 {
        line.strip_prefix('\u{feff}').unwrap_or(line)
    } else {
        line
    };
    Some(line.to_string())
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
pub(crate) fn resolve_frame_source_line(
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
            locals: None,
            frame: None,
        });
    }
}
