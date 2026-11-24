//! `pyrust-derive` — proc-macros for declaring built-in Python modules in Rust.
//!
//! Two entry points:
//!
//! - **`pyrust_module! { … }`** (function-like, primary) — declares a
//!   whole built-in module's content (optional constants, functions, and
//!   classes).  The **module's Python-level name is *not* given here**;
//!   the macro reads it from a sibling `MODULE_NAME: &str` constant that
//!   the surrounding `pyrust_builtin_modules!` invocation in
//!   `builtin_modules/mod.rs` injects.  This makes that `mod.rs`
//!   the single source of truth for the set of built-in modules and
//!   their Python-level names.
//!
//!   Each `fn name(args)` is expanded to a full Rust fn with the
//!   canonical dispatch signature; a `FN_NAME: &str` local at the top of
//!   the body lets call sites (helpers, error messages) reference the
//!   Python-level full name without re-spelling it.
//!
//! - **`#[pyfunction(name = "module.fn")]`** (attribute, one-off fallback)
//!   — drop-in for moving a single arm of the legacy cascade
//!   incrementally.  Same output as a single `fn` inside
//!   `pyrust_module!` minus the `regs`/`module()` plumbing.
//!
//! ## `pyrust_module!` syntax
//!
//! ```ignore
//! // bodies/math.rs (included from `pub mod math { … }` declared by
//! // `pyrust_builtin_modules!` in mod.rs):
//! pyrust_module! {
//!     constants {
//!         "pi" => Value::float(std::f64::consts::PI),
//!     }
//!
//!     /// CPython: math.sqrt(x) → float.
//!     fn sqrt(args) -> Result<Value> {
//!         Ok(Value::float(single_float(FN_NAME, args)?.sqrt()))
//!     }
//! }
//! ```
//!
//! Generated, in the surrounding `mod math { … }`:
//! - `fn __pyfn_sqrt(_interp, args) -> Result<Value> { /* FN_NAME = "math.sqrt"; body */ }`
//! - `pub(crate) fn regs() -> &'static [BuiltinReg]` — every fn's
//!   `BuiltinReg`, names composed once via leak from `FN_PREFIX +
//!   short_name` (e.g. `"math." + "sqrt"`, or `"" + "abs"` for the
//!   flat-namespace `builtins` module).
//! - `pub(crate) fn module() -> Value` — the `PyModule` carrying the
//!   declared constants plus each fn bound to its
//!   `Value::builtin_function(...)`.

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

/// File-scoped module declaration: see crate-level docs for the full
/// syntax and the contract with `pyrust_builtin_modules!`.
#[proc_macro]
pub fn pyrust_module(input: TokenStream) -> TokenStream {
    module_macro::expand_pyrust_module(input)
}
