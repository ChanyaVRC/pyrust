// `curses` package — 100% native Rust implementation of Python 3.12's
// `Lib/curses/__init__.py` re-exporting `_curses` with exact object identity
// plus `initscr`, `start_color`, and `wrapper` (issue #3266).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs` to match CPython 3.12.
// Reference: <https://docs.python.org/3.12/library/curses.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::PyCallable;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

const SKIP_REEXPORT: &[&str] = &[
    "__name__",
    "__doc__",
    "__package__",
    "__loader__",
    "__spec__",
    "__file__",
    "__path__",
    "__all__",
    "__cached__",
    "__builtins__",
    "initscr",
    "start_color",
    "wrapper",
];

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let f = interp.get_attr(obj, name)?;
    call_pos(interp, f, args)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let c_mod = interp.load_module("_curses")?;
    if let ValueKind::PyModule(src_rc) = c_mod.kind() {
        let snapshot = src_rc.borrow().attrs_snapshot();
        let mut dst = module.borrow_mut();
        for (k, v) in snapshot {
            if SKIP_REEXPORT.contains(&k.as_str()) {
                continue;
            }
            dst.insert_attr(k, v);
        }
    }
    Ok(())
}

fn do_initscr(interp: &mut Interpreter) -> Result<Value> {
    let c_mod = interp.load_module("_curses")?;
    // Resolve TERM from os.environ (defaulting to "unknown") and stdout fd
    // matching CPython's `Lib/curses/__init__.py`.
    let term = {
        let os_mod = interp.load_module("os")?;
        let environ = interp.get_attr(&os_mod, "environ")?;
        let v = call_method(
            interp,
            &environ,
            "get",
            &[Value::string("TERM"), Value::string("unknown")],
        )?;
        match v.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => "unknown".to_string(),
        }
    };
    let fd_val = {
        let sys_mod = interp.load_module("sys")?;
        if let Ok(stdout) = interp.get_attr(&sys_mod, "__stdout__")
            && let Ok(fd) = call_method(interp, &stdout, "fileno", &[])
        {
            fd
        } else {
            Value::int(1)
        }
    };
    let setupterm_fn = interp.get_attr(&c_mod, "setupterm")?;
    let setup_args = [
        ExpandedCallArg {
            name: Some("term".to_string()),
            value: Value::string(term),
        },
        ExpandedCallArg {
            name: Some("fd".to_string()),
            value: fd_val,
        },
    ];
    interp.call_function_expanded(setupterm_fn, &setup_args)?;

    let initscr_fn = interp.get_attr(&c_mod, "initscr")?;
    call_pos(interp, initscr_fn, &[])
}

fn do_start_color(interp: &mut Interpreter) -> Result<Value> {
    let c_mod = interp.load_module("_curses")?;
    let sc_fn = interp.get_attr(&c_mod, "start_color")?;
    call_pos(interp, sc_fn, &[])
}

pyrust_module! {
    /// Initialize the library and return a window object representing the whole screen.
    fn initscr() -> Result<Value> {
        do_initscr(_interp)
    }

    /// Initialize color support and publish `COLORS` and `COLOR_PAIRS`.
    fn start_color() -> Result<Value> {
        do_start_color(_interp)
    }

    /// Wrapper function that initializes curses, invokes `func(stdscr, *args, **kwds)`,
    /// and restores normal terminal behavior even if an exception is raised.
    fn wrapper(
        #[positional_only] func: PyCallable,
        #[varargs] args: &[Value],
        #[varkwargs] kwds: IndexMap<String, Value>,
    ) -> Result<Value> {
        let c_mod = _interp.load_module("_curses")?;
        let stdscr = do_initscr(_interp)?;

        let setup_res: Result<()> = (|| {
            let noecho_fn = _interp.get_attr(&c_mod, "noecho")?;
            call_pos(_interp, noecho_fn, &[])?;
            let cbreak_fn = _interp.get_attr(&c_mod, "cbreak")?;
            call_pos(_interp, cbreak_fn, &[])?;
            call_method(_interp, &stdscr, "keypad", &[Value::int(1)])?;
            // Ignore start_color() failure on monochrome terminals (matches CPython).
            let _ = do_start_color(_interp);
            Ok(())
        })();

        let call_result = if let Err(err) = setup_res {
            Err(err)
        } else {
            let mut expanded = Vec::with_capacity(1 + args.len() + kwds.len());
            expanded.push(ExpandedCallArg {
                name: None,
                value: stdscr.clone(),
            });
            for a in args {
                expanded.push(ExpandedCallArg {
                    name: None,
                    value: a.clone(),
                });
            }
            for (k, v) in kwds {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(func.into_value(), &expanded)
        };

        // Cleanup mirroring CPython's `finally:` block.
        let _ = call_method(_interp, &stdscr, "keypad", &[Value::int(0)]);
        if let Ok(echo_fn) = _interp.get_attr(&c_mod, "echo") {
            let _ = call_pos(_interp, echo_fn, &[]);
        }
        if let Ok(nocbreak_fn) = _interp.get_attr(&c_mod, "nocbreak") {
            let _ = call_pos(_interp, nocbreak_fn, &[]);
        }
        if let Ok(endwin_fn) = _interp.get_attr(&c_mod, "endwin") {
            let _ = call_pos(_interp, endwin_fn, &[]);
        }

        call_result
    }
}
