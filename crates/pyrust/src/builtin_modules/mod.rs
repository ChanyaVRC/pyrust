//! Per-module registration for built-in callables.
//!
//! **Single source of truth.**  Every built-in module — its Python-level
//! name, its Rust module ident, the file holding its functions — is
//! declared exactly once in the `pyrust_builtin_modules!` invocation
//! below.  Adding a new module is a one-line edit; nothing else in the
//! crate needs to change.
//!
//! The macro expands each entry into an inline `pub mod <ident> { … }`
//! that:
//! - sets a sibling `MODULE_NAME: &'static str` constant carrying the
//!   Python-level name (so the body file never has to repeat it),
//! - `include!`s the body file under `bodies/<ident>.rs`,
//! - is wired into the function registry (via `all_regs`) and the
//!   `import` path (via `load_builtin_module`).

use crate::builtin_registry::BuiltinReg;
use crate::value::Value;

/// Declares the set of built-in modules.
///
/// Syntax:
/// - `ident` — module whose Python-level name equals the Rust ident
///   (e.g. `math`, `sys`).  Body file: `bodies/<ident>.rs`.
/// - `"py.dotted.name" as ident` — module with a dotted Python-level name
///   that the Rust ident can't represent (e.g. `"os.path" as os_path`).
///   Body file: `bodies/<ident>.rs`.
///
/// For each entry the macro emits, in this file's scope:
/// - `pub mod <ident> { pub(super) const MODULE_NAME: &str = "<name>"; include!("bodies/<ident>.rs"); }`
/// - one branch of [`load_builtin_module`] keyed on the Python-level name,
/// - one slice contribution to [`all_regs`].
///
/// Body files therefore never need to know their own module name — the
/// macro injects `MODULE_NAME` and the body's `pyrust_module!` reads it
/// via the surrounding scope.
macro_rules! pyrust_builtin_modules {
    ($($spec:tt)*) => {
        pyrust_builtin_modules_internal! { @parse [] $($spec)* }
    };
}

/// Implementation detail of [`pyrust_builtin_modules!`] — accumulates
/// parsed entries as `(py_name_lit, rust_ident)` pairs and then emits the
/// full module + registry plumbing.
macro_rules! pyrust_builtin_modules_internal {
    // Entry: `"py.dotted.name" as ident,`
    (@parse [$($acc:tt)*] $py_name:literal as $ident:ident $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* ($py_name, $ident)] $($($rest)*)?
        }
    };
    // Entry: `ident,` — Python name equals stringify!(ident)
    (@parse [$($acc:tt)*] $ident:ident $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* (stringify!($ident), $ident)] $($($rest)*)?
        }
    };
    // Done.
    (@parse [$(($py_name:expr, $ident:ident))*]) => {
        $(
            pub mod $ident {
                /// Python-level name of this built-in module.  Injected
                /// from `pyrust_builtin_modules!` so the body file never
                /// has to repeat it; consumed by the body's
                /// `pyrust_module!` macro to compose each function's
                /// `FN_NAME` and to populate `PyModule.name`.
                pub(crate) const MODULE_NAME: &str = $py_name;

                include!(concat!("bodies/", stringify!($ident), ".rs"));
            }
        )*

        /// Concatenate every per-module `regs()` slice — consumed by
        /// `crate::builtin_registry::REGISTRY` on first use.
        pub(crate) fn all_regs() -> Vec<BuiltinReg> {
            let mut all: Vec<BuiltinReg> = Vec::new();
            $(
                all.extend_from_slice($ident::regs());
            )*
            all
        }

        /// Resolve `import name` to its built-in module value, if any.
        /// Replaces the hand-maintained `match name { "math" => …, … }`
        /// previously sitting inside `env.rs::load_module`.
        pub fn load_builtin_module(name: &str) -> Option<Value> {
            $(
                if name == $py_name {
                    return Some($ident::module());
                }
            )*
            None
        }
    };
}

// The single source of truth for the set of built-in modules pyrust
// ships.  To add a new module:
//   1. Drop the body file under `bodies/<ident>.rs`.
//   2. Add one line here.
// That's it — `env.rs`, `builtin_registry.rs`, and the body file itself
// need no name knowledge beyond what's declared on this line.
pyrust_builtin_modules! {
    math,
    sys,
}
