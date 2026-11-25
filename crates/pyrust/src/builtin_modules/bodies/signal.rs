// `signal` module — body for the `signal @inject` entry in
// `pyrust_builtin_modules!` (issue #3209).
//
// Native platform signal tables and `strsignal` bindings back the CPython 3.12
// `signal` surface (`Signals`, `Handlers`, `SIG_DFL`, `SIG_IGN`, `NSIG`,
// `default_int_handler`, `getsignal`, `signal`, `valid_signals`, `strsignal`)
// injected from `signal_py.py`.
//
// Reference: <https://docs.python.org/3.12/library/signal.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyDict, PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `signal` module.
const SIGNAL_PY_SOURCE: &str = include_str!("signal_py.py");

/// Private native helpers moved into the exec namespace and hidden from
/// `dir(signal)`.
const SIGNAL_PRIVATE_HELPERS: &[&str] = &[
    "_platform_signals",
    "_platform_nsig",
    "_platform_valid_signals",
    "_platform_strsignal",
    "_platform_is_posix",
    "_platform_uncatchable_signals",
];

/// Fixed public attributes exported onto the `signal` module (in addition to
/// the dynamically populated platform signal names from `_platform_signals`).
const SIGNAL_FIXED_EXPORTS: &[&str] = &[
    "Signals",
    "Handlers",
    "SIG_DFL",
    "SIG_IGN",
    "NSIG",
    "default_int_handler",
    "getsignal",
    "signal",
    "valid_signals",
    "strsignal",
];

/// Execute `SIGNAL_PY_SOURCE` once and publish `Signals`, `Handlers`, all
/// platform signal constants, and the signal API onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for &helper in SIGNAL_PRIVATE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        SIGNAL_PY_SOURCE,
        "<frozen signal>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("signal: exec namespace not a dict".into()))?;
    for &name in SIGNAL_FIXED_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("signal"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    // Also export every platform signal constant (SIGINT, SIGTERM, ...).
    for &(sig_name, _) in platform_signal_table() {
        if let Some(val) = dict.get(&PyKey::str_from(sig_name)) {
            module
                .borrow_mut()
                .attrs
                .insert(sig_name.to_string(), val.clone());
        }
    }
    drop(dict);
    Ok(Some(ns))
}

#[cfg(windows)]
fn platform_signal_table() -> &'static [(&'static str, i64)] {
    &[
        ("CTRL_C_EVENT", 0),
        ("CTRL_BREAK_EVENT", 1),
        ("SIGINT", 2),
        ("SIGILL", 4),
        ("SIGFPE", 8),
        ("SIGSEGV", 11),
        ("SIGTERM", 15),
        ("SIGBREAK", 21),
        ("SIGABRT", 22),
    ]
}

#[cfg(target_os = "linux")]
fn platform_signal_table() -> &'static [(&'static str, i64)] {
    &[
        ("SIGHUP", libc::SIGHUP as i64),
        ("SIGINT", libc::SIGINT as i64),
        ("SIGQUIT", libc::SIGQUIT as i64),
        ("SIGILL", libc::SIGILL as i64),
        ("SIGTRAP", libc::SIGTRAP as i64),
        ("SIGABRT", libc::SIGABRT as i64),
        ("SIGIOT", libc::SIGABRT as i64),
        ("SIGBUS", libc::SIGBUS as i64),
        ("SIGFPE", libc::SIGFPE as i64),
        ("SIGKILL", libc::SIGKILL as i64),
        ("SIGUSR1", libc::SIGUSR1 as i64),
        ("SIGSEGV", libc::SIGSEGV as i64),
        ("SIGUSR2", libc::SIGUSR2 as i64),
        ("SIGPIPE", libc::SIGPIPE as i64),
        ("SIGALRM", libc::SIGALRM as i64),
        ("SIGTERM", libc::SIGTERM as i64),
        ("SIGSTKFLT", libc::SIGSTKFLT as i64),
        ("SIGCHLD", libc::SIGCHLD as i64),
        ("SIGCLD", libc::SIGCHLD as i64),
        ("SIGCONT", libc::SIGCONT as i64),
        ("SIGSTOP", libc::SIGSTOP as i64),
        ("SIGTSTP", libc::SIGTSTP as i64),
        ("SIGTTIN", libc::SIGTTIN as i64),
        ("SIGTTOU", libc::SIGTTOU as i64),
        ("SIGURG", libc::SIGURG as i64),
        ("SIGXCPU", libc::SIGXCPU as i64),
        ("SIGXFSZ", libc::SIGXFSZ as i64),
        ("SIGVTALRM", libc::SIGVTALRM as i64),
        ("SIGPROF", libc::SIGPROF as i64),
        ("SIGWINCH", libc::SIGWINCH as i64),
        ("SIGIO", libc::SIGIO as i64),
        ("SIGPOLL", libc::SIGIO as i64),
        ("SIGPWR", libc::SIGPWR as i64),
        ("SIGSYS", libc::SIGSYS as i64),
        ("SIGRTMIN", 34),
        ("SIGRTMAX", 64),
    ]
}

