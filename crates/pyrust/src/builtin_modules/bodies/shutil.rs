// `shutil` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/shutil.py` (issues #3148, #3251, #3486).
//
// The module mirrors the CPython source: the exception classes, `__all__`,
// the module constants, every public function and the private helpers other
// code reaches for (`_samefile`, `_copytree`, `_rmtree_unsafe`,
// `_make_tarball`, `_ARCHIVE_FORMATS`, ...).  Like the source, the functions
// resolve their dependencies (`os`, `sys`, `stat`, `fnmatch`, ...) and each
// other (`Error`, `copy2`, `COPY_BUFSIZE`) as module globals at call time,
// and every file-system step goes through the `os` / `os.path` function the
// source calls, so patching `shutil.os` or `os.rename` behaves the same way.
//
// - `shutil/support.rs`: module state (`gil_local!`), lookups, call helpers
//   and the `try` / `except` emulation.
// - `shutil/copy.rs`: `copyfileobj` ... `copy2`, `ignore_patterns`,
//   `copytree`.
// - `shutil/tree.rs`: `rmtree` (fd-based on Unix) and `move`.
// - `shutil/archives.rs`: the archive / unpack registries, `make_archive`,
//   `unpack_archive` and the built-in archivers.
// - `shutil/system.rs`: `disk_usage`, `chown`, `get_terminal_size`, `which`.
// - `shutil/docs.rs`: the source's docstrings.
//
// Reference: <https://docs.python.org/3.12/library/shutil.html>

use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

#[path = "shutil/archives.rs"]
mod archives;
#[path = "shutil/copy.rs"]
mod copy;
#[path = "shutil/docs.rs"]
mod docs;
#[path = "shutil/support.rs"]
mod support;
#[path = "shutil/system.rs"]
mod system;
#[path = "shutil/tree.rs"]
mod tree;

fn all_list() -> Value {
    Value::list(
        [
            "copyfileobj",
            "copyfile",
            "copymode",
            "copystat",
            "copy",
            "copy2",
            "copytree",
            "move",
            "rmtree",
            "Error",
            "SpecialFileError",
            "ExecError",
            "make_archive",
            "get_archive_formats",
            "register_archive_format",
            "unregister_archive_format",
            "get_unpack_formats",
            "register_unpack_format",
            "unregister_unpack_format",
            "unpack_archive",
            "ignore_patterns",
            "chown",
            "which",
            "get_terminal_size",
            "SameFileError",
            // disk_usage is added later, if available on the platform
            "disk_usage",
        ]
        .into_iter()
        .map(Value::string)
        .collect(),
    )
}

/// `__doc__` of the `shutil` callable `registry_key` (`shutil.copy`, ...).
pub(crate) fn native_doc(registry_key: &str) -> Option<Option<&'static str>> {
    docs::function_doc(registry_key.strip_prefix("shutil.")?)
}

/// The function attributes the source sets: `rmtree.avoids_symlink_attacks`
/// and `_make_tarball.supports_root_dir` / `_make_zipfile.supports_root_dir`.
pub(crate) fn native_attr(registry_key: &str, name: &str) -> Option<Value> {
    match (registry_key, name) {
        // Allow introspection of whether or not the hardening against
        // symlink attacks is supported on the current platform
        ("shutil.rmtree", "avoids_symlink_attacks") => Some(Value::bool_(cfg!(unix))),
        ("shutil._make_tarball" | "shutil._make_zipfile", "supports_root_dir") => {
            Some(Value::bool_(true))
        }
        _ => None,
    }
}

