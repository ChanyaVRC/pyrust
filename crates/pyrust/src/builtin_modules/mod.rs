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

#[macro_use]
pub(crate) mod macros;
pub(crate) mod encodings_codecs;
pub(crate) mod internal_shims;

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

/// Publish the `__all__` object created by an injected Python body, if any.
///
/// The value itself is shared with the exec namespace so its type, item order,
/// and identity are preserved.
fn publish_exec_dunder_all(
    module: &std::rc::Rc<std::cell::RefCell<crate::value::PyModule>>,
    exec_ns: &Value,
) -> crate::error::Result<()> {
    use pyrust_core::PyKey;
    let dict = exec_ns.as_dict().ok_or_else(|| {
        crate::error::PyError::runtime("injected module exec namespace not a dict".into())
    })?;
    if let Some(all) = dict.get(&PyKey::str_from("__all__")) {
        module
            .borrow_mut()
            .attrs
            .insert("__all__".to_string(), all.clone());
    }
    Ok(())
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
/// Helper emitting one `pub mod <ident>` item per entry in
/// [`pyrust_builtin_modules!`]. Expanding each entry independently keeps
/// macro recursion depth O(1) regardless of how many built-in modules exist.
macro_rules! pyrust_builtin_mod_def {
    ($(#[cfg($($cfg:tt)*)])* @flat $ident:ident $(@inject)?) => {
        $(#[cfg($($cfg)*)])*
        pub mod $ident {
            /// Python-level name of this built-in module.  Used for
            /// `PyModule.name` and as the key for `import`.
            pub(crate) const MODULE_NAME: &str = stringify!($ident);
            /// Empty prefix for `@flat` modules whose functions live in the
            /// global namespace (`abs`, `len`, …).
            #[allow(dead_code)]
            pub(crate) const FN_PREFIX: &str = "";

            include!(concat!("bodies/", stringify!($ident), ".rs"));
        }
    };
    ($(#[cfg($($cfg:tt)*)])* $py_name:literal as $ident:ident $(@inject)?) => {
        $(#[cfg($($cfg)*)])*
        pub mod $ident {
            /// Python-level name of this built-in module.  Used for
            /// `PyModule.name` and as the key for `import`.
            pub(crate) const MODULE_NAME: &str = $py_name;
            /// Prefix prepended to every function's short name to form its
            /// Python-level registration name.
            #[allow(dead_code)]
            pub(crate) const FN_PREFIX: &str = concat!($py_name, ".");

            include!(concat!("bodies/", stringify!($ident), ".rs"));
        }
    };
    ($(#[cfg($($cfg:tt)*)])* $ident:ident $(@inject)?) => {
        $(#[cfg($($cfg)*)])*
        pub mod $ident {
            /// Python-level name of this built-in module.  Used for
            /// `PyModule.name` and as the key for `import`.
            pub(crate) const MODULE_NAME: &str = stringify!($ident);
            /// Prefix prepended to every function's short name to form its
            /// Python-level registration name.
            #[allow(dead_code)]
            pub(crate) const FN_PREFIX: &str = concat!(stringify!($ident), ".");

            include!(concat!("bodies/", stringify!($ident), ".rs"));
        }
    };
}

/// Helper expanding one entry's Python-level module name expression.
macro_rules! pyrust_builtin_mod_name {
    ($py_name:literal as $ident:ident) => {
        $py_name
    };
    ($ident:ident) => {
        stringify!($ident)
    };
}

/// Helper emitting one `post_load_inject` dispatch check for `@inject` modules.
macro_rules! pyrust_builtin_mod_inject {
    ($name:expr, $interp:expr, $module:expr, $py_name:expr, $ident:ident @inject) => {
        if $name == $py_name {
            let exec_ns = $ident::inject_python_members($interp, $module)?;
            if let Some(exec_ns) = exec_ns {
                publish_exec_dunder_all($module, &exec_ns)?;
            }
            return Ok(());
        }
    };
    ($name:expr, $interp:expr, $module:expr, $py_name:expr, $ident:ident) => {};
}

macro_rules! pyrust_builtin_modules {
    (
        $(
            $(#[cfg($($cfg:tt)*)])*
            $(@ $flat:ident)? $($py_name:literal as)? $ident:ident $(@ $inj:ident)?
        ),* $(,)?
    ) => {
        $(
            pyrust_builtin_mod_def!($(#[cfg($($cfg)*)])* $(@$flat)? $($py_name as)? $ident $(@$inj)?);
        )*

        /// Concatenate every per-module `regs()` slice — consumed by
        /// `crate::builtin_registry::REGISTRY` on first use.
        pub(crate) fn all_regs() -> Vec<BuiltinReg> {
            let mut all: Vec<BuiltinReg> = Vec::new();
            all.extend_from_slice(macros::structseq_regs());
            #[cfg(test)]
            all.extend_from_slice(macro_tests::fixture_regs());
            $(
                $(#[cfg($($cfg)*)])*
                {
                    all.extend_from_slice($ident::regs());
                }
            )*
            all.extend(encodings_codecs::all_regs());
            all.extend(internal_shims::all_regs());
            all
        }

        /// Resolve `import name` to its built-in module value, if any.
        /// Replaces the hand-maintained `match name { "math" => …, … }`
        /// previously sitting inside `env.rs::load_module`.
        pub fn load_builtin_module(name: &str) -> Option<Value> {
            $(
                $(#[cfg($($cfg)*)])*
                {
                    if name == pyrust_builtin_mod_name!($($py_name as)? $ident) {
                        return Some($ident::module());
                    }
                }
            )*
            if let Some(m) = encodings_codecs::load_builtin_module(name) {
                return Some(m);
            }
            internal_shims::load_builtin_module(name)
        }

        /// Return `true` when `name` is a built-in module declared in
        /// `pyrust_builtin_modules!` without allocating a module object.
        pub fn is_builtin_module(name: &str) -> bool {
            $(
                $(#[cfg($($cfg)*)])*
                {
                    if name == pyrust_builtin_mod_name!($($py_name as)? $ident) {
                        return true;
                    }
                }
            )*
            encodings_codecs::is_builtin_module(name)
                || internal_shims::is_builtin_module(name)
        }

        /// Return `true` when `name` is a built-in package (matches CPython
        /// 3.12 stdlib packages shipped as built-in modules in PyRust, plus
        /// any parent of a dotted built-in module except `os`).
        pub fn is_builtin_package(name: &str) -> bool {
            if name == "os" {
                return false;
            }
            if matches!(
                name,
                "asyncio"
                    | "collections"
                    | "email"
                    | "html"
                    | "http"
                    | "importlib"
                    | "json"
                    | "logging"
                    | "re"
                    | "sqlite3"
                    | "tomllib"
                    | "unittest"
                    | "urllib"
                    | "zipfile"
                    | "zoneinfo"
            ) {
                return true;
            }
            $(
                $(#[cfg($($cfg)*)])*
                {
                    if let Some(rest) = pyrust_builtin_mod_name!($($py_name as)? $ident).strip_prefix(name)
                        && rest.starts_with('.')
                    {
                        return true;
                    }
                }
            )*
            false
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
            if name == "site" {
                let module_val = Value::py_module(std::rc::Rc::clone(module));
                return site::post_load_init(interp, &module_val);
            }
            if name == "warnings" {
                return warnings::post_load_init(interp);
            }
            match name {
                "importlib.abc" => return importlib_abc::post_load_init(interp, module),
                "importlib.resources.abc" => {
                    return importlib_resources_abc::post_load_init(interp, module);
                }
                "importlib.resources.readers" => {
                    return importlib_resources_readers::post_load_init(interp, module);
                }
                "importlib.resources.simple" => {
                    return importlib_resources_simple::post_load_init(interp, module);
                }
                "importlib.resources" => {
                    return importlib_resources::post_load_init(interp, module);
                }
                "importlib.readers" => return importlib_readers::post_load_init(interp, module),
                "importlib.simple" => return importlib_simple::post_load_init(interp, module),
                "importlib.metadata" => return importlib_metadata::post_load_init(interp, module),
                "_frozen_importlib" => return _frozen_importlib::post_load_init(interp, module),
                "_frozen_importlib_external" => {
                    return _frozen_importlib_external::post_load_init(interp, module);
                }
                "encodings" => return encodings::post_load_init(interp, module),
                "encodings.punycode" => return encodings_punycode::post_load_init(interp, module),
                "encodings.idna" => return encodings_idna::post_load_init(interp, module),
                "_compression" => return _compression::post_load_init(interp, module),
                _ => {}
            }
            if encodings_codecs::post_load_init(name, interp, module)? {
                return Ok(());
            }
            if internal_shims::post_load_init(name, interp, module)? {
                return Ok(());
            }
            $(
                $(#[cfg($($cfg)*)])*
                {
                    pyrust_builtin_mod_inject!(
                        name,
                        interp,
                        module,
                        pyrust_builtin_mod_name!($($py_name as)? $ident),
                        $ident $(@$inj)?
                    );
                }
            )*
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
    // Cross-platform path modules (issue #3245): pure native Rust, importable
    // on ALL platforms matching CPython 3.12. Must precede `os.path` and `os`.
    genericpath,
    posixpath,
    ntpath,
    nturl2path,
    "os.path" as os_path,
    // `os` is a parent for `os.path` — see `bodies/os.rs`.  Must come
    // after `os.path` in the order so the `super::os_path::module()`
    // reference in `os`'s `constants` block resolves cleanly.
    os,
    // Generation-bound factory callables and dynamic helper classes are
    // installed after the fresh module enters this Interpreter's import cache.
    functools @inject,
    // `operator` is a pure-Python module (issue #2514): an empty native
    // `pyrust_module!` plus `operator_py.py` injected by the `@inject`
    // post-load hook (`post_load_inject` → `inject_python_members`).
    operator @inject,
    itertools,
    // `collections` Python-source members (issue #1884) are injected by the
    // `@inject` hook; `inject_python_members` also tags the public classes
    // with `__module__` / `__class_getitem__` (issues #2228 / #2603).
    collections @inject,
    "collections.abc" as collections_abc @inject,
    // `io` Python-source supplements (issue #2778): SEEK_* constants,
    // DEFAULT_BUFFER_SIZE, `open` alias, and the IOBase ABC family injected via
    // `@inject` (`io_py.py`); the native body provides StringIO / BytesIO.
    io @inject,
    // `typing` Python-source members (issue #2516) injected via `@inject`.
    typing @inject,
    // `copy.copy` keeps its native container fast paths. A private injected
    // Python helper owns the rare staticmethod-reduction reconstruction path
    // so its observable Python protocol operations match CPython (#2958).
    copy @inject,
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
    // `random` (issue #2785): a pure-Python MT19937 generator and the full
    // public API live in `random_py.py`, injected by the `@inject` post-load
    // hook.  The native body is an empty `pyrust_module!` so `import random`
    // resolves to a real `PyModule`.
    random @inject,
    // `time` (issue #2787): native clock / sleep functions and the libc-backed
    // calendar conversions; `struct_time` is defined in `time_py.py` and
    // injected by the `@inject` post-load hook.
    time @inject,
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
    // `struct` (issue #2807): native binary packing/unpacking primitives in
    // `struct_mod.rs` plus `error`, `Struct`, and top-level wrappers injected
    // from `struct_mod_py.py` by the `@inject` post-load hook.  The Rust ident
    // is `struct_mod` because `struct` is a keyword.
    "struct" as struct_mod @inject,
    // `array` (issue #2809): native typed-element binary encoding/decoding and
    // byteswap helpers in `array.rs` plus the `array` class, `ArrayType` alias,
    // and `typecodes` constant injected from `array_py.py` by the `@inject`
    // post-load hook.
    array @inject,
    // `datetime` (issue #2805): date/time/datetime/timedelta/tzinfo/timezone
    // types injected from `datetime_py.py` by the `@inject` post-load hook.
    datetime @inject,
    // `binascii` (issue #3145): native hex, Base64, CRC-32, CRC-HQX, uu, and
    // quoted-printable codecs in `binascii.rs` plus `Error`, `Incomplete`, and
    // exact CPython 3.12 wrapper signatures injected from `binascii_py.py`.
    binascii @inject,
    // `base64` (issue #3145): pure-Python Base16, Base32, Base32hex, Base64,
    // Base85, Ascii85, and RFC 2045 encoders/decoders injected from
    // `base64_py.py` by the `@inject` post-load hook.
    base64 @inject,
    // `hashlib` (issue #3146): pure-Rust cryptographic hash primitives
    // (MD5, SHA-1, SHA-2, SHA-3/SHAKE, BLAKE2b/s, PBKDF2-HMAC) in `hashlib.rs`
    // plus the CPython 3.12 wrapper surface injected from `hashlib_py.py`.
    hashlib @inject,
    // `hmac` (issue #3146): constant-time `compare_digest` and native HMAC
    // fast path in `hmac.rs` plus `HMAC`, `new`, and `digest` injected from
    // `hmac_py.py`.
    hmac @inject,
    // `secrets` (issue #3146): cryptographically strong token and random
    // helpers built on `base64`, `hmac.compare_digest`, and `random.SystemRandom`
    // injected from `secrets_py.py`.
    secrets @inject,
    // `traceback` (issue #3152): pure-Python stack/exception extraction and
    // formatting (`FrameSummary`, `StackSummary`, `TracebackException`,
    // `format_exception`, `format_exc`, `extract_tb`, `extract_stack`, …)
    // injected from `traceback_py.py`.
    traceback @inject,
    // `inspect` (issue #3152): live-object introspection predicates, PEP 362
    // `Signature`/`Parameter`/`BoundArguments`, frame/stack/closure/generator
    // inspection, and source/docstring helpers injected from `inspect_py.py`
    // with a private native bytecode-name classifier in `inspect.rs`.
    inspect @inject,
    // `numbers` (issue #3147): PEP 3141 numeric tower ABCs (`Number`,
    // `Complex`, `Real`, `Rational`, `Integral`) injected from `numbers_py.py`.
    numbers @inject,
    // `difflib` (issue #3147): pure-Python sequence matching and delta helpers
    // (`SequenceMatcher`, `Differ`, `unified_diff`, `context_diff`, `ndiff`,
    // `restore`, `diff_bytes`, `get_close_matches`, `HtmlDiff`, `Match`)
    // injected from `difflib_py.py`.
    difflib @inject,
    // `shlex` (issue #3147): pure-Python shell-like lexical analyzer (`shlex`,
    // `split`, `join`, `quote`) injected from `shlex_py.py`.
    shlex @inject,
    // `fnmatch` (issue #3148): pure-Python Unix shell-style filename pattern
    // matching (`fnmatch`, `fnmatchcase`, `filter`, `translate`) injected from
    // `fnmatch_py.py` by the `@inject` post-load hook.
    fnmatch @inject,
    // `glob` (issue #3148): pure-Python pathname pattern expansion (`glob`,
    // `iglob`, `escape`, `has_magic`, `glob0`, `glob1`) injected from
    // `glob_py.py` by the `@inject` post-load hook.
    glob @inject,
    // `shutil` (issue #3148): high-level file/directory copy, move, rmtree,
    // `which`, and `get_terminal_size` injected from `shutil_py.py` backed by
    // minimal native OS helpers in `shutil.rs`.
    shutil @inject,
    // `tempfile` (issue #3148): `gettempdir*`, `mkstemp`, `mkdtemp`,
    // `TemporaryFile`, `NamedTemporaryFile`, `SpooledTemporaryFile`, and
    // `TemporaryDirectory` injected from `tempfile_py.py` backed by atomic
    // exclusive creation and seekable file streams in `tempfile.rs`.
    tempfile @inject,
    // `argparse` (issue #3150): pure-Python command-line option and argument
    // parser (`ArgumentParser`, `Namespace`, `Action`, `BooleanOptionalAction`,
    // `FileType`, help formatters, groups, subparsers, …) injected from
    // `argparse_py.py`.
    argparse @inject,
    // `urllib` (`urllib.parse`, `urllib.response`, `urllib.error`,
    // `urllib.request`) (issues #3149, #3223): RFC 3986 URL parsing/quoting,
    // response wrappers, exception hierarchy, and extensible URL opener
    // exposed as submodules on the `urllib` parent package.
    "urllib.parse" as urllib_parse @inject,
    "urllib.response" as urllib_response @inject,
    "urllib.error" as urllib_error @inject,
    "urllib.request" as urllib_request @inject,
    urllib,
    // `html.entities`, `html.parser`, and `html` (issue #3149): static HTML4 /
    // HTML5 character reference tables in `html_entities.rs`, `escape` /
    // `unescape` injected from `html_py.py`, and streaming `HTMLParser`
    // injected from `html_parser_py.py`.
    "html.entities" as html_entities,
    "html.parser" as html_parser @inject,
    html @inject,
    // `logging` (issue #3151): Python 3.12 standard logging facility —
    // levels, `Logger`/`Manager`/`PlaceHolder`/`LoggerAdapter`, `LogRecord`,
    // `Handler`/`StreamHandler`/`FileHandler`/`NullHandler`/`lastResort`,
    // `Formatter` (`%`, `{`, `$` styles)/`BufferingFormatter`, `Filter`,
    // `basicConfig`, and `captureWarnings` — injected from `logging_py.py`
    // backed by a real `std::fs::File` stream helper in `logging.rs`.
    logging @inject,
    // `unittest` (issue #3153): Python 3.12 unit testing framework (`TestCase`,
    // `FunctionTestCase`, `IsolatedAsyncioTestCase`, `TestSuite`, `TestLoader`,
    // `TestResult`, `TextTestResult`, `TextTestRunner`, `TestProgram`/`main`,
    // skipping/expectedFailure, cleanups, subTest, and `unittest.*` submodules)
    // injected from `unittest_py.py`.
    unittest @inject,
    // `cmath` (issue #3185): native complex mathematical functions, constants,
    // and C99 Annex G branch-cut / special-value handling in `cmath.rs`.
    cmath,
    // `uuid` (issue #3185): pure-Python RFC 4122 UUID objects and v1/v3/v4/v5
    // generators injected from `uuid_py.py`.
    uuid @inject,
    // `calendar` (issue #3185): pure-Python calendar utilities, `Day`/`Month`
    // IntEnums, `Calendar`, `TextCalendar`, and `HTMLCalendar` injected from
    // `calendar_py.py`.
    calendar @inject,
    // `configparser` (issue #3185): pure-Python INI parser (`ConfigParser`,
    // `RawConfigParser`, interpolations, `SectionProxy`, `ConverterMapping`)
    // injected from `configparser_py.py`.
    configparser @inject,
    // `contextvars` (issue #3184, PEP 567): context variables (`ContextVar`,
    // `Token`, `Context`, `copy_context`) injected from `contextvars_py.py`.
    contextvars @inject,
    // `tomllib` (issue #3184, PEP 680): pure-Python TOML 1.0.0 parser
    // (`loads`, `load`, `TOMLDecodeError`) injected from `tomllib_py.py`.
    tomllib @inject,
    // `zlib`, `gzip`, `zipfile`, and `tarfile` (issue #3186): pure-Rust RFC
    // 1950/1951/1952 compression engine in `zlib/` plus the CPython 3.12
    // streaming/archive surfaces injected from `*_py.py`.
    zlib @inject,
    gzip @inject,
    // `bz2` and `lzma` (issue #3224): pure-Rust BZip2 (`bzip2` / `libbz2-rs-sys`)
    // and XZ/LZMA (`lzma-rust2`) compression modules with CPython 3.12 wire parity.
    bz2 @inject,
    lzma @inject,
    zipfile @inject,
    tarfile @inject,
    // `subprocess` (issue #3187): Python 3.12 subprocess management (`Popen`,
    // `CompletedProcess`, `run`, `call`, `check_call`, `check_output`,
    // `list2cmdline`, `getstatusoutput`, `getoutput`, `PIPE`/`STDOUT`/`DEVNULL`,
    // `SubprocessError`/`CalledProcessError`/`TimeoutExpired`) injected from
    // `subprocess_py.py` backed by `std::process::Command` in `subprocess.rs`.
    subprocess @inject,
    // `pickle` (issue #3188): pure-Python binary object serialization
    // (`Pickler`, `Unpickler`, `dump`/`dumps`, `load`/`loads`, `PickleBuffer`,
    // protocols 0-5) injected from `pickle_py.py`.
    pickle @inject,
    // `importlib`, `importlib.machinery`, `importlib.util`, `importlib.abc`,
    // `importlib.resources` (`.abc`, `.readers`, `.simple`), `importlib.readers`,
    // `importlib.simple`, `importlib.metadata`, and `_frozen_importlib*`
    // (issues #3189, #3244). All new submodules are 100% native Rust (no `@inject`).
    "importlib.machinery" as importlib_machinery @inject,
    "importlib.util" as importlib_util @inject,
    "importlib.resources.abc" as importlib_resources_abc,
    "importlib.resources.readers" as importlib_resources_readers,
    "importlib.resources.simple" as importlib_resources_simple,
    "importlib.resources" as importlib_resources,
    "importlib.readers" as importlib_readers,
    "importlib.simple" as importlib_simple,
    "importlib.abc" as importlib_abc,
    "importlib.metadata" as importlib_metadata,
    importlib @inject,
    _frozen_importlib,
    _frozen_importlib_external,
    // `weakref` (issue #3190): weak references (`ref`/`ReferenceType`),
    // `proxy`/`ProxyType`/`CallableProxyType`/`ProxyTypes`, `getweakrefcount`,
    // `getweakrefs`, `WeakMethod`, `WeakKeyDictionary`, `WeakValueDictionary`,
    // `WeakSet`, and `finalize` backed by native weak handles in `weakref.rs`.
    weakref @inject,
    // `unicodedata` (issue #3203): Unicode 15.0.0 character database (`category`,
    // `bidirectional`, `combining`, `east_asian_width`, `mirrored`,
    // `decomposition`, `decimal`, `digit`, `numeric`, `name`, `lookup`,
    // `normalize`, `is_normalized`, `unidata_version`, `UCD`, `ucd_3_2_0`).
    unicodedata @inject,
    // `codecs` (issue #3203): Python 3.12 codec registry, error handlers,
    // BOM constants, incremental/stream/recoder classes, and standard text +
    // binary/text transform codecs.
    codecs @inject,
    // `token`, `tokenize`, and `ast` (issue #3204): Python 3.12 token constants,
    // stateful native PEP 701 tokenizer (`tokenize/lexer.rs`), and full Python
    // 3.12 AST parser, class hierarchy, visitors, literal_eval, and unparser.
    token @inject,
    tokenize @inject,
    ast @inject,
    // `ipaddress`, `socket`, and `selectors` (issue #3205): Python 3.12
    // networking foundation modules — IPv4/IPv6 address/network/interface
    // arithmetic in `ipaddress_py.py`, Rust `std::net`-backed `socket` in
    // `socket.rs`/`socket_py.py`, and OS-backed I/O multiplexing in
    // `selectors.rs`/`selectors_py.py`.
    ipaddress @inject,
    socket @inject,
    selectors @inject,
    // `socketserver` (issue #3223): Python 3.12 generic TCP/UDP socket server
    // framework (`BaseServer`, `TCPServer`, `UDPServer`, `ThreadingMixIn`,
    // `BaseRequestHandler`, `StreamRequestHandler`, `DatagramRequestHandler`).
    socketserver @inject,
    // `copyreg`, `graphlib`, `threading`, and `queue` (issue #3208): Python
    // 3.12 pickle/copy extension registry, topological graph sorter,
    // single-interpreter synchronization primitives, and synchronized queues.
    copyreg @inject,
    graphlib @inject,
    threading @inject,
    queue @inject,
    // `stat`, `platform`, `atexit`, and `signal` (issue #3209): Python 3.12
    // OS/system utility stdlib modules.
    stat @inject,
    platform @inject,
    atexit,
    signal @inject,
    // `http`, `http.client`, `http.server`, and `http.cookies` (issue #3207):
    // Python 3.12 HTTP enums (`HTTPStatus`, `HTTPMethod`), HTTP/1.1 client,
    // socket-backed HTTP server and static file handler, and RFC cookie
    // serialization/parsing.
    "http.client" as http_client @inject,
    "http.server" as http_server @inject,
    "http.cookies" as http_cookies @inject,
    http @inject,
    // `email` (`email.utils`, `email.header`, `email.policy`, `email.message`,
    // `email.parser`) and `mimetypes` (issue #3211): Python 3.12 RFC 2822/MIME
    // email parsing, header/address/date utilities, policies, message objects,
    // and filename/URL MIME type mapping.
    "email.utils" as email_utils @inject,
    "email.header" as email_header @inject,
    "email.policy" as email_policy @inject,
    "email.message" as email_message @inject,
    "email.parser" as email_parser @inject,
    email @inject,
    mimetypes @inject,
    // `xml`, `xml.etree`, `xml.etree.ElementPath`, and `xml.etree.ElementTree`
    // (issue #3210): Python 3.12 XML ElementTree API, streaming XML 1.0 parser
    // and serializer, XPath subset evaluator, and C14N 2.0 canonicalizer.
    "xml.etree.ElementPath" as xml_etree_elementpath @inject,
    "xml.etree.ElementTree" as xml_etree_elementtree @inject,
    "xml.etree" as xml_etree,
    xml,
    // `sqlite3` and `sqlite3.dbapi2` (issue #3225): Python 3.12 DB-API 2.0
    // SQLite interface backed by bundled SQLite (`rusqlite::ffi`) in
    // `sqlite3.rs` plus the full public surface in `sqlite3_py.py`.
    sqlite3 @inject,
    "sqlite3.dbapi2" as sqlite3_dbapi2 @inject,
    // `unittest.mock` and `doctest` (issue #3222): Python 3.12 mocking/patching
    // library (`Mock`, `MagicMock`, `AsyncMock`, `PropertyMock`, `patch`,
    // `create_autospec`, `mock_open`, `seal`) and docstring example runner
    // (`DocTestParser`, `DocTestFinder`, `DocTestRunner`, `OutputChecker`,
    // `testmod`, `testfile`, `DocTestSuite`, `DocFileSuite`).
    "unittest.mock" as unittest_mock @inject,
    doctest @inject,
    // `timeit`, `opcode`, and `dis` (issue #3227): Python 3.12 benchmarking
    // (`Timer`, `timeit`, `repeat`, `main`), Python 3.12 bytecode opcode
    // metadata and `stack_effect`, and bytecode disassembly (`Instruction`,
    // `Positions`, `Bytecode`, `dis`, `distb`, `disassemble`,
    // `get_instructions`, `show_code`, `code_info`, `pretty_flags`,
    // `findlabels`, `findlinestarts`).
    timeit @inject,
    opcode @inject,
    dis @inject,
    // `codeop`, `code`, `cmd`, and `rlcompleter` (issue #3228): Python 3.12
    // interactive interpreter, command-shell, and readline-completion stdlib
    // modules.
    codeop @inject,
    code @inject,
    cmd @inject,
    rlcompleter @inject,
    // `zoneinfo` (PEP 615), `locale`, and `gettext` (issue #3226): Python 3.12
    // IANA time zone support (TZif v1/v2/v3 parser + POSIX TZ rules), locale
    // conventions/formatting/normalization, and GNU gettext `.mo` catalogs.
    zoneinfo @inject,
    locale @inject,
    gettext @inject,
    // `linecache`, `pkgutil`, `runpy`, and `sysconfig` (issue #3229): Python
    // 3.12 source-line cache, package discovery/resolution utilities, module/
    // script runner, and build/install-scheme configuration interface.
    linecache @inject,
    pkgutil @inject,
    runpy @inject,
    sysconfig @inject,
    // `colorsys`, `fileinput`, `filecmp`, `netrc`, `quopri`, and `getopt`
    // (issue #3230): Python 3.12 compact standard library utility modules.
    colorsys @inject,
    fileinput @inject,
    filecmp @inject,
    netrc @inject,
    quopri @inject,
    getopt @inject,
    // `keyword`, `reprlib`, `sched`, `pickletools`, `optparse`, `this`, and
    // `antigravity` (issue #3248): 100% native-Rust Python 3.12 compact stdlib
    // modules built with PyRust's expanded macro toolkit (no `@inject`).
    keyword,
    reprlib,
    sched,
    pickletools,
    optparse,
    this,
    antigravity,
    // `select`, `mmap`, `getpass`, and `webbrowser` (issue #3253): 100% native
    // Rust stdlib modules built with PyRust's expanded macro toolkit.
    select,
    mmap,
    getpass,
    webbrowser,
    // `_sitebuiltins` and `site` (issue #3243): 100% native-Rust CPython 3.12
    // site configuration, `.pth` processing, and interactive builtins.
    _sitebuiltins,
    site,
    // `bdb`, `pdb`, `_lsprof`, `cProfile`, `profile`, `pstats`, and `trace`
    // (issue #3247): 100% native-Rust Python 3.12 debugging, profiling, and
    // execution-tracing stdlib modules built on PyRust's live `sys.settrace` /
    // `sys.setprofile` hooks and expanded macro toolkit (no `@inject`).
    bdb,
    pdb,
    _lsprof,
    "cProfile" as cprofile,
    profile,
    pstats,
    trace,
    // `gc`, `_tracemalloc`, `tracemalloc`, `faulthandler`, and `marshal`
    // (issue #3246): 100% native-Rust Python 3.12 runtime modules (no `@inject`).
    gc,
    _tracemalloc,
    tracemalloc,
    faulthandler,
    marshal,
    // `stringprep`, `encodings` (+ `encodings.aliases`, `encodings.punycode`,
    // `encodings.idna`, and macro-generated standard codec submodules), and
    // native `_*` internal compatibility modules (issue #3250).
    stringprep,
    "encodings.aliases" as encodings_aliases,
    "encodings.punycode" as encodings_punycode,
    "encodings.idna" as encodings_idna,
    encodings,
    _imp,
    _compression,
    _compat_pickle,
    _string,
    _strptime,
    // `pydoc_data`, `pydoc_data.topics`, and `pydoc` (issue #3255): 100%
    // native-Rust Python 3.12 documentation generator and help topic database
    // built with PyRust's expanded macro toolkit (no `@inject`).
    "pydoc_data.topics" as pydoc_data_topics,
    pydoc_data,
    pydoc,
}

include!("finalization.rs");

#[cfg(test)]
mod macro_tests;