#[cfg(target_os = "macos")]
fn platform_signal_table() -> &'static [(&'static str, i64)] {
    &[
        ("SIGHUP", libc::SIGHUP as i64),
        ("SIGINT", libc::SIGINT as i64),
        ("SIGQUIT", libc::SIGQUIT as i64),
        ("SIGILL", libc::SIGILL as i64),
        ("SIGTRAP", libc::SIGTRAP as i64),
        ("SIGABRT", libc::SIGABRT as i64),
        ("SIGIOT", libc::SIGABRT as i64),
        ("SIGEMT", libc::SIGEMT as i64),
        ("SIGFPE", libc::SIGFPE as i64),
        ("SIGKILL", libc::SIGKILL as i64),
        ("SIGBUS", libc::SIGBUS as i64),
        ("SIGSEGV", libc::SIGSEGV as i64),
        ("SIGSYS", libc::SIGSYS as i64),
        ("SIGPIPE", libc::SIGPIPE as i64),
        ("SIGALRM", libc::SIGALRM as i64),
        ("SIGTERM", libc::SIGTERM as i64),
        ("SIGURG", libc::SIGURG as i64),
        ("SIGSTOP", libc::SIGSTOP as i64),
        ("SIGTSTP", libc::SIGTSTP as i64),
        ("SIGCONT", libc::SIGCONT as i64),
        ("SIGCHLD", libc::SIGCHLD as i64),
        ("SIGTTIN", libc::SIGTTIN as i64),
        ("SIGTTOU", libc::SIGTTOU as i64),
        ("SIGIO", libc::SIGIO as i64),
        ("SIGXCPU", libc::SIGXCPU as i64),
        ("SIGXFSZ", libc::SIGXFSZ as i64),
        ("SIGVTALRM", libc::SIGVTALRM as i64),
        ("SIGPROF", libc::SIGPROF as i64),
        ("SIGWINCH", libc::SIGWINCH as i64),
        ("SIGINFO", libc::SIGINFO as i64),
        ("SIGUSR1", libc::SIGUSR1 as i64),
        ("SIGUSR2", libc::SIGUSR2 as i64),
    ]
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn platform_signal_table() -> &'static [(&'static str, i64)] {
    &[
        ("SIGHUP", libc::SIGHUP as i64),
        ("SIGINT", libc::SIGINT as i64),
        ("SIGQUIT", libc::SIGQUIT as i64),
        ("SIGILL", libc::SIGILL as i64),
        ("SIGABRT", libc::SIGABRT as i64),
        ("SIGFPE", libc::SIGFPE as i64),
        ("SIGKILL", libc::SIGKILL as i64),
        ("SIGSEGV", libc::SIGSEGV as i64),
        ("SIGPIPE", libc::SIGPIPE as i64),
        ("SIGALRM", libc::SIGALRM as i64),
        ("SIGTERM", libc::SIGTERM as i64),
        ("SIGUSR1", libc::SIGUSR1 as i64),
        ("SIGUSR2", libc::SIGUSR2 as i64),
        ("SIGCHLD", libc::SIGCHLD as i64),
        ("SIGCONT", libc::SIGCONT as i64),
        ("SIGSTOP", libc::SIGSTOP as i64),
    ]
}

