//! Process identity, host capabilities, entropy, and terminal queries.

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, reject_keyword_args_expanded};
use crate::value::Value;

use super::arguments::require_int;
use super::result_types::make_terminal_size;

pub(super) fn getpid(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    require_no_args(args, fn_name)?;
    Ok(Value::int(std::process::id() as i64))
}

pub(super) fn getppid(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    require_no_args(args, fn_name)?;
    Ok(Value::int(get_parent_pid()))
}

pub(super) fn cpu_count(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    require_no_args(args, fn_name)?;
    match std::thread::available_parallelism() {
        Ok(count) => Ok(Value::int(count.get() as i64)),
        Err(_) => Ok(Value::none()),
    }
}

pub(super) fn urandom(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let size = require_int(fn_name, &args[0].value, "size")?;
    if size < 0 {
        return Err(PyError::named(
            "ValueError",
            "negative argument not allowed".to_string(),
        ));
    }
    Ok(Value::bytes(os_urandom(size as usize)?))
}

pub(super) fn strerror(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let code = require_int(fn_name, &args[0].value, "code")?;
    let raw = std::io::Error::from_raw_os_error(code as i32).to_string();
    let message = match raw.rfind(" (os error ") {
        Some(index) => raw[..index].to_string(),
        None => raw,
    };
    Ok(Value::string(message))
}

pub(super) fn get_terminal_size(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let columns = positive_env_integer("COLUMNS").unwrap_or(80);
    let lines = positive_env_integer("LINES").unwrap_or(24);
    Ok(make_terminal_size(columns, lines))
}

fn require_no_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments ({} given)", args.len()),
        ))
    }
}

fn positive_env_integer(name: &str) -> Option<i64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|&value| value > 0)
}

/// Read bytes from the platform cryptographically secure random source.
fn os_urandom(size: usize) -> Result<Vec<u8>> {
    let mut buffer = vec![0_u8; size];
    if size == 0 {
        return Ok(buffer);
    }
    getrandom::getrandom(&mut buffer).map_err(|error| match error.raw_os_error() {
        Some(code) => {
            let io_error = std::io::Error::from_raw_os_error(code);
            PyError::from_io_error(&io_error, None)
        }
        None => PyError::named("OSError", error.to_string()),
    })?;
    Ok(buffer)
}

/// Parent process id through the platform primitive.
#[cfg(unix)]
fn get_parent_pid() -> i64 {
    // SAFETY: `getppid` has no arguments or preconditions and always returns
    // the caller's parent process id.
    unsafe { libc::getppid() as i64 }
}

#[cfg(windows)]
fn get_parent_pid() -> i64 {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32, Process32First, Process32Next, TH32CS_SNAPPROCESS,
    };

    let current_pid = std::process::id();
    // SAFETY: the handle is checked before use, `PROCESSENTRY32::dwSize` is
    // initialized as required, and every valid snapshot is closed.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut entry: PROCESSENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32>() as u32;
        let mut parent_pid = 0_i64;
        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32ProcessID == current_pid {
                    parent_pid = entry.th32ParentProcessID as i64;
                    break;
                }
                if Process32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        parent_pid
    }
}

#[cfg(not(any(unix, windows)))]
fn get_parent_pid() -> i64 {
    0
}
