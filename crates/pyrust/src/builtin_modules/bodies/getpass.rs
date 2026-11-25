// `getpass` module — 100% native Rust implementation (issue #3253).
//
// Provides:
// - `GetPassWarning(UserWarning)`
// - `getpass(prompt='Password: ', stream=None)` — disables terminal echo on
//   interactive TTYs (Unix `termios` / Windows `SetConsoleMode`) and falls back
//   cleanly to `GetPassWarning` + `sys.stdin.readline()` when stdin is
//   redirected or non-TTY, matching CPython 3.12 identically for headless tests.
// - `getuser()` — checks `LOGNAME`, `USER`, `LNAME`, `USERNAME` environment
//   variables (with Unix `getpwuid(getuid())` fallback), matching CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/getpass.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyStr, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

fn write_and_flush(interp: &mut Interpreter, stream: &Value, text: &str) -> Result<()> {
    let write_fn = interp.get_attr(stream, "write")?;
    let _ = interp.call_function_expanded(
        write_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(text),
        }],
    )?;
    if let Ok(flush_fn) = interp.get_attr(stream, "flush") {
        let _ = interp.call_function_expanded(flush_fn, &[])?;
    }
    Ok(())
}

fn is_default_stdin(stdin: &Value) -> bool {
    matches!(
        pyrust_builtins::file::default_stdio_kind(stdin),
        Some(pyrust_builtins::file::StdioKind::Stdin)
    )
}

#[cfg(unix)]
fn try_read_tty_password(
    interp: &mut Interpreter,
    stream: &Value,
    prompt: &str,
) -> Result<Option<String>> {
    use std::io::BufRead;
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 0 {
        return Ok(None);
    }
    let mut old_termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut old_termios) } != 0 {
        return Ok(None);
    }
    let mut no_echo = old_termios;
    no_echo.c_lflag &= !libc::ECHO;
    if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &no_echo) } != 0 {
        return Ok(None);
    }

    let read_res = (|| -> Result<String> {
        write_and_flush(interp, stream, prompt)?;
        let mut line = String::new();
        let stdin = std::io::stdin();
        let n = stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        let _ = write_and_flush(interp, stream, "\n");
        if n == 0 {
            return Err(PyError::named("EOFError", ""));
        }
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(line)
    })();

    unsafe {
        libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &old_termios);
    }
    read_res.map(Some)
}

#[cfg(windows)]
fn try_read_tty_password(
    interp: &mut Interpreter,
    stream: &Value,
    prompt: &str,
) -> Result<Option<String>> {
    use std::io::BufRead;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Ok(None);
    }
    let mut old_mode: u32 = 0;
    if unsafe { GetConsoleMode(handle, &mut old_mode) } == 0 {
        return Ok(None);
    }
    if unsafe { SetConsoleMode(handle, old_mode & !ENABLE_ECHO_INPUT) } == 0 {
        return Ok(None);
    }

    let read_res = (|| -> Result<String> {
        write_and_flush(interp, stream, prompt)?;
        let mut line = String::new();
        let stdin = std::io::stdin();
        let n = stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| PyError::from_io_error(&e, None))?;
        let _ = write_and_flush(interp, stream, "\n");
        if n == 0 {
            return Err(PyError::named("EOFError", ""));
        }
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(line)
    })();

    unsafe {
        SetConsoleMode(handle, old_mode);
    }
    read_res.map(Some)
}

fn emit_getpass_warning(interp: &mut Interpreter) -> Result<()> {
    let warnings_mod = interp.load_module("warnings")?;
    let warn_fn = interp.get_attr(&warnings_mod, "warn")?;
    let getpass_mod = interp.load_module("getpass")?;
    let warn_cls = interp.get_attr(&getpass_mod, "GetPassWarning")?;
    let _ = interp.call_function_expanded(
        warn_fn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string("Can not control echo on the terminal."),
            },
            ExpandedCallArg {
                name: None,
                value: warn_cls,
            },
            ExpandedCallArg {
                name: Some("stacklevel".to_string()),
                value: Value::int(2),
            },
        ],
    )?;
    Ok(())
}

fn fallback_getpass_impl(
    interp: &mut Interpreter,
    prompt: &str,
    stream: &Value,
    stdin: &Value,
) -> Result<Value> {
    emit_getpass_warning(interp)?;
    write_and_flush(interp, stream, "Warning: Password input may be echoed.\n")?;
    if !prompt.is_empty() {
        write_and_flush(interp, stream, prompt)?;
    }
    let readline_fn = interp.get_attr(stdin, "readline")?;
    let line_val = interp.call_function_expanded(readline_fn, &[])?;
    let line_str = match line_val.kind() {
        ValueKind::Str(s) => s.to_string(),
        ValueKind::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "stdin.readline() did not return a string",
            ));
        }
    };
    if line_str.is_empty() {
        return Err(PyError::named("EOFError", ""));
    }
    let trimmed = line_str
        .strip_suffix("\r\n")
        .or_else(|| line_str.strip_suffix('\n'))
        .or_else(|| line_str.strip_suffix('\r'))
        .unwrap_or(&line_str);
    Ok(Value::string(trimmed))
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("getpass"),
            Value::string("getuser"),
            Value::string("GetPassWarning"),
        ]),
    }

    /// Warning raised when password input may be echoed.
    exception GetPassWarning(UserWarning);

    /// Prompt for a password, with echo turned off when possible.
    fn getpass(
        #[default("Password: ".into())] prompt: PyStr<'_>,
        #[default(None)] stream: Option<PyValue>,
    ) -> Result<Value> {
        let sys_mod = _interp.load_module("sys")?;
        let stdin = _interp.get_attr(&sys_mod, "stdin")?;
        let out_stream = match stream {
            Some(v) => v.0,
            None => _interp.get_attr(&sys_mod, "stderr")?,
        };
        if is_default_stdin(&stdin)
            && let Some(pw) = try_read_tty_password(_interp, &out_stream, &prompt)?
        {
            return Ok(Value::string(pw));
        }
        fallback_getpass_impl(_interp, &prompt, &out_stream, &stdin)
    }

    /// Return the login name of the current user.
    fn getuser() -> Result<Value> {
        for var in ["LOGNAME", "USER", "LNAME", "USERNAME"] {
            if let Ok(val) = std::env::var(var)
                && !val.is_empty()
            {
                return Ok(Value::string(val));
            }
        }
        #[cfg(unix)]
        {
            let uid = unsafe { libc::getuid() };
            let pw = unsafe { libc::getpwuid(uid) };
            if !pw.is_null() {
                let name_ptr = unsafe { (*pw).pw_name };
                if !name_ptr.is_null() {
                    let cstr = unsafe { std::ffi::CStr::from_ptr(name_ptr) };
                    return Ok(Value::string(cstr.to_string_lossy()));
                }
            }
            Err(PyError::named("KeyError", format!("getpwuid(): uid not found: {uid}")))
        }
        #[cfg(not(unix))]
        {
            Err(PyError::named(
                "OSError",
                "No username set in the environment",
            ))
        }
    }
}
