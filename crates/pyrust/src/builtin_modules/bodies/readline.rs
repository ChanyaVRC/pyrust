// `readline` module — 100% native Rust implementation of Python 3.12's GNU
// readline interface (issue #3266).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs` to match CPython 3.12
// (where `import readline` raises `ModuleNotFoundError` on Windows).
// Reference: <https://docs.python.org/3.12/library/readline.html>

use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyCallable, PyIndex, PyPath, PyStr, PyValue};
use crate::value::Value;

const DEFAULT_COMPLETER_DELIMS: &str = " \t\n`~!@#$%^&*()-=+[{]}\\|;:'\",<>/?";

struct ReadlineState {
    history: Vec<String>,
    history_length: i64,
    auto_history: bool,
    completer: Option<Value>,
    completer_delims: String,
    completion_type: i64,
    begidx: i64,
    endidx: i64,
    completion_display_matches_hook: Option<Value>,
    startup_hook: Option<Value>,
    pre_input_hook: Option<Value>,
    line_buffer: String,
    bindings: Vec<String>,
}

impl Default for ReadlineState {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            history_length: -1,
            auto_history: true,
            completer: None,
            completer_delims: DEFAULT_COMPLETER_DELIMS.to_string(),
            completion_type: 0,
            begidx: 0,
            endidx: 0,
            completion_display_matches_hook: None,
            startup_hook: None,
            pre_input_hook: None,
            line_buffer: String::new(),
            bindings: Vec::new(),
        }
    }
}

thread_local! {
    static READLINE_STATE: RefCell<ReadlineState> = RefCell::new(ReadlineState::default());
}

/// Record a line entered in PyRust's `rustyline`-powered REPL when
/// `auto_history` is enabled.
#[allow(dead_code)]
pub(crate) fn record_repl_history(line: &str) {
    READLINE_STATE.with(|cell| {
        let mut st = cell.borrow_mut();
        if st.auto_history && !line.is_empty() {
            st.history.push(line.to_string());
        }
    });
}

fn default_history_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".history");
        p
    } else {
        PathBuf::from(".history")
    }
}

fn default_init_path() -> PathBuf {
    if let Some(inputrc) = std::env::var_os("INPUTRC")
        && !inputrc.is_empty()
    {
        return PathBuf::from(inputrc);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".inputrc");
        p
    } else {
        PathBuf::from(".inputrc")
    }
}

fn resolve_optional_path(filename: Option<PyPath>, default_fn: fn() -> PathBuf) -> PathBuf {
    match filename {
        Some(p) => p.into_path_buf(),
        None => default_fn(),
    }
}

fn parse_history_bytes(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s).to_string())
        .collect();
    // Drop the empty trailing element caused by a terminating '\n'.
    if bytes.ends_with(b"\n") {
        lines.pop();
    }
    lines
}

fn truncate_history_file(path: &Path, max_lines: i64) -> Result<()> {
    if max_lines < 0 {
        return Ok(());
    }
    let max_n = max_lines as usize;
    let bytes =
        fs::read(path).map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
    let lines = parse_history_bytes(&bytes);
    if lines.len() <= max_n {
        return Ok(());
    }
    let keep = &lines[lines.len() - max_n..];
    let mut out = String::new();
    for line in keep {
        out.push_str(line);
        out.push('\n');
    }
    fs::write(path, out.as_bytes())
        .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
    Ok(())
}

fn opt_callable_to_value(func: Option<PyCallable>) -> Option<Value> {
    func.map(PyCallable::into_value)
}

