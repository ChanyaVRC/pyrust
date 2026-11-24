// `contextlib` module — context management utilities.
//
// This file is the thin Python-registration facade. Concrete context manager
// implementations, generation tracking, and Python-source injection live in
// submodules under `bodies/contextlib/`.
//
// Reference: <https://docs.python.org/3/library/contextlib.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "contextlib/async_exit_stack.rs"]
mod async_exit_stack;
#[path = "contextlib/decorators.rs"]
mod decorators;
#[path = "contextlib/exit_stack.rs"]
mod exit_stack;
#[path = "contextlib/helpers.rs"]
mod helpers;
#[path = "contextlib/registry.rs"]
mod registry;

pub(crate) use self::async_exit_stack::inject_python_members;
pub(crate) use self::registry::prepare_module_classes;

pyrust_module! {
    /// CPython: contextlib.suppress(*exceptions).
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.suppress>
    class suppress {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            helpers::suppress_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            helpers::noop_method(args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            let _ = _interp;
            helpers::suppress_exit(args, FN_NAME)
        }
    }

    /// Internal context manager produced by `@contextmanager`.
    class _GeneratorContextManager {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            decorators::gen_cm_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            decorators::gen_cm_enter(_interp, args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            decorators::gen_cm_exit(_interp, args, FN_NAME)
        }
    }

    /// CPython: contextlib.contextmanager(func).
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.contextmanager>
    fn contextmanager(args) -> Result<Value> {
        let _ = _interp;
        decorators::contextmanager(args, FN_NAME)
    }

    /// Internal callable returned by `@contextmanager`.
    class _ContextManagerFactory {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            decorators::factory_init(args, FN_NAME)
        }

        fn __call__(args) -> Result<Value> {
            decorators::factory_call(_interp, args, FN_NAME)
        }
    }

    /// CPython: contextlib.closing(thing).
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.closing>
    class closing {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            helpers::closing_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            helpers::closing_enter(args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            helpers::closing_exit(_interp, args, FN_NAME)
        }
    }

    /// CPython: contextlib.nullcontext(enter_result=None).
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.nullcontext>
    class nullcontext {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            helpers::nullcontext_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            helpers::nullcontext_enter(args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            let _ = _interp;
            helpers::noop_method(args, FN_NAME)
        }
    }

    /// CPython: contextlib.chdir(path).
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.chdir>
    class chdir {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            helpers::chdir_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            helpers::chdir_enter(_interp, args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            helpers::chdir_exit(_interp, args, FN_NAME)
        }
    }

    /// CPython: contextlib.redirect_stdout(new_target).
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.redirect_stdout>
    class redirect_stdout {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            helpers::redirect_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            helpers::redirect_enter(_interp, args, FN_NAME, "stdout")
        }

        fn __exit__(args) -> Result<Value> {
            helpers::redirect_exit(_interp, args, FN_NAME, "stdout")
        }
    }

    /// CPython: contextlib.redirect_stderr(new_target).
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.redirect_stderr>
    class redirect_stderr {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            helpers::redirect_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            helpers::redirect_enter(_interp, args, FN_NAME, "stderr")
        }

        fn __exit__(args) -> Result<Value> {
            helpers::redirect_exit(_interp, args, FN_NAME, "stderr")
        }
    }

    /// CPython: contextlib.ExitStack().
    /// <https://docs.python.org/3/library/contextlib.html#contextlib.ExitStack>
    class ExitStack {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            exit_stack::init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            exit_stack::enter(args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            exit_stack::exit(_interp, args, FN_NAME)
        }

        fn enter_context(args) -> Result<Value> {
            exit_stack::enter_context(_interp, args, FN_NAME)
        }

        fn callback(args) -> Result<Value> {
            let _ = _interp;
            exit_stack::callback(args, FN_NAME)
        }

        fn close(args) -> Result<Value> {
            exit_stack::close(_interp, args, FN_NAME)
        }
    }

    /// Internal wrapper for `ExitStack.callback(fn, *args)`.
    class _StackCallback {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            exit_stack::stack_callback_init(args, FN_NAME)
        }

        fn __call__(args) -> Result<Value> {
            exit_stack::stack_callback_call(_interp, args, FN_NAME)
        }
    }
}
