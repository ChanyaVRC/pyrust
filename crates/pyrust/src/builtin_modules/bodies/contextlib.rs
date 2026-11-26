// `contextlib` module — utilities for with-statement contexts (PEP 343).
//
// A 100% native-Rust port of CPython 3.12's `Lib/contextlib.py`: no Python
// source is executed. This file is the thin registration facade; the concrete
// context managers live in submodules under `bodies/contextlib/`:
//
// - `abstract_cms`      `AbstractContextManager` / `AbstractAsyncContextManager`
// - `context_decorator` `ContextDecorator` / `AsyncContextDecorator` and the
//                       callables they produce
// - `decorators`        `contextmanager`, `_GeneratorContextManager`
// - `exit_stack`        `_BaseExitStack`, `ExitStack`
// - `async_exit_stack`  `AsyncExitStack`, `aclosing`,
//                       `asynccontextmanager`'s manager
// - `helpers`           `suppress`, `closing`, `nullcontext`, `chdir`,
//                       `redirect_stdout`, `redirect_stderr`
// - `coroutine`         the native coroutines returned by `async` methods
// - `registry`          generation-local class lookup
//
// Reference: <https://docs.python.org/3/library/contextlib.html>

use crate::builtin_modules::asyncio_rt::awaitable::{
    NativeCoroState, coro_step_to_iter_result, step_native_coro,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

#[path = "contextlib/abstract_cms.rs"]
mod abstract_cms;
#[path = "contextlib/async_exit_stack.rs"]
mod async_exit_stack;
#[path = "contextlib/context_decorator.rs"]
mod context_decorator;
#[path = "contextlib/coroutine.rs"]
mod coroutine;
#[path = "contextlib/decorators.rs"]
mod decorators;
#[path = "contextlib/docs.rs"]
mod docs;
#[path = "contextlib/exit_stack.rs"]
mod exit_stack;
#[path = "contextlib/helpers.rs"]
mod helpers;
#[path = "contextlib/registry.rs"]
mod registry;
#[path = "contextlib/support.rs"]
mod support;

/// Finish one imported generation of the module: class docstrings and the
/// generation registry. Needs no interpreter.
fn prepare_module_generation(module: &Value) {
    let ValueKind::PyModule(namespace) = module.kind() else {
        return;
    };
    for (name, doc) in docs::CLASS_DOCS {
        let class = namespace.borrow().attrs.get(*name).cloned();
        if let Some(class) = class
            && let ValueKind::PyClass(class) = class.kind()
        {
            class
                .borrow_mut()
                .attrs
                .insert("__doc__".to_string(), Value::string(*doc));
        }
    }
    registry::register_module_generation(module);
}

/// Post-load hook for `contextlib`: wire the ABC base classes and register the
/// generation's helper classes.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    abstract_cms::wire_abc_classes(interp, module);
    prepare_module_generation(module);
}