/// The module namespace in the source's definition order.
const NAMESPACE_ORDER: &[&str] = &[
    "__doc__",
    "os",
    "sys",
    "stat",
    "fnmatch",
    "errno",
    "warnings",
    "_ZLIB_SUPPORTED",
    "_BZ2_SUPPORTED",
    "_LZMA_SUPPORTED",
    "_WINDOWS",
    "posix",
    "nt",
    "_winapi",
    "COPY_BUFSIZE",
    "_USE_CP_SENDFILE",
    "_HAS_FCOPYFILE",
    "_WIN_DEFAULT_PATHEXT",
    "__all__",
    "Error",
    "SameFileError",
    "SpecialFileError",
    "ExecError",
    "ReadError",
    "RegistryError",
    "_GiveupOnFastCopy",
    "copyfileobj",
    "_samefile",
    "_stat",
    "_islink",
    "copyfile",
    "copymode",
    "_copyxattr",
    "copystat",
    "copy",
    "copy2",
    "ignore_patterns",
    "_copytree",
    "copytree",
    "_rmtree_islink",
    "_rmtree_unsafe",
    "_use_fd_functions",
    "rmtree",
    "_basename",
    "move",
    "_destinsrc",
    "_is_immutable",
    "_get_gid",
    "_get_uid",
    "_make_tarball",
    "_make_zipfile",
    "_ARCHIVE_FORMATS",
    "get_archive_formats",
    "register_archive_format",
    "unregister_archive_format",
    "make_archive",
    "get_unpack_formats",
    "_check_unpack_options",
    "register_unpack_format",
    "unregister_unpack_format",
    "_ensure_directory",
    "_unpack_zipfile",
    "_unpack_tarfile",
    "_UNPACK_FORMATS",
    "_find_unpack_format",
    "unpack_archive",
    "disk_usage",
    "chown",
    "get_terminal_size",
    "_access_check",
    "_win_path_needs_curdir",
    "which",
];

