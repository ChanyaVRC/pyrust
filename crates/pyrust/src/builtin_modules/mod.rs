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
///   (e.g. `math`, `sys`).  Body file: `bodies/<ident>.rs`.  Functions
///   register as `"<ident>.<short>"`.
/// - `"py.dotted.name" as ident` — module with a dotted Python-level name
///   that the Rust ident can't represent (e.g. `"os.path" as os_path`).
/// - `@flat ident` — **flat-namespace** module.  Functions register
///   under their short name only (no `<ident>.` prefix), so this is the
///   form for Python's built-in `builtins` module whose members
///   (`abs`, `len`, `print`, …) are accessed without a prefix.  The
///   module is *also* importable (`import builtins`) with its Python
///   name set to the Rust ident.
///
/// For each entry the macro emits, in this file's scope:
/// - `pub mod <ident> { … }` with two injected constants — `MODULE_NAME`
///   (Python-level name for `PyModule.name` + `import`) and `FN_PREFIX`
///   (prepended to each function's short name to form its registration
///   name) — followed by `include!("bodies/<ident>.rs")`.
/// - one branch of [`load_builtin_module`] keyed on the Python-level name,
/// - one slice contribution to [`all_regs`].
///
/// Body files therefore never need to know their own module name — the
/// macro injects `MODULE_NAME` / `FN_PREFIX` and the body's
/// `pyrust_module!` reads them via the surrounding scope.
macro_rules! pyrust_builtin_modules {
    ($($spec:tt)*) => {
        pyrust_builtin_modules_internal! { @parse [] $($spec)* }
    };
}

/// Implementation detail of [`pyrust_builtin_modules!`].  Accumulates
/// parsed entries as `(py_name_lit, rust_ident, fn_prefix_lit)` triples,
/// then emits the full module + registry plumbing once the input is
/// drained.
macro_rules! pyrust_builtin_modules_internal {
    // Entry: `@flat ident,` — flat namespace, no fn prefix.
    (@parse [$($acc:tt)*] @flat $ident:ident $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* (stringify!($ident), $ident, "")] $($($rest)*)?
        }
    };
    // Entry: `"py.dotted.name" as ident,` — fn prefix = "<py.name>.".
    (@parse [$($acc:tt)*] $py_name:literal as $ident:ident $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* ($py_name, $ident, concat!($py_name, "."))] $($($rest)*)?
        }
    };
    // Entry: `ident,` — Python name = stringify!(ident), fn prefix = "<ident>.".
    (@parse [$($acc:tt)*] $ident:ident $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* (stringify!($ident), $ident, concat!(stringify!($ident), "."))] $($($rest)*)?
        }
    };
    // Done — emit.
    (@parse [$(($py_name:expr, $ident:ident, $fn_prefix:expr))*]) => {
        $(
            pub mod $ident {
                /// Python-level name of this built-in module.  Used for
                /// `PyModule.name` and as the key for `import`.
                pub(crate) const MODULE_NAME: &str = $py_name;
                /// Prefix prepended to every function's short name to
                /// form its Python-level registration name (e.g.
                /// `"math."` so `sqrt` becomes `"math.sqrt"`).  Empty
                /// for `@flat` modules whose functions live in the
                /// global namespace (`abs`, `len`, …).
                ///
                /// `dead_code` allow because constants-only bodies
                /// (`os.rs`, …) declare no fns, so the macro-emitted
                /// `pyrust_module!` expansion doesn't reference this
                /// const.  The other 99% of bodies use it via
                /// `emit_fn_artefacts`.
                #[allow(dead_code)]
                pub(crate) const FN_PREFIX: &str = $fn_prefix;

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
    errno,
    @flat builtins,
    "os.path" as os_path,
    // `os` is a parent for `os.path` — see `bodies/os.rs`.  Must come
    // after `os.path` in the order so the `super::os_path::module()`
    // reference in `os`'s `constants` block resolves cleanly.
    os,
    functools,
    // `operator` is a pure-Python module (issue #2514): an empty native
    // `pyrust_module!` plus `operator_py.py` injected by the post-load hook
    // in `env.rs::load_module`.
    operator,
    itertools,
    collections,
    "collections.abc" as collections_abc,
    io,
    typing,
    copy,
    pathlib,
    // `string`: ASCII character-class constants are native; `capwords`,
    // `Template`, and `Formatter` are injected from `string_py.py` by the
    // post-load hook in `env.rs::load_module` (issue #2515).
    string,
    contextlib,
    "__future__" as future,
    warnings,
    // `json` is a pure-Python module (issue #2620): an empty native
    // `pyrust_module!` plus `json_py.py` injected by the post-load hook in
    // `env.rs::load_module`.
    json,
    // Minimal async/await support (issue #1039): `asyncio.run` is native;
    // `sleep` / `gather` are injected from `asyncio_py.py` by the post-load
    // hook in `env.rs::load_module`.
    asyncio,
    // `abc` (issue #2612): the whole surface (`ABCMeta`, `ABC`,
    // `abstractmethod`, …) is defined in `abc_py.py` and injected by the
    // post-load hook in `env.rs::load_module`; the native body is empty.
    abc,
    // `dataclasses` (issue #2610): `@dataclass`, `field`, `fields`, `asdict`,
    // `astuple`, … are defined in `dataclasses_py.py` and injected by the
    // post-load hook in `env.rs::load_module`; the native body is empty.
    dataclasses,
    // `enum` (issue #2611): a pure-Python module (`enum_py.py`) — `Enum`,
    // `IntEnum`, `EnumMeta`/`EnumType`, `auto` — injected by the post-load
    // hook in `env.rs::load_module`.  The Rust ident is `enum_mod` because
    // `enum` is a keyword; its Python-level name is `enum`.
    "enum" as enum_mod,
}