pyrust_module! {
    constants {
        "_READLINE_VERSION" => Value::int(0x0802),
        "_READLINE_RUNTIME_VERSION" => Value::int(0x0802),
        "_READLINE_LIBRARY_VERSION" => Value::string("8.2"),
        "backend" => Value::string("readline"),
    }

    /// Execute the init line provided in the string argument.
    fn parse_and_bind(#[positional_only] string: PyStr<'_>) -> Result<Value> {
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().bindings.push(string.0.to_string());
        });
        Ok(Value::none())
    }

    /// Execute a readline initialization file.
    fn read_init_file(
        #[positional_only]
        #[default(None)]
        filename: Option<PyPath>,
    ) -> Result<Value> {
        let path = resolve_optional_path(filename, default_init_path);
        let bytes = fs::read(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
        let text = String::from_utf8_lossy(&bytes);
        READLINE_STATE.with(|cell| {
            let mut st = cell.borrow_mut();
            for line in text.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    st.bindings.push(trimmed.to_string());
                }
            }
        });
        Ok(Value::none())
    }

    /// Return the current contents of the line buffer.
    fn get_line_buffer() -> Result<Value> {
        let buf = READLINE_STATE.with(|cell| cell.borrow().line_buffer.clone());
        Ok(Value::string(buf))
    }

    /// Insert text into the line buffer at the cursor position.
    fn insert_text(#[positional_only] string: PyStr<'_>) -> Result<Value> {
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().line_buffer.push_str(&string.0);
        });
        Ok(Value::none())
    }

    /// Change what's displayed on the screen to reflect the current contents
    /// of the line buffer.
    fn redisplay() -> Result<Value> {
        Ok(Value::none())
    }

    /// Load a readline history file.
    fn read_history_file(
        #[positional_only]
        #[default(None)]
        filename: Option<PyPath>,
    ) -> Result<Value> {
        let path = resolve_optional_path(filename, default_history_path);
        let bytes = fs::read(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
        let lines = parse_history_bytes(&bytes);
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().history.extend(lines);
        });
        Ok(Value::none())
    }

    /// Save a readline history file.
    fn write_history_file(
        #[positional_only]
        #[default(None)]
        filename: Option<PyPath>,
    ) -> Result<Value> {
        let path = resolve_optional_path(filename, default_history_path);
        let (lines, max_len) = READLINE_STATE.with(|cell| {
            let st = cell.borrow();
            (st.history.clone(), st.history_length)
        });
        let slice: &[String] = if max_len >= 0 && (max_len as usize) < lines.len() {
            &lines[lines.len() - (max_len as usize)..]
        } else {
            &lines[..]
        };
        let mut out = String::new();
        for line in slice {
            out.push_str(line);
            out.push('\n');
        }
        fs::write(&path, out.as_bytes())
            .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
        Ok(Value::none())
    }

    /// Append the last `nelements` items of the history list to a file.
    fn append_history_file(
        #[positional_only] nelements: PyIndex,
        #[positional_only]
        #[default(None)]
        filename: Option<PyPath>,
    ) -> Result<Value> {
        let count = nelements.expect_i64(FN_NAME, "nelements")?;
        if count < 0 {
            return Err(PyError::named(
                "ValueError",
                "nelements must be non-negative".to_string(),
            ));
        }
        let path = resolve_optional_path(filename, default_history_path);
        let (to_append, max_len) = READLINE_STATE.with(|cell| {
            let st = cell.borrow();
            let n = (count as usize).min(st.history.len());
            let start = st.history.len() - n;
            (st.history[start..].to_vec(), st.history_length)
        });
        let mut file = OpenOptions::new()
            .write(true)
            .append(true)
            .open(&path)
            .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
        let mut out = String::new();
        for line in &to_append {
            out.push_str(line);
            out.push('\n');
        }
        file.write_all(out.as_bytes())
            .map_err(|e| PyError::from_io_error(&e, Some(&path.to_string_lossy())))?;
        drop(file);
        truncate_history_file(&path, max_len)?;
        Ok(Value::none())
    }

    /// Return the current (length) of history saved to the history file.
    fn get_history_length() -> Result<Value> {
        let len = READLINE_STATE.with(|cell| cell.borrow().history_length);
        Ok(Value::int(len))
    }

    /// Set the maximal number of lines which will be written to the history file.
    fn set_history_length(#[positional_only] length: PyIndex) -> Result<Value> {
        let len = length.expect_i64(FN_NAME, "length")?;
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().history_length = len;
        });
        Ok(Value::none())
    }

    /// Clear the current history.
    fn clear_history() -> Result<Value> {
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().history.clear();
        });
        Ok(Value::none())
    }

    /// Return the number of lines currently in the history.
    fn get_current_history_length() -> Result<Value> {
        let len = READLINE_STATE.with(|cell| cell.borrow().history.len() as i64);
        Ok(Value::int(len))
    }

    /// Return the current contents of history item at `index` (1-based).
    fn get_history_item(#[positional_only] index: PyIndex) -> Result<Value> {
        let idx = index.expect_i64(FN_NAME, "index")?;
        if idx <= 0 {
            return Ok(Value::none());
        }
        let item = READLINE_STATE.with(|cell| {
            let st = cell.borrow();
            st.history.get((idx - 1) as usize).cloned()
        });
        Ok(match item {
            Some(s) => Value::string(s),
            None => Value::none(),
        })
    }

    /// Remove history item specified by its 0-based position from history.
    fn remove_history_item(#[positional_only] pos: PyIndex) -> Result<Value> {
        let p = pos.expect_i64(FN_NAME, "pos")?;
        if p < 0 {
            return Err(PyError::named(
                "ValueError",
                "History index cannot be negative".to_string(),
            ));
        }
        READLINE_STATE.with(|cell| {
            let mut st = cell.borrow_mut();
            if (p as usize) >= st.history.len() {
                return Err(PyError::named(
                    "ValueError",
                    format!("No history item at position {p}"),
                ));
            }
            st.history.remove(p as usize);
            Ok(Value::none())
        })
    }

    /// Replace history item specified by its 0-based position with `line`.
    fn replace_history_item(
        #[positional_only] pos: PyIndex,
        #[positional_only] line: PyStr<'_>,
    ) -> Result<Value> {
        let p = pos.expect_i64(FN_NAME, "pos")?;
        if p < 0 {
            return Err(PyError::named(
                "ValueError",
                "History index cannot be negative".to_string(),
            ));
        }
        READLINE_STATE.with(|cell| {
            let mut st = cell.borrow_mut();
            if (p as usize) >= st.history.len() {
                return Err(PyError::named(
                    "ValueError",
                    format!("No history item at position {p}"),
                ));
            }
            st.history[p as usize] = line.0.to_string();
            Ok(Value::none())
        })
    }

    /// Add an item to the history buffer.
    fn add_history(#[positional_only] string: PyStr<'_>) -> Result<Value> {
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().history.push(string.0.to_string());
        });
        Ok(Value::none())
    }

    /// Enable or disable automatic calls to `add_history()` when reading input.
    fn set_auto_history(#[positional_only] enabled: PyValue) -> Result<Value> {
        let flag = _interp.truthy_value(&enabled.0)?;
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().auto_history = flag;
        });
        Ok(Value::none())
    }

    /// Set or remove the startup_hook function.
    fn set_startup_hook(
        #[positional_only]
        #[default(None)]
        function: Option<PyCallable>,
    ) -> Result<Value> {
        let hook = opt_callable_to_value(function);
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().startup_hook = hook;
        });
        Ok(Value::none())
    }

    /// Set or remove the pre_input_hook function.
    fn set_pre_input_hook(
        #[positional_only]
        #[default(None)]
        function: Option<PyCallable>,
    ) -> Result<Value> {
        let hook = opt_callable_to_value(function);
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().pre_input_hook = hook;
        });
        Ok(Value::none())
    }

    /// Set or remove the completer function.
    fn set_completer(
        #[positional_only]
        #[default(None)]
        function: Option<PyCallable>,
    ) -> Result<Value> {
        let hook = opt_callable_to_value(function);
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().completer = hook;
        });
        Ok(Value::none())
    }

    /// Get the completer function.
    fn get_completer() -> Result<Value> {
        let c = READLINE_STATE.with(|cell| cell.borrow().completer.clone());
        Ok(c.unwrap_or_else(Value::none))
    }

    /// Get the type of completion being attempted.
    fn get_completion_type() -> Result<Value> {
        let t = READLINE_STATE.with(|cell| cell.borrow().completion_type);
        Ok(Value::int(t))
    }

    /// Get the beginning index of the completion scope.
    fn get_begidx() -> Result<Value> {
        let v = READLINE_STATE.with(|cell| cell.borrow().begidx);
        Ok(Value::int(v))
    }

    /// Get the ending index of the completion scope.
    fn get_endidx() -> Result<Value> {
        let v = READLINE_STATE.with(|cell| cell.borrow().endidx);
        Ok(Value::int(v))
    }

    /// Set the word delimiters for completion.
    fn set_completer_delims(#[positional_only] string: PyStr<'_>) -> Result<Value> {
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().completer_delims = string.0.to_string();
        });
        Ok(Value::none())
    }

    /// Get the word delimiters for completion.
    fn get_completer_delims() -> Result<Value> {
        let d = READLINE_STATE.with(|cell| cell.borrow().completer_delims.clone());
        Ok(Value::string(d))
    }

    /// Set or remove the completion display function.
    fn set_completion_display_matches_hook(
        #[positional_only]
        #[default(None)]
        function: Option<PyCallable>,
    ) -> Result<Value> {
        let hook = opt_callable_to_value(function);
        READLINE_STATE.with(|cell| {
            cell.borrow_mut().completion_display_matches_hook = hook;
        });
        Ok(Value::none())
    }
}