/// Wire what the source gets from its module-level imports and statements:
/// the dependency modules, the platform constants, the exception docstrings,
/// the format registries, and the hidden classes standing in for the
/// source's closures; then record the module state for the native functions.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    let module_rc = Rc::clone(module_rc);
    for name in ["os", "sys", "stat", "fnmatch", "errno", "warnings"] {
        if let Ok(dep) = interp.load_module(name) {
            module_rc.borrow_mut().insert_attr(name.to_string(), dep);
        }
    }
    let os = module_rc.borrow().get_attr_value("os");
    let os_has = |interp: &mut Interpreter, name: &str| {
        os.as_ref()
            .is_some_and(|os| interp.get_attr(os, name).is_ok())
    };
    let use_cp_sendfile = cfg!(target_os = "linux") && os_has(interp, "sendfile");

    // `posix = nt = None`, then the platform's module; `_winapi` is not
    // provided by the runtime.
    let posix = if cfg!(unix) {
        interp
            .load_module("posix")
            .unwrap_or_else(|_| Value::none())
    } else {
        Value::none()
    };
    let nt = if cfg!(windows) {
        interp.load_module("nt").unwrap_or_else(|_| Value::none())
    } else {
        Value::none()
    };
    // `posix and hasattr(posix, "_fcopyfile")`
    let has_fcopyfile = if posix.is_none() {
        Value::none()
    } else {
        Value::bool_(interp.get_attr(&posix, "_fcopyfile").is_ok())
    };

    let function = |name: &str| {
        module_rc
            .borrow()
            .get_attr_value(name)
            .unwrap_or_else(Value::none)
    };
    let (archive_formats, unpack_formats) = archives::initial_registries(
        function("_make_tarball"),
        function("_make_zipfile"),
        function("_unpack_tarfile"),
        function("_unpack_zipfile"),
    );

    let hidden_class = |name: &str| {
        let removed = module_rc.borrow_mut().attrs.shift_remove(name);
        match removed.as_ref().map(Value::kind) {
            Some(ValueKind::PyClass(class)) => Some(Rc::clone(class)),
            _ => None,
        }
    };
    let ignore_class = hidden_class("_IgnorePatterns");
    let owner_filter_class = hidden_class("_SetUidGid");

    {
        let mut module = module_rc.borrow_mut();
        for (name, value) in [
            ("posix", posix),
            ("nt", nt),
            ("_winapi", Value::none()),
            ("_USE_CP_SENDFILE", Value::bool_(use_cp_sendfile)),
            ("_HAS_FCOPYFILE", has_fcopyfile),
            ("_ARCHIVE_FORMATS", archive_formats.clone()),
            ("_UNPACK_FORMATS", unpack_formats.clone()),
        ] {
            module.insert_attr(name.to_string(), value);
        }
    }

    for (name, error_name, doc) in [
        ("Error", "shutil.Error", None),
        (
            "SameFileError",
            "shutil.SameFileError",
            Some(docs::DOC_CLASS_SAMEFILEERROR),
        ),
        (
            "SpecialFileError",
            "shutil.SpecialFileError",
            Some(docs::DOC_CLASS_SPECIALFILEERROR),
        ),
        (
            "ExecError",
            "shutil.ExecError",
            Some(docs::DOC_CLASS_EXECERROR),
        ),
        (
            "ReadError",
            "shutil.ReadError",
            Some(docs::DOC_CLASS_READERROR),
        ),
        (
            "RegistryError",
            "shutil.RegistryError",
            Some(docs::DOC_CLASS_REGISTRYERROR),
        ),
        (
            "_GiveupOnFastCopy",
            "shutil._GiveupOnFastCopy",
            Some(docs::DOC_CLASS_GIVEUPONFASTCOPY),
        ),
    ] {
        let class = module_rc.borrow().get_attr_value(name);
        if let Some(ValueKind::PyClass(class)) = class.as_ref().map(Value::kind) {
            let mut class = class.borrow_mut();
            class.error_name = Some(error_name);
            class.attrs.insert(
                "__doc__".to_string(),
                doc.map_or_else(Value::none, Value::string),
            );
        }
    }

    support::install_state(support::State {
        module: Rc::clone(&module_rc),
        archive_formats,
        unpack_formats,
        ignore_class,
        owner_filter_class,
        ntuple_diskusage: None,
    });

    let mut module = module_rc.borrow_mut();
    for name in NAMESPACE_ORDER {
        if let Some(value) = module.attrs.shift_remove(*name) {
            module.attrs.insert((*name).to_string(), value);
        }
    }
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "_ZLIB_SUPPORTED" => Value::bool_(true),
        "_BZ2_SUPPORTED" => Value::bool_(true),
        "_LZMA_SUPPORTED" => Value::bool_(true),
        "_WINDOWS" => Value::bool_(cfg!(windows)),
        "COPY_BUFSIZE" => Value::int(copy::COPY_BUFSIZE),
        "_WIN_DEFAULT_PATHEXT" => Value::string(system::WIN_DEFAULT_PATHEXT),
        "__all__" => all_list(),
        "_use_fd_functions" => Value::bool_(cfg!(unix)),
    }

    exception Error(OSError);
    exception SameFileError(Error);
    exception SpecialFileError(OSError);
    exception ExecError(OSError);
    exception ReadError(OSError);
    exception RegistryError;
    exception _GiveupOnFastCopy;

    fn copyfileobj(args) -> Result<Value> {
        copy::copyfileobj_entry(_interp, args)
    }

    fn _samefile(args) -> Result<Value> {
        copy::samefile_entry(_interp, args)
    }

    fn _stat(args) -> Result<Value> {
        copy::stat_entry(_interp, args)
    }

    fn _islink(args) -> Result<Value> {
        copy::islink_entry(_interp, args)
    }

    fn copyfile(args) -> Result<Value> {
        copy::copyfile_entry(_interp, args)
    }

    fn copymode(args) -> Result<Value> {
        copy::copymode_entry(_interp, args)
    }

    fn _copyxattr(args) -> Result<Value> {
        copy::copyxattr_entry(_interp, args)
    }

    fn copystat(args) -> Result<Value> {
        copy::copystat_entry(_interp, args)
    }

    fn copy(args) -> Result<Value> {
        copy::copy_entry(_interp, args)
    }

    fn copy2(args) -> Result<Value> {
        copy::copy2_entry(_interp, args)
    }

    fn ignore_patterns(args) -> Result<Value> {
        copy::ignore_patterns_entry(_interp, args)
    }

    fn _copytree(args) -> Result<Value> {
        copy::copytree_entries_entry(_interp, args)
    }

    fn copytree(args) -> Result<Value> {
        copy::copytree_entry(_interp, args)
    }

    fn _rmtree_islink(args) -> Result<Value> {
        tree::rmtree_islink_entry(_interp, args)
    }

    fn _rmtree_unsafe(args) -> Result<Value> {
        tree::rmtree_unsafe_entry(_interp, args)
    }

    fn rmtree(args) -> Result<Value> {
        tree::rmtree_entry(_interp, args)
    }

    fn _basename(args) -> Result<Value> {
        tree::basename_entry(_interp, args)
    }

    #[py_name = "move"]
    fn move_(args) -> Result<Value> {
        tree::move_entry(_interp, args)
    }

    fn _destinsrc(args) -> Result<Value> {
        tree::destinsrc_entry(_interp, args)
    }

    fn _is_immutable(args) -> Result<Value> {
        tree::is_immutable_entry(_interp, args)
    }

    fn _get_gid(args) -> Result<Value> {
        system::get_gid_entry(_interp, args)
    }

    fn _get_uid(args) -> Result<Value> {
        system::get_uid_entry(_interp, args)
    }

    fn _make_tarball(args) -> Result<Value> {
        archives::make_tarball_entry(_interp, args)
    }

    fn _make_zipfile(args) -> Result<Value> {
        archives::make_zipfile_entry(_interp, args)
    }

    fn get_archive_formats(args) -> Result<Value> {
        archives::get_archive_formats_entry(_interp, args)
    }

    fn register_archive_format(args) -> Result<Value> {
        archives::register_archive_format_entry(_interp, args)
    }

    fn unregister_archive_format(args) -> Result<Value> {
        archives::unregister_archive_format_entry(_interp, args)
    }

    fn make_archive(args) -> Result<Value> {
        archives::make_archive_entry(_interp, args)
    }

    fn get_unpack_formats(args) -> Result<Value> {
        archives::get_unpack_formats_entry(_interp, args)
    }

    fn _check_unpack_options(args) -> Result<Value> {
        archives::check_unpack_options_entry(_interp, args)
    }

    fn register_unpack_format(args) -> Result<Value> {
        archives::register_unpack_format_entry(_interp, args)
    }

    fn unregister_unpack_format(args) -> Result<Value> {
        archives::unregister_unpack_format_entry(_interp, args)
    }

    fn _ensure_directory(args) -> Result<Value> {
        archives::ensure_directory_entry(_interp, args)
    }

    fn _unpack_zipfile(args) -> Result<Value> {
        archives::unpack_zipfile_entry(_interp, args)
    }

    fn _unpack_tarfile(args) -> Result<Value> {
        archives::unpack_tarfile_entry(_interp, args)
    }

    fn _find_unpack_format(args) -> Result<Value> {
        archives::find_unpack_format_entry(_interp, args)
    }

    fn unpack_archive(args) -> Result<Value> {
        archives::unpack_archive_entry(_interp, args)
    }

    fn disk_usage(args) -> Result<Value> {
        system::disk_usage_entry(_interp, args)
    }

    fn chown(args) -> Result<Value> {
        system::chown_entry(_interp, args)
    }

    fn get_terminal_size(args) -> Result<Value> {
        system::get_terminal_size_entry(_interp, args)
    }

    fn _access_check(args) -> Result<Value> {
        system::access_check_entry(_interp, args)
    }

    fn _win_path_needs_curdir(args) -> Result<Value> {
        system::win_path_needs_curdir_entry(_interp, args)
    }

    fn which(args) -> Result<Value> {
        system::which_entry(_interp, args)
    }

    /// The callable `ignore_patterns` returns (the source's
    /// `_ignore_patterns` closure); detached from the module namespace in
    /// `prepare_module`.
    class _IgnorePatterns {
        fn __call__(args) -> Result<Value> {
            copy::ignore_patterns_call(_interp, args)
        }

        fn __repr__(args) -> Result<Value> {
            let _ = _interp;
            copy::ignore_patterns_repr(args)
        }
    }

    /// The `_set_uid_gid` tar filter `_make_tarball` builds when an owner or
    /// group is requested; detached from the module namespace in
    /// `prepare_module`.
    class _SetUidGid {
        fn __call__(args) -> Result<Value> {
            archives::owner_filter_call(_interp, args)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::interpreter::Interpreter;

    /// Run `src` in a fresh interpreter on its own thread (its own GIL
    /// context, so its own `shutil` state).
    fn run_on_fresh_thread(src: &'static str) -> std::result::Result<(), String> {
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let mut interp = Interpreter::default();
                interp
                    .exec_source(src, None, None)
                    .map(|_| ())
                    .map_err(|err| format!("{err:?}"))
            })
            .expect("spawn interpreter thread")
            .join()
            .expect("interpreter thread must not panic")
    }

    #[test]
    fn copies_moves_and_removes_trees() {
        let src = r#"
import os, shutil, tempfile

with tempfile.TemporaryDirectory() as tmp:
    src = os.path.join(tmp, "src")
    os.makedirs(os.path.join(src, "sub"))
    for name in ("a.txt", os.path.join("sub", "b.txt"), "skip.tmp"):
        with open(os.path.join(src, name), "w") as f:
            f.write(name)
    dst = shutil.copytree(src, os.path.join(tmp, "dst"), ignore=shutil.ignore_patterns("*.tmp"))
    assert sorted(os.listdir(dst)) == ["a.txt", "sub"], os.listdir(dst)
    moved = shutil.move(dst, os.path.join(tmp, "moved"))
    assert os.path.isfile(os.path.join(moved, "sub", "b.txt"))
    calls = []
    shutil.rmtree(os.path.join(tmp, "missing"), onexc=lambda *a: calls.append(a[0].__name__))
    assert calls, calls
    shutil.rmtree(moved)
    assert not os.path.exists(moved)
    try:
        shutil.copyfile(os.path.join(src, "a.txt"), os.path.join(src, "a.txt"))
    except shutil.SameFileError:
        pass
    else:
        raise AssertionError("expected SameFileError")
"#;
        run_on_fresh_thread(src).expect("copytree / move / rmtree should work");
    }

    #[test]
    fn format_registries_are_per_interpreter() {
        // Registering formats in one interpreter must not leak into another.
        let registering = r#"
import shutil
shutil.register_archive_format("only_here", lambda *a, **k: None, description="local")
shutil.register_unpack_format("only_here", [".onlyhere"], lambda *a, **k: None)
assert "only_here" in dict(shutil.get_archive_formats())
assert shutil._find_unpack_format("x.onlyhere") == "only_here"
shutil.unregister_archive_format("zip")
assert "zip" not in shutil._ARCHIVE_FORMATS
"#;
        run_on_fresh_thread(registering).expect("registration should succeed");
        let fresh = r#"
import shutil
names = [name for name, _ in shutil.get_archive_formats()]
assert names == ["bztar", "gztar", "tar", "xztar", "zip"], names
assert shutil._find_unpack_format("x.onlyhere") is None
assert shutil._ARCHIVE_FORMATS["zip"][0] is shutil._make_zipfile
"#;
        run_on_fresh_thread(fresh).expect("a fresh interpreter sees only the built-in formats");
    }
}
