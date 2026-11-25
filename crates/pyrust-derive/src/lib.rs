//! `pyrust-derive` — proc-macros for declaring built-in Python modules in Rust.
//!
//! Two entry points:
//!
//! - **`pyrust_module! { … }`** (function-like, primary) — declares a
//!   whole built-in module's content: `constants`, `exception` subclasses,
//!   top-level `fn`s (including type-based overloads and variadics), and
//!   `class` blocks (inheritance, properties, class/static methods, class
//!   constants, overloads, and `#[native_state(T)]`).  The **module's
//!   Python-level name is *not* given here**; the macro reads it from a
//!   sibling `MODULE_NAME: &str` constant that the surrounding
//!   `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs` injects.
//!
//! - **`#[pyfunction(name = "module.fn")]`** (attribute, one-off fallback)
//!   — drop-in for moving a single arm of the legacy cascade incrementally.
//!
//! See `crates/pyrust/src/builtin_modules/NATIVE_MACROS.md` for the full
//! idiomatic reference guide (including `pyrust_struct_seq!`,
//! `pyrust_int_enum!`, `pyrust_int_flag!`, and `FromValue` protocol wrappers).
//!
//! ## `pyrust_module!` quick syntax
//!
//! ```ignore
//! pyrust_module! {
//!     constants {
//!         "pi" => Value::float(std::f64::consts::PI),
//!         #[cfg(unix)]
//!         "O_NONBLOCK" => Value::int(libc::O_NONBLOCK as i64),
//!     }
//!
//!     /// Custom module exception inheriting from `OSError`.
//!     exception MyError(OSError);
//!
//!     #[py_name = "getValue"]
//!     fn get_value(
//!         x: PyIndex,
//!         #[varargs] rest: &[Value],
//!         #[varkwargs] kw: indexmap::IndexMap<String, Value>,
//!     ) -> Result<Value> {
//!         /* FN_NAME = "<module>.getValue" */
//!         Ok(Value::int(x.expect_i64(FN_NAME, "x")?))
//!     }
//!
//!     #[native_state(MyState)]
//!     class MyClass("BaseClass") {
//!         constants { "KIND" => Value::string("demo") }
//!
//!         fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
//!             set_state(self_val.0, MyState::default())?;
//!             Ok(Value::none())
//!         }
//!
//!         #[getter]
//!         fn count(#[state] st: &MyState) -> Result<Value> {
//!             Ok(Value::int(st.count))
//!         }
//!
//!         #[setter("count")]
//!         fn set_count(#[state_mut] st: &mut MyState, v: PyInt) -> Result<Value> {
//!             st.count = v.expect_i64(FN_NAME, "v")?;
//!             Ok(Value::none())
//!         }
//!
//!         #[classmethod]
//!         fn create(#[positional_only] cls: PyValueRef<'_>) -> Result<Value> { /* ... */ }
//!
//!         #[staticmethod]
//!         fn version() -> Result<Value> { Ok(Value::int(1)) }
//!     }
//! }
//! ```

mod attrs;
mod builtin_fn;
mod class_macro;
mod clinic;
mod module_macro;
mod overload;
mod signature;

use proc_macro::TokenStream;

/// `#[pyfunction(name = "module.fn")]` — emits a sibling registration
/// constant so the function is picked up by the built-in dispatch
/// registry.  Use this for migrating individual arms; for a whole
/// module, prefer [`pyrust_module`].
#[proc_macro_attribute]
pub fn pyfunction(attr: TokenStream, item: TokenStream) -> TokenStream {
    builtin_fn::expand_pyfunction(attr, item)
}

/// File-scoped module declaration: see crate-level docs and
/// `crates/pyrust/src/builtin_modules/NATIVE_MACROS.md` for the full syntax.
#[proc_macro]
pub fn pyrust_module(input: TokenStream) -> TokenStream {
    module_macro::expand_pyrust_module(input)
}