#[cfg(not(any(unix, windows)))]
fn platform_signal_table() -> &'static [(&'static str, i64)] {
    &[
        ("SIGINT", 2),
        ("SIGILL", 4),
        ("SIGABRT", 6),
        ("SIGFPE", 8),
        ("SIGSEGV", 11),
        ("SIGTERM", 15),
    ]
}

fn platform_nsig() -> i64 {
    #[cfg(windows)]
    {
        23
    }
    #[cfg(target_os = "linux")]
    {
        65
    }
    #[cfg(target_os = "macos")]
    {
        32
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        32
    }
}

fn platform_valid_signal_ints() -> Vec<i64> {
    #[cfg(windows)]
    {
        vec![2, 4, 8, 11, 15, 21, 22]
    }
    #[cfg(target_os = "linux")]
    {
        // Linux glibc reserves signals 32 and 33 for NPTL; valid signals are
        // 1..=31 and SIGRTMIN(34)..=SIGRTMAX(64), matching CPython 3.12's
        // `sigfillset()` enumeration on Linux.
        let mut out: Vec<i64> = (1..=31).collect();
        out.extend(34..=64);
        out
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        (1..platform_nsig()).collect()
    }
    #[cfg(not(any(unix, windows)))]
    {
        vec![2, 4, 6, 8, 11, 15]
    }
}

fn platform_uncatchable_ints() -> Vec<i64> {
    #[cfg(unix)]
    {
        vec![libc::SIGKILL as i64, libc::SIGSTOP as i64]
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

fn platform_strsignal_opt(sig: i64) -> Option<String> {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        // SAFETY: `sig` has already been range-checked (`1 <= sig < NSIG`);
        // `libc::strsignal` returns a valid NUL-terminated C string or NULL.
        let ptr = unsafe { libc::strsignal(sig as libc::c_int) };
        if ptr.is_null() {
            None
        } else {
            Some(
                unsafe { CStr::from_ptr(ptr) }
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }
    #[cfg(windows)]
    {
        // Exact CPython 3.12.3 Windows mapping (`Modules/signalmodule.c`).
        match sig {
            2 => Some("Interrupt".to_string()),
            4 => Some("Illegal instruction".to_string()),
            8 => Some("Floating point exception".to_string()),
            11 => Some("Segmentation fault".to_string()),
            15 => Some("Terminated".to_string()),
            22 => Some("Aborted".to_string()),
            _ => None,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        match sig {
            2 => Some("Interrupt".to_string()),
            4 => Some("Illegal instruction".to_string()),
            6 => Some("Aborted".to_string()),
            8 => Some("Floating-point exception".to_string()),
            11 => Some("Segmentation fault".to_string()),
            15 => Some("Terminated".to_string()),
            _ => None,
        }
    }
}

pyrust_module! {
    fn _platform_signals(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        let mut dict = PyDict::default();
        for &(name, num) in platform_signal_table() {
            dict.insert(PyKey::str_from(name), Value::int(num));
        }
        Ok(Value::dict(dict))
    }

    fn _platform_nsig(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        Ok(Value::int(platform_nsig()))
    }

    fn _platform_valid_signals(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        Ok(Value::list(
            platform_valid_signal_ints()
                .into_iter()
                .map(Value::int)
                .collect(),
        ))
    }

    fn _platform_uncatchable_signals(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        Ok(Value::list(
            platform_uncatchable_ints()
                .into_iter()
                .map(Value::int)
                .collect(),
        ))
    }

    fn _platform_is_posix(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        Ok(Value::bool_(cfg!(unix)))
    }

    fn _platform_strsignal(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        let sig = match args.first().map(|a| a.value.kind()) {
            Some(ValueKind::Int(n)) => n,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "strsignal() argument must be an integer".to_string(),
                ));
            }
        };
        Ok(match platform_strsignal_opt(sig) {
            Some(s) => Value::string(s),
            None => Value::none(),
        })
    }
}
