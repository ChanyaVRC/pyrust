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
//! - contributes one row to the registry's module table (`MODULE_REGS`), so
//!   its callables are registered lazily, the first time one is looked up,
//! - is wired into the `import` path (via `load_builtin_module`).

#[macro_use]
pub(crate) mod macros;
pub(crate) mod asyncio_rt;
pub(crate) mod asyncio_submodules;
pub(crate) mod ctypes_rt;
#[cfg(any(unix, test))]
pub(crate) mod curses_engine;
pub(crate) mod encodings_codecs;
pub(crate) mod internal_shims;
pub(crate) mod mp_rt;
pub(crate) mod ssl_engine;

use crate::builtin_registry::{BuiltinReg, ModuleRegs};
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
/// - `pub mod <ident> { … }` with the module's identity injected by
///   [`pyrust_module_identity!`] — `MODULE_NAME` (Python-level name for
///   `PyModule.name` + `import`), `FN_PREFIX`, and `__pyrust_qualname!`
///   (which prepends the prefix to a function's short name at compile time
///   to form its registration name) — followed by
///   `include!("bodies/<ident>.rs")`.
/// - one branch of [`load_builtin_module`] keyed on the Python-level name,
/// - one row of [`MODULE_REGS`] pointing at the module's `REGS` slice.
///
/// Body files therefore never need to know their own module name — the
/// macro injects the identity and the body's `pyrust_module!` reads it via
/// the surrounding scope.
/// Helper emitting one `pub mod <ident>` item per entry in
/// [`pyrust_builtin_modules!`]. Expanding each entry independently keeps
/// macro recursion depth O(1) regardless of how many built-in modules exist.
macro_rules! pyrust_builtin_mod_def {
    ($(#[cfg($($cfg:tt)*)])* @flat $ident:ident $(@inject)?) => {
        $(#[cfg($($cfg)*)])*
        pub mod $ident {
            pyrust_module_identity!(@flat stringify!($ident));

            include!(concat!("bodies/", stringify!($ident), ".rs"));
        }
    };
    ($(#[cfg($($cfg:tt)*)])* $py_name:literal as $ident:ident $(@inject)?) => {
        $(#[cfg($($cfg)*)])*
        pub mod $ident {
            pyrust_module_identity!($py_name);

            include!(concat!("bodies/", stringify!($ident), ".rs"));
        }
    };
    ($(#[cfg($($cfg:tt)*)])* $ident:ident $(@inject)?) => {
        $(#[cfg($($cfg)*)])*
        pub mod $ident {
            pyrust_module_identity!(stringify!($ident));

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

/// Helper expanding one entry's lazily registered `REGS` slice.
///
/// A `@flat` module registers under bare names, which no module prefix can
/// resolve; its callables belong to the registry's always-available tier
/// instead (see [`TIER1_REG_FAMILIES`]), so its row here is empty.
macro_rules! pyrust_builtin_mod_regs {
    (@flat $ident:ident) => {
        &[]
    };
    ($ident:ident) => {
        $ident::REGS
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

        /// One row per module declared here: its Python-level name and its
        /// `REGS` slice.  The registry sorts these rows by name at compile
        /// time and reads a module's slice only once one of its callables
        /// is looked up, so a new entry in this macro is lazily registered
        /// with no further wiring.
        pub(crate) static MODULE_REGS: &[ModuleRegs] = &[
            $(
                $(#[cfg($($cfg)*)])*
                ModuleRegs {
                    name: pyrust_builtin_mod_name!($($py_name as)? $ident),
                    regs: pyrust_builtin_mod_regs!($(@$flat)? $ident),
                },
            )*
        ];

        /// Resolve `import name` to its built-in module value, if any.
        /// Replaces the hand-maintained `match name { "math" => …, … }`
        /// previously sitting inside `env.rs::load_module`.
        pub fn load_builtin_module(name: &str) -> Option<Value> {
            $(
                $(#[cfg($($cfg)*)])*
                {
                    if name == pyrust_builtin_mod_name!($($py_name as)? $ident) {
                        #[cfg(test)]
                        record_builtin_module_load(pyrust_builtin_mod_name!($($py_name as)? $ident));
                        return Some($ident::module());
                    }
                }
            )*
            if let Some(m) = asyncio_submodules::load_builtin_module(name) {
                return Some(m);
            }
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
            asyncio_submodules::is_builtin_module(name)
                || encodings_codecs::is_builtin_module(name)
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
                    | "venv"
                    | "wsgiref"
                    | "xmlrpc"
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
            if name == "sys" {
                // `zipimport` (and its `sys.path_hooks` entry) is installed
                // the first time a script could observe it, not on every
                // startup (issue #3293); see `post_load_deferred`.
                module.borrow().set_deferred_init(true);
                return Ok(());
            }
            if name == "py_compile" {
                let module_val = Value::py_module(std::rc::Rc::clone(module));
                py_compile::prepare_module(interp, &module_val);
                return Ok(());
            }
            if name == "site" {
                let module_val = Value::py_module(std::rc::Rc::clone(module));
                return site::post_load_init(interp, &module_val);
            }
            if name == "warnings" {
                return warnings::post_load_init(interp);
            }
            match name {
                "os" => return os::post_load_init(interp, module),
                "numbers" => return numbers::post_load_init(interp, module),
                "array" => return array::post_load_init(interp, module),
                "string" => return string::post_load_init(interp, module),
                "html" => return html::post_load_init(interp, module),
                "csv" => return csv::post_load_init(interp, module),
                "fractions" => return fractions::post_load_init(interp, module),
                #[cfg(unix)]
                "posix" => return posix::post_load_init(interp, module),
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
                "logging.handlers" => return logging_handlers::post_load_init(interp, module),
                "logging.config" => return logging_config::post_load_init(interp, module),
                "json" => return json::post_load_init(interp, module),
                "json.scanner" => return json_scanner::post_load_init(interp, module),
                "json.decoder" => return json_decoder::post_load_init(interp, module),
                "json.encoder" => return json_encoder::post_load_init(interp, module),
                "wsgiref.simple_server" => {
                    return wsgiref_simple_server::post_load_init(interp, module);
                }
                "wsgiref.types" => return wsgiref_types::post_load_init(interp, module),
                "xmlrpc.server" => return xmlrpc_server::post_load_init(interp, module),
                "ssl" => return ssl::post_load_init(interp, module),
                "_ssl" => return _ssl::post_load_init(interp, module),
                "email.errors" => return email_errors::post_load_init(interp, module),
                "email.contentmanager" => {
                    return email_contentmanager::post_load_init(interp, module);
                }
                "email.headerregistry" => {
                    return email_headerregistry::post_load_init(interp, module);
                }
                "email.mime.base" => return email_mime_base::post_load_init(interp, module),
                "email.mime.nonmultipart" => {
                    return email_mime_nonmultipart::post_load_init(interp, module);
                }
                "email.mime.multipart" => {
                    return email_mime_multipart::post_load_init(interp, module);
                }
                "email.mime.text" => return email_mime_text::post_load_init(interp, module),
                "email.mime.application" => {
                    return email_mime_application::post_load_init(interp, module);
                }
                "email.mime.audio" => return email_mime_audio::post_load_init(interp, module),
                "email.mime.image" => return email_mime_image::post_load_init(interp, module),
                "email.mime.message" => return email_mime_message::post_load_init(interp, module),
                "mailbox" => return mailbox::post_load_init(interp, module),
                "pyexpat" => return pyexpat::post_load_init(interp, module),
                "xml.parsers.expat" => return xml_parsers_expat::post_load_init(interp, module),
                "xml.sax" => return xml_sax::post_load_init(interp, module),
                "xml.sax.xmlreader" => return xml_sax_xmlreader::post_load_init(interp, module),
                "xml.sax.saxutils" => return xml_sax_saxutils::post_load_init(interp, module),
                "xml.sax.expatreader" => return xml_sax_expatreader::post_load_init(interp, module),
                "xml.dom" => return xml_dom::post_load_init(interp, module),
                "xml.dom.minicompat" => return xml_dom_minicompat::post_load_init(interp, module),
                "xml.dom.xmlbuilder" => return xml_dom_xmlbuilder::post_load_init(interp, module),
                "xml.dom.minidom" => return xml_dom_minidom::post_load_init(interp, module),
                "xml.dom.expatbuilder" => {
                    return xml_dom_expatbuilder::post_load_init(interp, module);
                }
                "xml.dom.pulldom" => return xml_dom_pulldom::post_load_init(interp, module),
                "xml.etree.ElementPath" => {
                    return xml_etree_elementpath::post_load_init(interp, module);
                }
                "xml.etree.ElementInclude" => {
                    return xml_etree_elementinclude::post_load_init(interp, module);
                }
                "xml.etree.cElementTree" => {
                    return xml_etree_celementtree::post_load_init(interp, module);
                }
                "asyncio" => return asyncio::post_load_init(interp, module),
                "ftplib" => return ftplib::post_load_init(interp, module),
                "smtplib" => return smtplib::post_load_init(interp, module),
                "poplib" => return poplib::post_load_init(interp, module),
                "imaplib" => return imaplib::post_load_init(interp, module),
                "_ctypes" => return _ctypes::post_load_init(interp, module),
                "ctypes._endian" => return ctypes_endian::post_load_init(interp, module),
                "ctypes" => return ctypes::post_load_init(interp, module),
                "ctypes.wintypes" => return ctypes_wintypes::post_load_init(interp, module),
                "concurrent.futures.thread" => {
                    return concurrent_futures_thread::post_load_init(interp, module);
                }
                "concurrent.futures.process" => {
                    return concurrent_futures_process::post_load_init(interp, module);
                }
                "concurrent.futures" => {
                    return concurrent_futures::post_load_init(interp, module);
                }
                "venv" => return venv::post_load_init(interp, module),
                #[cfg(unix)]
                "curses" => return curses::post_load_init(interp, module),
                #[cfg(unix)]
                "curses.has_key" => return curses_has_key::post_load_init(interp, module),
                #[cfg(unix)]
                "curses.panel" => return curses_panel::post_load_init(interp, module),
                "multiprocessing.util" => {
                    return multiprocessing_util::post_load_init(interp, module);
                }
                "multiprocessing.process" => {
                    return multiprocessing_process::post_load_init(interp, module);
                }
                "multiprocessing.connection" => {
                    return multiprocessing_connection::post_load_init(interp, module);
                }
                "multiprocessing.queues" => {
                    return multiprocessing_queues::post_load_init(interp, module);
                }
                "multiprocessing.synchronize" => {
                    return multiprocessing_synchronize::post_load_init(interp, module);
                }
                "multiprocessing.spawn" => {
                    return multiprocessing_spawn::post_load_init(interp, module);
                }
                "multiprocessing.context" => {
                    return multiprocessing_context::post_load_init(interp, module);
                }
                "multiprocessing.shared_memory" => {
                    return multiprocessing_shared_memory::post_load_init(interp, module);
                }
                "multiprocessing.sharedctypes" => {
                    return multiprocessing_sharedctypes::post_load_init(interp, module);
                }
                "multiprocessing.reduction" => {
                    return multiprocessing_reduction::post_load_init(interp, module);
                }
                "multiprocessing.managers" => {
                    return multiprocessing_managers::post_load_init(interp, module);
                }
                "multiprocessing.pool" => {
                    return multiprocessing_pool::post_load_init(interp, module);
                }
                "multiprocessing.dummy" => {
                    return multiprocessing_dummy::post_load_init(interp, module);
                }
                "multiprocessing" => {
                    return multiprocessing::post_load_init(interp, module);
                }
                _ => {}
            }
            if asyncio_submodules::post_load_init(name, interp, module)? {
                return Ok(());
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
    // `os` binds `os.path` via `os::post_load_init` using `interp.load_module("os.path")`.
    os,
    // Generation-bound factory callables and dynamic helper classes are
    // installed after the fresh module enters this Interpreter's import cache.
    functools @inject,
    // `operator` (issues #2514, #3364): 100% native Rust in `bodies/operator.rs`;
    // dunder aliases, `index` and `__module__` are wired by its
    // `prepare_module` hook, with no Python-source injection.
    operator,
    itertools,
    // `collections` Python-source members (issue #1884) are injected by the
    // `@inject` hook; `inject_python_members` also tags the public classes
    // with `__module__` / `__class_getitem__` (issues #2228 / #2603).
    collections @inject,
    "collections.abc" as collections_abc @inject,
    // native `IOBase` family plus `StringIO` / `BytesIO` (issue #3342).
    io,
    // `typing` Python-source members (issue #2516) injected via `@inject`.
    typing @inject,
    // `copy` (issues #2958, #3346): 100% native Rust in `bodies/copy.rs`; the
    // reduction reconstruction (`copy._reconstruct`) lives in
    // `bodies/copy/reconstruct.rs`, with no Python-source injection.
    copy,
    pathlib,
    // `string` (issues #2515, #3401): 100% native Rust in `bodies/string.rs`;
    // `Template.flags` / `Template.pattern` and the `_string` / `_re` /
    // `_ChainMap` imports are wired by its `post_load_init` hook, with no
    // Python-source injection.
    string,
    // `contextlib`: implemented 100% natively in Rust (issue #3388) — the ABCs,
    // decorators, `ExitStack` / `AsyncExitStack`, and every context manager.
    contextlib,
    "__future__" as future,
    warnings,
    // 100% native-Rust Python 3.12 `json` package (issues #2620, #3413): the
    // `JSONEncoder` / `JSONDecoder` / `JSONDecodeError` classes live in
    // `json.encoder` / `json.decoder` and are re-exported by `post_load_init`
    // (no `@inject`).
    json,
    // 100% native-Rust Python 3.12 `asyncio` (issues #2281, #3154, #3254):
    // event loop, streams, transports/protocols, TLS, Runner, subprocesses,
    // and 26 `asyncio.*` submodules (no `@inject`).
    asyncio,
    // `abc` (issues #2612, #3293): native Rust `ABCMeta`, `ABC`,
    // `abstractmethod`, … — no Python source is compiled at import.
    abc,
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
    // calendar conversions; `struct_time` is a native `pyrust_struct_seq!`
    // type (issue #3307) — no `@inject` Python body.
    time,
    // `textwrap` (issue #2786): a pure-Python module (`textwrap_py.py`) —
    // `TextWrapper`, `wrap`, `fill`, `shorten`, `dedent`, `indent` — injected
    // by the `@inject` post-load hook; the native body is empty.
    textwrap @inject,
    // `bisect` (issues #2784, #3310): 100% native array-bisection algorithms
    // (`bisect_left` / `bisect_right` / `insort_left` / `insort_right` plus the
    // `bisect` / `insort` aliases) in `bisect.rs`; no Python-source injection.
    bisect,
    // `heapq` (issues #2784, #3363): 100% native heap-queue algorithms
    // (`heappush` / `heappop` / `heapify` / `merge` / `nlargest` / ...) in
    // `heapq.rs`; no Python-source injection.
    heapq,
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
    // `csv` (issues #2808, #3340, #3400): the whole CPython 3.12 `csv` /
    // `_csv` surface — the `_csv.c` reader state machine and writer, the
    // dialect type and registry, `field_size_limit`, the `Dialect` family,
    // `DictReader` / `DictWriter` and `Sniffer` — implemented 100% natively.
    csv,
    // `decimal` (issue #2806): a port of CPython 3.12's pure-Python
    // `_pydecimal.py` (`decimal_py.py`) — `Decimal`, `Context`,
    // `getcontext` / `setcontext` / `localcontext`, the `ROUND_*` constants,
    // and the `DecimalException` hierarchy — injected by the `@inject`
    // post-load hook.  The native body is empty.
    decimal @inject,
    // `fractions` (issues #2810, #3412): 100% native Rust in
    // `bodies/fractions.rs` — the `Fraction` rational-number class over
    // `BigInt` numerator/denominator; `post_load_init` makes it a
    // `numbers.Rational` ABC subclass and restores CPython's namespace order.
    fractions,
    // `struct` (issue #3309): 100% native `error`, `Struct`, and top-level
    // functions in `struct_mod.rs`.  The Rust ident is `struct_mod` because
    // `struct` is a keyword.
    "struct" as struct_mod,
    // `array` (issues #2809, #3389): 100% native Rust in `bodies/array.rs` —
    // the `array` type, the hidden `arrayiterator`, `typecodes` and
    // `_array_reconstructor`; `post_load_init` wires `ArrayType`, the CPython
    // namespace order and the `MutableSequence` registration.
    array,
    // `datetime` (issue #2805): date/time/datetime/timedelta/tzinfo/timezone
    // types injected from `datetime_py.py` by the `@inject` post-load hook.
    datetime @inject,
    // `binascii` (issues #3145, #3308): 100% native hex, Base64, CRC-32,
    // CRC-HQX, uu, and quoted-printable codecs plus `Error` / `Incomplete`
    // in `binascii.rs`; no Python-source injection.
    binascii,
    // `base64` (issue #3145): pure-Python Base16, Base32, Base32hex, Base64,
    // Base85, Ascii85, and RFC 2045 encoders/decoders injected from
    // `base64_py.py` by the `@inject` post-load hook.
    base64 @inject,
    // `hashlib` (issues #3146, #3372): pure-Rust cryptographic hash primitives
    // (MD5, SHA-1, SHA-2, SHA-3/SHAKE, BLAKE2b/s, PBKDF2-HMAC, scrypt) and the
    // whole CPython 3.12 `hashlib` surface implemented 100% natively in Rust.
    hashlib,
    // `hmac` (issues #3146, #3444): the whole CPython 3.12 `hmac` surface
    // (`HMAC`, `new`, `digest`, `compare_digest`) implemented 100% natively in
    // Rust on top of the `hashlib` engine.
    hmac,
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
    // `numbers` (issues #3147, #3371): PEP 3141 numeric tower ABCs (`Number`,
    // `Complex`, `Real`, `Rational`, `Integral`) implemented 100% natively in
    // Rust; no Python-source injection.
    numbers,
    // `difflib` (issue #3147): pure-Python sequence matching and delta helpers
    // (`SequenceMatcher`, `Differ`, `unified_diff`, `context_diff`, `ndiff`,
    // `restore`, `diff_bytes`, `get_close_matches`, `HtmlDiff`, `Match`)
    // injected from `difflib_py.py`.
    difflib @inject,
    // `shlex` (issues #3147, #3356): shell-like lexical analyzer (`shlex`,
    // `split`, `join`, `quote`) implemented 100% natively in Rust.
    shlex,
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
    "urllib.response" as urllib_response,
    "urllib.error" as urllib_error,
    "urllib.request" as urllib_request @inject,
    "urllib.robotparser" as urllib_robotparser,
    urllib,
    // `html.entities`, `html.parser`, and `html` (issues #3149, #3446): static
    // HTML4 / HTML5 character reference tables in `html_entities.rs`, `html`
    // itself (`escape` / `unescape`) 100% native in `html.rs`, and streaming
    // `HTMLParser` injected from `html_parser_py.py`.
    "html.entities" as html_entities,
    "html.parser" as html_parser @inject,
    html,
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
    // `zlib` (issues #3186, #3324): 100% native pure-Rust RFC 1950/1951/1952
    // compression engine in `zlib/` plus the CPython 3.12 `error`,
    // `Compress`, and `Decompress` surfaces in `zlib.rs`; no Python-source
    // injection.
    zlib,
    // `gzip` (issue #3186): CPython 3.12 archive surface injected from
    // `gzip_py.py` on top of `zlib`.
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
    // `unicodedata` (issues #3203, #3323): 100% native Rust Unicode 15.0.0
    // character database (`category`, `bidirectional`, `combining`,
    // `east_asian_width`, `mirrored`, `decomposition`, `decimal`, `digit`,
    // `numeric`, `name`, `lookup`, `normalize`, `is_normalized`,
    // `unidata_version`, native `UCD` class, `ucd_3_2_0`); no Python injection.
    unicodedata,
    // `codecs` (issue #3203): Python 3.12 codec registry, error handlers,
    // BOM constants, incremental/stream/recoder classes, and standard text +
    // binary/text transform codecs.
    codecs @inject,
    // `token` (issues #3204, #3348): 100% native Rust token constants in
    // `token.rs`.  `tokenize` and `ast` (issue #3204): stateful native PEP 701
    // tokenizer (`tokenize/lexer.rs`) and full Python 3.12 AST parser, class
    // hierarchy, visitors, literal_eval, and unparser.
    token,
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
    // `stat` (issues #3209, #3355: 100% native Rust in `stat.rs`), `platform`,
    // `atexit`, and `signal` (issue #3209): Python 3.12
    // OS/system utility stdlib modules.
    stat,
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
    "http.cookiejar" as http_cookiejar,
    http @inject,
    // `email` (`email.utils`, `email.header`, `email.policy`, `email.message`,
    // `email.parser`) and `mimetypes` (issue #3211), plus all remaining 19
    // native-Rust `email.*` submodules and `mailbox` (issue #3257).
    "email.errors" as email_errors,
    "email.base64mime" as email_base64mime,
    "email.quoprimime" as email_quoprimime,
    "email.encoders" as email_encoders,
    "email.charset" as email_charset,
    "email.iterators" as email_iterators,
    "email.feedparser" as email_feedparser,
    "email.generator" as email_generator,
    "email.contentmanager" as email_contentmanager,
    "email.headerregistry" as email_headerregistry,
    "email.mime" as email_mime,
    "email.mime.base" as email_mime_base,
    "email.mime.nonmultipart" as email_mime_nonmultipart,
    "email.mime.multipart" as email_mime_multipart,
    "email.mime.text" as email_mime_text,
    "email.mime.application" as email_mime_application,
    "email.mime.audio" as email_mime_audio,
    "email.mime.image" as email_mime_image,
    "email.mime.message" as email_mime_message,
    "email.utils" as email_utils @inject,
    "email.header" as email_header @inject,
    "email.policy" as email_policy @inject,
    "email.message" as email_message @inject,
    "email.parser" as email_parser @inject,
    email @inject,
    mailbox,
    mimetypes @inject,
    // `pyexpat`, `xml.parsers.expat`, `xml.sax.*`, `xml.dom.*`, and
    // `xml.etree.*` (issues #3210, #3260): 100% pure-Rust streaming XML/Expat
    // parser, SAX2 drivers/utilities, W3C DOM (minidom/pulldom/expatbuilder),
    // XInclude processor, and ElementTree.
    "pyexpat.errors" as pyexpat_errors,
    "pyexpat.model" as pyexpat_model,
    pyexpat,
    "xml.parsers.expat" as xml_parsers_expat,
    "xml.parsers" as xml_parsers,
    "xml.sax.handler" as xml_sax_handler,
    "xml.sax.xmlreader" as xml_sax_xmlreader,
    "xml.sax.saxutils" as xml_sax_saxutils,
    "xml.sax.expatreader" as xml_sax_expatreader,
    "xml.sax" as xml_sax,
    "xml.dom.minicompat" as xml_dom_minicompat,
    "xml.dom.domreg" as xml_dom_domreg,
    "xml.dom.NodeFilter" as xml_dom_nodefilter,
    "xml.dom.xmlbuilder" as xml_dom_xmlbuilder,
    "xml.dom.minidom" as xml_dom_minidom,
    "xml.dom.expatbuilder" as xml_dom_expatbuilder,
    "xml.dom.pulldom" as xml_dom_pulldom,
    "xml.dom" as xml_dom,
    "xml.etree.ElementPath" as xml_etree_elementpath,
    "xml.etree.ElementTree" as xml_etree_elementtree @inject,
    "xml.etree.ElementInclude" as xml_etree_elementinclude,
    "xml.etree.cElementTree" as xml_etree_celementtree,
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
    opcode,
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
    // `linecache` (issues #3229, #3445): the whole CPython 3.12 source-line
    // cache implemented 100% natively in Rust (`linecache.rs`).
    linecache,
    // `pkgutil`, `runpy`, and `sysconfig` (issue #3229): Python 3.12 package
    // discovery/resolution utilities, module/script runner, and
    // build/install-scheme configuration interface.
    pkgutil @inject,
    runpy @inject,
    sysconfig @inject,
    // `colorsys` (issues #3230, #3329) and `getopt` (issues #3230, #3330): 100%
    // native Rust in `colorsys.rs` / `getopt.rs`; no Python-source injection.
    // `fileinput`, `filecmp`, and `quopri` (issue #3230): Python 3.12 compact
    // standard library utility modules.  `netrc` (issues #3230, #3451) is 100%
    // native Rust in `netrc.rs`.
    colorsys,
    fileinput @inject,
    filecmp @inject,
    netrc,
    quopri @inject,
    getopt,
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
    // `ssl` and `_ssl` (issue #3252): 100% native-Rust Python 3.12 TLS/SSL
    // modules backed by pure-Rust `rustls` (no OpenSSL, no `@inject`).
    ssl,
    _ssl,
    _string,
    _strptime,
    // `pydoc_data`, `pydoc_data.topics`, and `pydoc` (issue #3255): 100%
    // native-Rust Python 3.12 documentation generator and help topic database
    // built with PyRust's expanded macro toolkit (no `@inject`).
    "pydoc_data.topics" as pydoc_data_topics,
    pydoc_data,
    pydoc,
    // `logging.handlers`, `logging.config`, `json.{scanner,decoder,encoder,tool}`,
    // and `sqlite3.dump` (issue #3251): 100% native-Rust Python 3.12 stdlib
    // submodules built with PyRust's expanded macro toolkit (no `@inject`).
    "logging.handlers" as logging_handlers,
    "logging.config" as logging_config,
    "json.scanner" as json_scanner,
    "json.decoder" as json_decoder,
    "json.encoder" as json_encoder,
    "json.tool" as json_tool,
    "sqlite3.dump" as sqlite3_dump,
    // `_symtable`, `symtable`, `py_compile`, `compileall`, `zipimport`, `zipapp`,
    // `modulefinder`, `pyclbr`, and `tabnanny` (issue #3249): 100% native-Rust
    // Python 3.12 compiler, packaging, and source-analysis modules (no `@inject`).
    _symtable,
    symtable,
    py_compile,
    compileall,
    zipimport,
    zipapp,
    modulefinder,
    pyclbr,
    tabnanny,
    // `wsgiref` (`.util`, `.headers`, `.handlers`, `.simple_server`,
    // `.validate`, `.types`) and `xmlrpc` (`.client`, `.server`) (issue #3256):
    // 100% native-Rust Python 3.12 web & RPC stdlib modules built with PyRust's
    // expanded macro toolkit (no `@inject`).
    "wsgiref.util" as wsgiref_util,
    "wsgiref.headers" as wsgiref_headers,
    "wsgiref.handlers" as wsgiref_handlers,
    "wsgiref.simple_server" as wsgiref_simple_server,
    "wsgiref.validate" as wsgiref_validate,
    "wsgiref.types" as wsgiref_types,
    wsgiref,
    "xmlrpc.client" as xmlrpc_client,
    "xmlrpc.server" as xmlrpc_server,
    xmlrpc,
    // `dbm`, `dbm.dumb`, `shelve`, `plistlib`, and `wave` (issue #3259): 100%
    // native-Rust Python 3.12 persistence and binary format stdlib modules
    // built with PyRust's expanded macro toolkit (no `@inject`).
    "dbm.dumb" as dbm_dumb,
    dbm,
    shelve,
    plistlib,
    wave,
    // POSIX OS modules (issue #3261): 100% native-Rust Unix-only Python 3.12
    // stdlib modules gated with `#[cfg(unix)]` (raising `ModuleNotFoundError`
    // on Windows matching CPython 3.12).
    #[cfg(unix)]
    posix,
    #[cfg(unix)]
    fcntl,
    #[cfg(unix)]
    termios,
    #[cfg(unix)]
    tty,
    #[cfg(unix)]
    pty,
    #[cfg(unix)]
    pwd,
    #[cfg(unix)]
    grp,
    #[cfg(unix)]
    resource,
    #[cfg(unix)]
    syslog,
    // `ftplib`, `smtplib`, `poplib`, and `imaplib` (issue #3258): 100% native-Rust
    // Python 3.12 internet protocol client modules built over `socket` and `ssl`
    // using PyRust's expanded macro toolkit (no `@inject`).
    ftplib,
    smtplib,
    poplib,
    imaplib,
    // Windows-only OS modules (issue #3262): 100% native-Rust Python 3.12
    // modules gated with `#[cfg(windows)]` so Unix raises `ModuleNotFoundError`.
    #[cfg(windows)]
    nt,
    #[cfg(windows)]
    msvcrt,
    #[cfg(windows)]
    winreg,
    #[cfg(windows)]
    winsound,
    // `_ctypes`, `ctypes`, `ctypes._endian`, `ctypes.util`, and `ctypes.wintypes`
    // (issue #3265): 100% native-Rust Python 3.12 FFI with zero C libffi dependency.
    _ctypes,
    "ctypes._endian" as ctypes_endian,
    "ctypes.util" as ctypes_util,
    "ctypes.wintypes" as ctypes_wintypes,
    ctypes,
    // `concurrent` and `concurrent.futures` (`.{_base,thread,process}`)
    // (issue #3263): 100% native-Rust Python 3.12 thread-pool and process-pool
    // executors and futures built with PyRust's expanded macro toolkit.
    "concurrent.futures._base" as concurrent_futures_base,
    "concurrent.futures.thread" as concurrent_futures_thread,
    "concurrent.futures.process" as concurrent_futures_process,
    "concurrent.futures" as concurrent_futures,
    concurrent,
    // `venv` and `ensurepip` (issue #3267): 100% native-Rust Python 3.12
    // virtual environment creation and pip bootstrapping modules (no `@inject`).
    venv,
    "ensurepip.__main__" as ensurepip_main,
    "ensurepip._uninstall" as ensurepip_uninstall,
    ensurepip,
    // `readline` and `curses` ecosystem (issue #3266): 100% native-Rust
    // Unix-only Python 3.12 stdlib modules (zero `libncurses` C linkage).
    #[cfg(unix)]
    readline,
    #[cfg(unix)]
    _curses,
    #[cfg(unix)]
    _curses_panel,
    #[cfg(unix)]
    curses,
    #[cfg(unix)]
    "curses.ascii" as curses_ascii,
    #[cfg(unix)]
    "curses.has_key" as curses_has_key,
    #[cfg(unix)]
    "curses.panel" as curses_panel,
    #[cfg(unix)]
    "curses.textpad" as curses_textpad,
    // `_multiprocessing` and `multiprocessing` package (issue #3264): 100%
    // native-Rust Python 3.12 process spawning/forking, IPC, queues, locks,
    // shared memory, managers, pools, and `multiprocessing.dummy` (no `@inject`).
    _multiprocessing,
    "multiprocessing.util" as multiprocessing_util,
    "multiprocessing.process" as multiprocessing_process,
    "multiprocessing.connection" as multiprocessing_connection,
    "multiprocessing.queues" as multiprocessing_queues,
    "multiprocessing.synchronize" as multiprocessing_synchronize,
    "multiprocessing.spawn" as multiprocessing_spawn,
    "multiprocessing.context" as multiprocessing_context,
    "multiprocessing.shared_memory" as multiprocessing_shared_memory,
    "multiprocessing.sharedctypes" as multiprocessing_sharedctypes,
    "multiprocessing.heap" as multiprocessing_heap,
    "multiprocessing.reduction" as multiprocessing_reduction,
    "multiprocessing.resource_tracker" as multiprocessing_resource_tracker,
    "multiprocessing.resource_sharer" as multiprocessing_resource_sharer,
    "multiprocessing.managers" as multiprocessing_managers,
    "multiprocessing.pool" as multiprocessing_pool,
    "multiprocessing.dummy.connection" as multiprocessing_dummy_connection,
    "multiprocessing.dummy" as multiprocessing_dummy,
    #[cfg(windows)]
    "multiprocessing.popen_spawn_win32" as multiprocessing_popen_spawn_win32,
    #[cfg(unix)]
    "multiprocessing.popen_spawn_posix" as multiprocessing_popen_spawn_posix,
    #[cfg(unix)]
    "multiprocessing.popen_fork" as multiprocessing_popen_fork,
    #[cfg(unix)]
    "multiprocessing.popen_forkserver" as multiprocessing_popen_forkserver,
    multiprocessing,
}

pub(crate) use concurrent_futures_base as bodies_helpers_cf_base;

/// Registrations available before any import: the `builtins` families.
///
/// This is the registry's first tier.  It is merged into one sorted table at
/// compile time, so resolving `len` or `list.__getitem__` never reads another
/// module's registrations.
pub(crate) const TIER1_REG_FAMILIES: &[&[BuiltinReg]] = builtins::REG_FAMILIES;

/// First-tier registrations that belong to no module: the shared
/// `_structseq` hooks, reachable as soon as `sys` builds a struct sequence.
pub(crate) const TIER1_EXTRA_REGS: &[BuiltinReg] = macros::STRUCTSEQ_REGS;

/// Every module table that feeds the registry's lazy tier.  Each row names a
/// module and points at its `REGS` slice; nothing here reads a slice.
pub(crate) const MODULE_REG_TABLES: &[&[ModuleRegs]] = &[
    MODULE_REGS,
    asyncio_submodules::MODULE_REGS,
    encodings_codecs::MODULE_REGS,
    internal_shims::MODULE_REGS,
    #[cfg(test)]
    macro_tests::MODULE_REGS,
];

/// Every registration of every module, in declaration order.
///
/// Test-only: production code never materialises this list.  The registry's
/// tests use it to check that names are globally unique and that each one
/// resolves through the lazy lookup.
#[cfg(test)]
pub(crate) fn all_regs() -> Vec<BuiltinReg> {
    let mut all: Vec<BuiltinReg> = Vec::new();
    for family in TIER1_REG_FAMILIES {
        all.extend_from_slice(family);
    }
    all.extend_from_slice(TIER1_EXTRA_REGS);
    for table in MODULE_REG_TABLES {
        for module in *table {
            all.extend_from_slice(module.regs);
        }
    }
    all
}

include!("finalization.rs");

#[cfg(test)]
thread_local! {
    static BUILTIN_LOAD_COUNTS: std::cell::RefCell<std::collections::HashMap<&'static str, usize>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

#[cfg(test)]
fn record_builtin_module_load(name: &'static str) {
    BUILTIN_LOAD_COUNTS.with(|counts| {
        *counts.borrow_mut().entry(name).or_insert(0) += 1;
    });
}

#[cfg(test)]
fn reset_builtin_load_counts() {
    BUILTIN_LOAD_COUNTS.with(|counts| counts.borrow_mut().clear());
}

#[cfg(test)]
fn builtin_load_count(name: &str) -> usize {
    BUILTIN_LOAD_COUNTS.with(|counts| counts.borrow().get(name).copied().unwrap_or(0))
}

#[cfg(test)]
mod macro_tests;

#[cfg(test)]
mod lazy_facade_tests {
    use super::*;
    use crate::interpreter::Interpreter;

    #[test]
    fn modules_rs_has_zero_load_builtin_module_existence_probes() {
        let src = include_str!("../interpreter/runtime/namespaces/modules.rs");
        for (idx, line) in src.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }
            assert!(
                !(line.contains("load_builtin_module(")
                    && (line.contains(".is_some()") || line.contains(".is_none()"))),
                "modules.rs:{} uses allocating load_builtin_module(...).is_some()/is_none() probe; \
                 use is_builtin_module(...) instead: {line}",
                idx + 1
            );
        }
    }

    #[test]
    fn package_facades_have_zero_eager_super_module_calls() {
        let bodies_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builtin_modules/bodies");
        for entry in std::fs::read_dir(&bodies_dir).expect("read builtin_modules/bodies") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read facade body");
            for (idx, line) in src.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("//") {
                    continue;
                }
                let has_eager_submod = line
                    .split("super::")
                    .skip(1)
                    .any(|rest| rest.contains("::module()"));
                assert!(
                    !has_eager_submod,
                    "{}:{} eagerly constructs a submodule via super::*::module(); \
                     load submodules lazily or via interp.load_module(...) in post_load_init: {line}",
                    path.display(),
                    idx + 1
                );
            }
        }
    }

    #[test]
    fn startup_constructs_os_and_os_path_at_most_once() {
        reset_builtin_load_counts();
        let mut interp = Interpreter::default();
        interp.load_module("site").expect("startup import site");
        assert_eq!(
            builtin_load_count("os"),
            1,
            "os must construct at most once at startup"
        );
        assert_eq!(
            builtin_load_count("os.path"),
            1,
            "os.path must construct at most once at startup"
        );
    }

    #[test]
    fn eager_cpython_submodules_construct_exactly_once() {
        reset_builtin_load_counts();
        let mut interp = Interpreter::default();
        interp.load_module("os").expect("import os");
        interp.load_module("os.path").expect("import os.path");
        assert_eq!(builtin_load_count("os"), 1);
        assert_eq!(builtin_load_count("os.path"), 1);

        interp.load_module("encodings").expect("import encodings");
        interp
            .load_module("encodings.aliases")
            .expect("import encodings.aliases");
        assert_eq!(builtin_load_count("encodings"), 1);
        assert_eq!(builtin_load_count("encodings.aliases"), 1);

        interp.load_module("pyexpat").expect("import pyexpat");
        interp
            .load_module("pyexpat.errors")
            .expect("import pyexpat.errors");
        interp
            .load_module("pyexpat.model")
            .expect("import pyexpat.model");
        assert_eq!(builtin_load_count("pyexpat"), 1);
        assert_eq!(builtin_load_count("pyexpat.errors"), 1);
        assert_eq!(builtin_load_count("pyexpat.model"), 1);
    }
}