fn all_names() -> Value {
    Value::list(
        [
            "asynccontextmanager",
            "contextmanager",
            "closing",
            "nullcontext",
            "AbstractContextManager",
            "AbstractAsyncContextManager",
            "AsyncExitStack",
            "ContextDecorator",
            "ExitStack",
            "redirect_stdout",
            "redirect_stderr",
            "suppress",
            "aclosing",
            "chdir",
        ]
        .into_iter()
        .map(Value::string)
        .collect(),
    )
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "__all__" => all_names(),
    }

    // ── Abstract base classes ────────────────────────────────────────────────

    class AbstractContextManager {
        /// Return `self` upon entering the runtime context.
        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            Ok(Value::py_instance(support::expect_self(args, FN_NAME)?))
        }

        /// Raise any exception triggered within the runtime context.
        fn __exit__(args) -> Result<Value> {
            let _ = (_interp, support::expect_self(args, FN_NAME)?);
            Ok(Value::none())
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            abstract_cms::subclasshook(
                _interp,
                cls.0,
                subclass.0,
                "AbstractContextManager",
                ["__enter__", "__exit__"],
            )
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] item: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(abstract_cms::class_getitem(cls.0, item.0))
        }
    }

    class AbstractAsyncContextManager {
        /// Return `self` upon entering the runtime context.
        fn __aenter__(args) -> Result<Value> {
            let _ = _interp;
            abstract_cms::async_enter(Value::py_instance(support::expect_self(args, FN_NAME)?))
        }

        /// Raise any exception triggered within the runtime context.
        fn __aexit__(args) -> Result<Value> {
            let _ = (_interp, support::expect_self(args, FN_NAME)?);
            abstract_cms::async_exit()
        }

        #[classmethod]
        fn __subclasshook__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] subclass: PyValueRef<'_>,
        ) -> Result<Value> {
            abstract_cms::subclasshook(
                _interp,
                cls.0,
                subclass.0,
                "AbstractAsyncContextManager",
                ["__aenter__", "__aexit__"],
            )
        }

        #[classmethod]
        fn __class_getitem__(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] item: PyValueRef<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(abstract_cms::class_getitem(cls.0, item.0))
        }
    }

    /// The abstract `__exit__` / `__aexit__` of the ABCs (`@abstractmethod`).
    class _AbstractMethod {
        constants {
            "__isabstractmethod__" => Value::bool_(true),
        }

        fn __call__(args) -> Result<Value> {
            let _ = _interp;
            abstract_cms::abstract_method_call(args, FN_NAME)
        }

        fn __get__(args) -> Result<Value> {
            let _ = _interp;
            Ok(Value::py_instance(support::expect_self(args, FN_NAME)?))
        }
    }

    // ── ContextDecorator / AsyncContextDecorator ─────────────────────────────

    class ContextDecorator {
        /// Return a recreated instance of self.
        fn _recreate_cm(args) -> Result<Value> {
            let _ = _interp;
            context_decorator::recreate_cm(args, FN_NAME)
        }

        fn __call__(args) -> Result<Value> {
            context_decorator::decorate(_interp, args, FN_NAME, false)
        }
    }

    class AsyncContextDecorator {
        /// Return a recreated instance of self.
        fn _recreate_cm(args) -> Result<Value> {
            let _ = _interp;
            context_decorator::recreate_cm(args, FN_NAME)
        }

        fn __call__(args) -> Result<Value> {
            context_decorator::decorate(_interp, args, FN_NAME, true)
        }
    }

    /// The callable `ContextDecorator.__call__` returns (`inner` in CPython).
    class _DecoratedCallable {
        fn __call__(args) -> Result<Value> {
            context_decorator::decorated_call(_interp, args, FN_NAME)
        }

        fn __get__(args) -> Result<Value> {
            let _ = _interp;
            context_decorator::decorated_get(args, FN_NAME)
        }
    }

    // ── @contextmanager / @asynccontextmanager ───────────────────────────────

    class _GeneratorContextManagerBase {
        fn __init__(args) -> Result<Value> {
            decorators::base_init(_interp, args, FN_NAME)
        }

        fn _recreate_cm(args) -> Result<Value> {
            decorators::recreate_cm(_interp, args, FN_NAME)
        }
    }

    class _GeneratorContextManager(
        "_GeneratorContextManagerBase",
        "AbstractContextManager",
        "ContextDecorator",
    ) {
        fn __enter__(args) -> Result<Value> {
            decorators::gen_cm_enter(_interp, args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            decorators::gen_cm_exit(_interp, args, FN_NAME)
        }
    }

    class _AsyncGeneratorContextManager(
        "_GeneratorContextManagerBase",
        "AbstractAsyncContextManager",
        "AsyncContextDecorator",
    ) {
        fn __aenter__(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::gen_cm_aenter(args, FN_NAME)
        }

        fn __aexit__(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::gen_cm_aexit(args, FN_NAME)
        }
    }

    /// The `helper` callable `@contextmanager` / `@asynccontextmanager` return.
    class _ContextManagerFactory {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            decorators::factory_init(args, FN_NAME)
        }

        fn __call__(args) -> Result<Value> {
            decorators::factory_call(_interp, args, FN_NAME)
        }

        fn __get__(args) -> Result<Value> {
            let _ = _interp;
            decorators::factory_get(args, FN_NAME)
        }
    }

    /// @contextmanager decorator.
    fn contextmanager(args) -> Result<Value> {
        let func = support::bind_function_args(args, "contextmanager", &["func"])?.remove(0);
        decorators::make_factory(_interp, func, false)
    }

    /// @asynccontextmanager decorator.
    fn asynccontextmanager(args) -> Result<Value> {
        let func = support::bind_function_args(args, "asynccontextmanager", &["func"])?.remove(0);
        decorators::make_factory(_interp, func, true)
    }

    // ── Simple context managers ──────────────────────────────────────────────

    class closing("AbstractContextManager") {
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

    class aclosing("AbstractAsyncContextManager") {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::aclosing_init(args, FN_NAME)
        }

        fn __aenter__(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::aclosing_aenter(args, FN_NAME)
        }

        fn __aexit__(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::aclosing_aexit(args, FN_NAME)
        }
    }

    class suppress("AbstractContextManager") {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            helpers::suppress_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            helpers::noop_method(args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            helpers::suppress_exit(_interp, args, FN_NAME)
        }
    }

    class _RedirectStream("AbstractContextManager") {
        constants {
            "_stream" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            helpers::redirect_init(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            helpers::redirect_enter(_interp, args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            helpers::redirect_exit(_interp, args, FN_NAME)
        }
    }

    class redirect_stdout("_RedirectStream") {
        constants {
            "_stream" => Value::string("stdout"),
        }
    }

    class redirect_stderr("_RedirectStream") {
        constants {
            "_stream" => Value::string("stderr"),
        }
    }

    class nullcontext("AbstractContextManager", "AbstractAsyncContextManager") {
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

        fn __aenter__(args) -> Result<Value> {
            let _ = _interp;
            helpers::nullcontext_aenter(args, FN_NAME)
        }

        fn __aexit__(args) -> Result<Value> {
            let _ = (_interp, support::expect_self(args, FN_NAME)?);
            coroutine::ready(Value::none())
        }
    }

    class chdir("AbstractContextManager") {
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

    // ── ExitStack / AsyncExitStack ───────────────────────────────────────────

    class _BaseExitStack {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            exit_stack::init(args, FN_NAME)
        }

        /// Preserve the context stack by transferring it to a new instance.
        fn pop_all(args) -> Result<Value> {
            exit_stack::pop_all(_interp, args, FN_NAME)
        }

        /// Registers a callback with the standard __exit__ method signature.
        fn push(args) -> Result<Value> {
            exit_stack::push(_interp, args, FN_NAME)
        }

        /// Enters the supplied context manager.
        fn enter_context(args) -> Result<Value> {
            exit_stack::enter_context(_interp, args, FN_NAME)
        }

        /// Registers an arbitrary callback and arguments.
        fn callback(args) -> Result<Value> {
            exit_stack::callback(_interp, args, FN_NAME, false)
        }

        /// Helper to correctly register callbacks to __exit__ methods.
        fn _push_cm_exit(args) -> Result<Value> {
            exit_stack::push_cm_exit(_interp, args, FN_NAME)
        }

        fn _push_exit_callback(args) -> Result<Value> {
            exit_stack::push_exit_callback_method(_interp, args, FN_NAME)
        }

        #[staticmethod]
        fn _create_exit_wrapper(
            #[positional_only] cm: PyValue,
            #[positional_only] cm_exit: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(exit_stack::create_exit_wrapper(cm.0, cm_exit.0))
        }

        #[staticmethod]
        fn _create_cb_wrapper(
            #[positional_only] callback: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kwds: IndexMap<String, Value>,
        ) -> Result<Value> {
            let _ = _interp;
            exit_stack::create_callback_wrapper(callback.0, args, kwds, false)
        }
    }

    class ExitStack("_BaseExitStack", "AbstractContextManager") {
        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            exit_stack::enter(args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            exit_stack::exit(_interp, args, FN_NAME)
        }

        /// Immediately unwind the context stack.
        fn close(args) -> Result<Value> {
            exit_stack::close(_interp, args, FN_NAME)
        }
    }

    class AsyncExitStack("_BaseExitStack", "AbstractAsyncContextManager") {
        #[staticmethod]
        fn _create_async_exit_wrapper(
            #[positional_only] cm: PyValue,
            #[positional_only] cm_exit: PyValue,
        ) -> Result<Value> {
            let _ = _interp;
            Ok(exit_stack::create_exit_wrapper(cm.0, cm_exit.0))
        }

        #[staticmethod]
        fn _create_async_cb_wrapper(
            #[positional_only] callback: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kwds: IndexMap<String, Value>,
        ) -> Result<Value> {
            let _ = _interp;
            exit_stack::create_callback_wrapper(callback.0, args, kwds, true)
        }

        /// Enters the supplied async context manager.
        fn enter_async_context(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::enter_async_context(args, FN_NAME)
        }

        /// Registers a coroutine function with the standard __aexit__ method signature.
        fn push_async_exit(args) -> Result<Value> {
            async_exit_stack::push_async_exit(_interp, args, FN_NAME)
        }

        /// Registers an arbitrary coroutine function and arguments.
        fn push_async_callback(args) -> Result<Value> {
            exit_stack::callback(_interp, args, FN_NAME, true)
        }

        /// Immediately unwind the context stack.
        fn aclose(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::aclose(args, FN_NAME)
        }

        /// Helper to correctly register coroutine function to __aexit__ method.
        fn _push_async_cm_exit(args) -> Result<Value> {
            async_exit_stack::push_async_cm_exit(_interp, args, FN_NAME)
        }

        fn __aenter__(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::aenter(args, FN_NAME)
        }

        fn __aexit__(args) -> Result<Value> {
            let _ = _interp;
            async_exit_stack::aexit(args, FN_NAME)
        }
    }

    /// Internal wrapper stored in `ExitStack._exit_callbacks`.
    class _StackCallback {
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            exit_stack::stack_callback_init(args, FN_NAME)
        }

        fn __call__(args) -> Result<Value> {
            exit_stack::stack_callback_call(_interp, args, FN_NAME)
        }
    }

    // ── The hidden `coroutine` type returned by the module's async methods ───

    #[native_state(NativeCoroState)]
    class coroutine {
        fn __await__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let step = step_native_coro(_interp, self_val.0, Value::none(), None);
            coro_step_to_iter_result(step)
        }

        fn send(#[positional_only] self_val: PyValueRef<'_>, value: PyValue) -> Result<Value> {
            let step = step_native_coro(_interp, self_val.0, value.0, None);
            coro_step_to_iter_result(step)
        }

        fn throw(
            #[positional_only] self_val: PyValueRef<'_>,
            exc: PyValue,
            #[default(OptionalArg::Omitted)] _val: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _tb: OptValueRef<'_>,
        ) -> Result<Value> {
            let exception = if matches!(exc.0.kind(), ValueKind::PyClass(_)) {
                _interp.call_function_expanded(exc.0, &[])?
            } else {
                exc.0
            };
            let step = step_native_coro(
                _interp,
                self_val.0,
                Value::none(),
                Some(PyError::raised(exception)),
            );
            coro_step_to_iter_result(step)
        }

        fn close(#[state_mut] st: &mut NativeCoroState) -> Result<Value> {
            let _ = _interp;
            st.done = true;
            st.body = None;
            Ok(Value::none())
        }
    }
}
