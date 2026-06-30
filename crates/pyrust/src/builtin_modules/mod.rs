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

/// Build a fresh exec namespace dict for a built-in module's Python body file,
/// pre-seeded with `__name__` set to the module's name.
///
/// `inject_python_members` exec's a Python source (e.g. `typing_py.py`) into a
/// throwaway dict.  CPython's class machinery reads the global `__name__` to set
/// `__module__` on classes defined in that source; without it every such class
/// would report `__module__ == "__main__"` (issue #2801).  Seeding `__name__`
/// here makes `typing.ParamSpec.__module__ == "typing"`, etc.
pub(crate) fn make_module_exec_ns(
    module: &std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>,
) -> crate::error::Result<Value> {
    use pyrust_core::PyKey;
    let name = module.borrow().name.clone();
    let ns = Value::dict(crate::value::PyDict::default());
    ns.dict_insert(PyKey::str_from("__name__"), Value::string(&name))?;
    Ok(ns)
}

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
        pyrust_builtin_modules_internal! { @parse [] [] $($spec)* }
    };
}

/// Implementation detail of [`pyrust_builtin_modules!`].  Threads two
/// accumulators through a TT-muncher: the first collects every entry as a
/// `(py_name_lit, rust_ident, fn_prefix_lit)` triple; the second collects a
/// `(py_name_lit, rust_ident)` pair for every entry carrying `@inject`, so
/// the generated `post_load_inject` dispatcher can call that module's
/// `inject_python_members` after import.  Once the input is drained both
/// lists are emitted as the full module + registry plumbing.
macro_rules! pyrust_builtin_modules_internal {
    // --- `@inject` arms (must precede the plain arms so the trailing
    //     `@ inject` token pair is matched before the plain arm consumes the
    //     entry without it). ---

    // Entry: `@flat ident @inject,` — flat namespace, post-load injection.
    (@parse [$($acc:tt)*] [$($inj:tt)*] @flat $ident:ident @ inject $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* (stringify!($ident), $ident, "")]
                   [$($inj)* (stringify!($ident), $ident)] $($($rest)*)?
        }
    };
    // Entry: `"py.dotted.name" as ident @inject,` — post-load injection.
    (@parse [$($acc:tt)*] [$($inj:tt)*] $py_name:literal as $ident:ident @ inject $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* ($py_name, $ident, concat!($py_name, "."))]
                   [$($inj)* ($py_name, $ident)] $($($rest)*)?
        }
    };
    // Entry: `ident @inject,` — post-load injection.
    (@parse [$($acc:tt)*] [$($inj:tt)*] $ident:ident @ inject $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* (stringify!($ident), $ident, concat!(stringify!($ident), "."))]
                   [$($inj)* (stringify!($ident), $ident)] $($($rest)*)?
        }
    };

    // --- plain arms (no `@inject`) ---

    // Entry: `@flat ident,` — flat namespace, no fn prefix.
    (@parse [$($acc:tt)*] [$($inj:tt)*] @flat $ident:ident $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* (stringify!($ident), $ident, "")] [$($inj)*] $($($rest)*)?
        }
    };
    // Entry: `"py.dotted.name" as ident,` — fn prefix = "<py.name>.".
    (@parse [$($acc:tt)*] [$($inj:tt)*] $py_name:literal as $ident:ident $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* ($py_name, $ident, concat!($py_name, "."))] [$($inj)*] $($($rest)*)?
        }
    };
    // Entry: `ident,` — Python name = stringify!(ident), fn prefix = "<ident>.".
    (@parse [$($acc:tt)*] [$($inj:tt)*] $ident:ident $(, $($rest:tt)*)?) => {
        pyrust_builtin_modules_internal! {
            @parse [$($acc)* (stringify!($ident), $ident, concat!(stringify!($ident), "."))] [$($inj)*] $($($rest)*)?
        }
    };
    // Done — emit.
    (@parse [$(($py_name:expr, $ident:ident, $fn_prefix:expr))*]
            [$(($inj_py_name:expr, $inj_ident:ident))*]) => {
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

        /// Post-import hook for Python-source modules.  For every `@inject`
        /// entry this dispatches to that module's `inject_python_members`,
        /// which exec's its `*_py.py` source onto the freshly imported
        /// module.  Called once from `env.rs::load_module` immediately after
        /// the module lands in `module_cache`, replacing the former chain of
        /// per-module `if name == "X" { … }` blocks.
        pub(crate) fn post_load_inject(
            name: &str,
            interp: &mut crate::interpreter::Interpreter,
            module: &std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>,
        ) -> crate::error::Result<()> {
            match name {
                $($inj_py_name => $inj_ident::inject_python_members(interp, module)?,)*
                _ => {}
            }
            Ok(())
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
    // `pyrust_module!` plus `operator_py.py` injected by the `@inject`
    // post-load hook (`post_load_inject` → `inject_python_members`).
    operator @inject,
    itertools,
    // `collections` Python-source members (issue #1884) are injected by the
    // `@inject` hook; `inject_python_members` also tags the public classes
    // with `__module__` / `__class_getitem__` (issues #2228 / #2603).
    collections @inject,
    "collections.abc" as collections_abc,
    io,
    // `typing` Python-source members (issue #2516) injected via `@inject`.
    typing @inject,
    copy,
    pathlib,
    // `string`: ASCII character-class constants are native; `capwords`,
    // `Template`, and `Formatter` are injected from `string_py.py` by the
    // `@inject` post-load hook (issue #2515).
    string @inject,
    // `contextlib`: `suppress`, `contextmanager`, `closing`, `nullcontext`,
    // `redirect_stdout/stderr`, and `ExitStack` are native; the ABCs,
    // `ContextDecorator`, `asynccontextmanager`, `aclosing`, and
    // `AsyncExitStack` are injected from `contextlib_py.py` by the `@inject`
    // post-load hook (issue #2795).
    contextlib @inject,
    "__future__" as future,
    warnings,
    // `json` is a pure-Python module (issue #2620): an empty native
    // `pyrust_module!` plus `json_py.py` injected by the `@inject` post-load
    // hook.
    json @inject,
    // Minimal async/await support (issue #1039): `asyncio.run` is native;
    // `sleep` / `gather` are injected from `asyncio_py.py` by the `@inject`
    // post-load hook.
    asyncio @inject,
    // `abc` (issue #2612): the whole surface (`ABCMeta`, `ABC`,
    // `abstractmethod`, …) is defined in `abc_py.py` and injected by the
    // `@inject` post-load hook; the native body is empty.
    abc @inject,
    // `dataclasses` (issue #2610): `@dataclass`, `field`, `fields`, `asdict`,
    // `astuple`, … are defined in `dataclasses_py.py` and injected by the
    // `@inject` post-load hook; the native body is empty.
    dataclasses @inject,
    // `enum` (issue #2611): a pure-Python module (`enum_py.py`) — `Enum`,
    // `IntEnum`, `EnumMeta`/`EnumType`, `auto` — injected by the `@inject`
    // post-load hook.  The Rust ident is `enum_mod` because `enum` is a
    // keyword; its Python-level name is `enum`.
    "enum" as enum_mod @inject,
    // `re` (issue #2625): a pure-Python regex engine (`re_py.py`) — `compile`,
    // `match` / `search` / `findall` / `sub` / `split`, the `Pattern` / `Match`
    // objects, and the `error` exception — injected by the `@inject` post-load
    // hook.  The Rust ident is `re_mod` because `re` is a keyword; its
    // Python-level name is `re`.
    "re" as re_mod @inject,
    // `types` — the type objects for runtime objects without a built-in name
    // binding (`NoneType`, `FunctionType`, `MappingProxyType`, …).  The native
    // body supplies the type-object constants and `MappingProxyType`;
    // `SimpleNamespace` is defined in `types_py.py` and injected by the
    // `@inject` post-load hook.
    types @inject,
    // `textwrap` (issue #2786): a pure-Python module (`textwrap_py.py`) —
    // `TextWrapper`, `wrap`, `fill`, `shorten`, `dedent`, `indent` — injected
    // by the `@inject` post-load hook; the native body is empty.
    textwrap @inject,
    // `bisect` (issue #2784): array-bisection algorithms — a pure-Python
    // module (`bisect_py.py`) injected by the `@inject` post-load hook; the
    // native body is empty.
    bisect @inject,
    // `heapq` (issue #2784): heap-queue (priority-queue) algorithms — a
    // pure-Python module (`heapq_py.py`) injected by the `@inject` post-load
    // hook; the native body is empty.
    heapq @inject,
    // `pprint` (issue #2812): a pure-Python pretty-printer (`pprint_py.py`,
    // transcribed from CPython 3.12's `Lib/pprint.py`) — `pprint`, `pformat`,
    // `PrettyPrinter`, `isreadable`, `isrecursive`, `saferepr`, `pp` — injected
    // by the `@inject` post-load hook.  The native body is empty.
    pprint @inject,
    // `statistics` (issue #2811): a float-based adaptation of CPython's pure-
    // Python `statistics` module — `mean` / `median` / `mode` / `variance` /
    // `stdev` / `NormalDist` / … — defined in `statistics_py.py` and injected
    // by the `@inject` post-load hook; the native body is empty.
    statistics @inject,
    // `csv` (issue #2808): a pure-Python module (`csv_py.py`) — the
    // `reader` / `writer` factories, `DictReader` / `DictWriter`, the
    // `Dialect` family, the dialect registry, and the `QUOTE_*` constants —
    // injected by the `@inject` post-load hook; the native body is empty.
    csv @inject,
    // `decimal` (issue #2806): a port of CPython 3.12's pure-Python
    // `_pydecimal.py` (`decimal_py.py`) — `Decimal`, `Context`,
    // `getcontext` / `setcontext` / `localcontext`, the `ROUND_*` constants,
    // and the `DecimalException` hierarchy — injected by the `@inject`
    // post-load hook.  The native body is empty.
    decimal @inject,
    // `fractions` (issue #2810): a pure-Python module (`fractions_py.py`) — the
    // `Fraction` rational-number class — injected by the `@inject` post-load
    // hook.  The native body is empty.
    fractions @inject,
}
