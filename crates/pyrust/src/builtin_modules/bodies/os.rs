// `os` registration facade.
//
// Python-visible names stay in this owner while private Rust modules implement
// independent responsibilities:
//
// - `arguments`: call-shape and primitive argument validation;
// - `environment`: live process environment and `_Environ`;
// - `filesystem`: cwd, filesystem mutation, metadata, and traversal;
// - `path_protocol`: interpreter-level `__fspath__` dispatch;
// - `system`: process ids, host capabilities, entropy, and terminal queries;
// - `result_types`: `terminal_size`/`stat_result` identity and presentation.
//
// Every adapter forwards the macro's borrowed argument slice directly. The
// split therefore adds neither a collection nor an allocation to call paths.
//
// Reference: <https://docs.python.org/3/library/os.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "os/arguments.rs"]
mod arguments;
#[path = "os/environment.rs"]
mod environment;
#[path = "os/filesystem.rs"]
mod filesystem;
#[path = "os/path_protocol.rs"]
mod path_protocol;
#[path = "os/result_types.rs"]
mod result_types;
#[path = "os/system.rs"]
mod system;

/// Registry names for the private `_Environ` class.
///
/// They live next to their `#[py_name]` declarations so class construction
/// never has to derive dispatch identity from a Python-visible dotted string.
const ENVIRON_METHODS: &[(&str, &str)] = &[
    ("__getitem__", "os._Environ.__getitem__"),
    ("__setitem__", "os._Environ.__setitem__"),
    ("__delitem__", "os._Environ.__delitem__"),
    ("__contains__", "os._Environ.__contains__"),
    ("__iter__", "os._Environ.__iter__"),
    ("__len__", "os._Environ.__len__"),
    ("__repr__", "os._Environ.__repr__"),
    ("get", "os._Environ.get"),
    ("keys", "os._Environ.keys"),
    ("values", "os._Environ.values"),
    ("items", "os._Environ.items"),
];

/// Internal registry identity used by the `terminal_size` result class.
pub(super) const TERMINAL_SIZE_NEW_REGISTRY: &str = "os.terminal_size_new";
pub(super) const TERMINAL_SIZE_REPR_REGISTRY: &str = "os.terminal_size_repr";
pub(super) const STAT_RESULT_NEW_REGISTRY: &str = "os.stat_result_new";
pub(super) const STAT_RESULT_REPR_REGISTRY: &str = "os.stat_result_repr";

pyrust_module! {
    constants {
        "sep" => Value::string(std::path::MAIN_SEPARATOR.to_string()),
        "path" => super::os_path::module(),
        "environ" => environment::make_environ_instance(ENVIRON_METHODS),
        "name" => Value::string(if cfg!(windows) { "nt" } else { "posix" }),
        "linesep" => Value::string(if cfg!(windows) { "\r\n" } else { "\n" }),
        "curdir" => Value::string("."),
        "pardir" => Value::string(".."),
        "extsep" => Value::string("."),
        "pathsep" => Value::string(if cfg!(windows) { ";" } else { ":" }),
        "altsep" => {
            if cfg!(windows) {
                Value::string("/")
            } else {
                Value::none()
            }
        },
        "devnull" => Value::string(if cfg!(windows) { "nul" } else { "/dev/null" }),
        // Struct-sequence classes are process-canonical, just like CPython's
        // `posix.stat_result` / `os.terminal_size` exports.  Host factories and
        // direct Python construction share these exact class objects.
        "terminal_size" => result_types::terminal_size_class_value(),
        "stat_result" => result_types::stat_result_class_value(),
    }

    /// Return the current working directory.
    fn getcwd(args) -> Result<Value> {
        filesystem::getcwd(args, FN_NAME)
    }

    /// Change the current working directory.
    fn chdir(args) -> Result<Value> {
        filesystem::chdir(args, FN_NAME)
    }

    /// Read a live process-environment value.
    fn getenv(args) -> Result<Value> {
        environment::getenv(args, FN_NAME)
    }

    /// List names in a directory.
    fn listdir(args) -> Result<Value> {
        filesystem::listdir(args, FN_NAME)
    }

    /// Create one directory.
    fn mkdir(args) -> Result<Value> {
        filesystem::mkdir(args, FN_NAME)
    }

    /// Recursively create directories.
    fn makedirs(args) -> Result<Value> {
        filesystem::makedirs(args, FN_NAME)
    }

    /// Delete a file.
    fn remove(args) -> Result<Value> {
        filesystem::remove(args, FN_NAME)
    }

    /// Delete a file through the `unlink` alias.
    fn unlink(args) -> Result<Value> {
        filesystem::unlink(args, FN_NAME)
    }

    /// Remove an empty directory.
    fn rmdir(args) -> Result<Value> {
        filesystem::rmdir(args, FN_NAME)
    }

    /// Rename or move a filesystem entry.
    fn rename(args) -> Result<Value> {
        filesystem::rename(args, FN_NAME)
    }

    /// Recursively walk a directory tree.
    fn walk(args) -> Result<Value> {
        filesystem::walk(args, FN_NAME)
    }

    /// Return the current process id.
    fn getpid(args) -> Result<Value> {
        system::getpid(args, FN_NAME)
    }

    /// Return the parent process id.
    fn getppid(args) -> Result<Value> {
        system::getppid(args, FN_NAME)
    }

    /// Return the host's logical CPU count.
    fn cpu_count(args) -> Result<Value> {
        system::cpu_count(args, FN_NAME)
    }

    /// Read bytes from the OS cryptographic random source.
    fn urandom(args) -> Result<Value> {
        system::urandom(args, FN_NAME)
    }

    /// Return the host error message for an error code.
    fn strerror(args) -> Result<Value> {
        system::strerror(args, FN_NAME)
    }

    /// Return a terminal-size result object.
    fn get_terminal_size(args) -> Result<Value> {
        system::get_terminal_size(args, FN_NAME)
    }

    /// Apply the interpreter-level path protocol.
    fn fspath(args) -> Result<Value> {
        path_protocol::fspath(_interp, args, FN_NAME)
    }

    /// Return filesystem metadata as an `os.stat_result`.
    fn stat(args) -> Result<Value> {
        filesystem::stat(args, FN_NAME)
    }

    /// Present an `os.terminal_size` result.
    #[py_name = "terminal_size_repr"]
    fn terminal_size_repr(args) -> Result<Value> {
        result_types::terminal_size_repr(args, FN_NAME)
    }

    #[py_name = "terminal_size_new"]
    fn terminal_size_new(args) -> Result<Value> {
        result_types::terminal_size_new(_interp, args, FN_NAME)
    }

    #[py_name = "stat_result_new"]
    fn stat_result_new(args) -> Result<Value> {
        result_types::stat_result_new(_interp, args, FN_NAME)
    }

    #[py_name = "stat_result_repr"]
    fn stat_result_repr(args) -> Result<Value> {
        result_types::stat_result_repr(args, FN_NAME)
    }

    #[py_name = "_Environ.__getitem__"]
    fn environ_getitem(args) -> Result<Value> {
        environment::environ_getitem(args, FN_NAME)
    }

    #[py_name = "_Environ.__setitem__"]
    fn environ_setitem(args) -> Result<Value> {
        environment::environ_setitem(args, FN_NAME)
    }

    #[py_name = "_Environ.__delitem__"]
    fn environ_delitem(args) -> Result<Value> {
        environment::environ_delitem(args, FN_NAME)
    }

    #[py_name = "_Environ.__contains__"]
    fn environ_contains(args) -> Result<Value> {
        environment::environ_contains(args, FN_NAME)
    }

    #[py_name = "_Environ.__iter__"]
    fn environ_iter(args) -> Result<Value> {
        environment::environ_iter(args, FN_NAME)
    }

    #[py_name = "_Environ.__len__"]
    fn environ_len(args) -> Result<Value> {
        environment::environ_len(args, FN_NAME)
    }

    #[py_name = "_Environ.__repr__"]
    fn environ_repr(args) -> Result<Value> {
        environment::environ_repr(args, FN_NAME)
    }

    #[py_name = "_Environ.get"]
    fn environ_get(args) -> Result<Value> {
        environment::environ_get(args, FN_NAME)
    }

    #[py_name = "_Environ.keys"]
    fn environ_keys(args) -> Result<Value> {
        environment::environ_keys(args, FN_NAME)
    }

    #[py_name = "_Environ.values"]
    fn environ_values(args) -> Result<Value> {
        environment::environ_values(args, FN_NAME)
    }

    #[py_name = "_Environ.items"]
    fn environ_items(args) -> Result<Value> {
        environment::environ_items(args, FN_NAME)
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::ENVIRON_METHODS;

    const OWNER: &str = include_str!("os.rs");
    const ARGUMENTS: &str = include_str!("os/arguments.rs");
    const ENVIRONMENT: &str = include_str!("os/environment.rs");
    const FILESYSTEM: &str = include_str!("os/filesystem.rs");
    const PATH_PROTOCOL: &str = include_str!("os/path_protocol.rs");
    const RESULT_TYPES: &str = include_str!("os/result_types.rs");
    const SYSTEM: &str = include_str!("os/system.rs");
    const IMPLEMENTATIONS: &str = concat!(
        include_str!("os/arguments.rs"),
        include_str!("os/environment.rs"),
        include_str!("os/filesystem.rs"),
        include_str!("os/path_protocol.rs"),
        include_str!("os/result_types.rs"),
        include_str!("os/system.rs"),
    );

    #[test]
    fn owner_keeps_the_only_registration_surface() {
        let registration_macro = concat!("pyrust_", "module!");
        assert_eq!(OWNER.matches(registration_macro).count(), 1);
        assert!(!IMPLEMENTATIONS.contains(registration_macro));
        assert!(
            !IMPLEMENTATIONS
                .lines()
                .any(|line| line.trim_start().starts_with(concat!("#[py_", "name")))
        );
    }

    #[test]
    fn implementation_dependencies_are_explicit_real_modules() {
        assert!(!IMPLEMENTATIONS.contains(concat!("use super::", "*")));
        assert!(!IMPLEMENTATIONS.contains(concat!("include", "!(")));
        assert!(!OWNER.contains(concat!("include", "!(")));
    }

    #[test]
    fn responsibilities_stay_on_their_side_of_the_boundary() {
        assert!(ARGUMENTS.contains("fn require_str"));
        assert!(ENVIRONMENT.contains("static ENV_LOCK"));
        assert!(FILESYSTEM.contains("fn walk_collect"));
        assert!(PATH_PROTOCOL.contains("\"__fspath__\""));
        assert!(RESULT_TYPES.contains("STAT_RESULT_CLASS"));
        assert!(SYSTEM.contains("fn get_parent_pid"));

        assert!(!ARGUMENTS.contains("std::fs::"));
        assert!(!ARGUMENTS.contains("std::env::"));
        assert!(!ENVIRONMENT.contains("std::fs::"));
        assert!(!PATH_PROTOCOL.contains("std::fs::"));
        assert!(!PATH_PROTOCOL.contains("std::env::"));
        assert!(!SYSTEM.contains("walk_collect"));
    }

    #[test]
    fn registration_facade_stays_small_and_complete() {
        let environ_registration = concat!("#[py_", "name = \"_Environ.");
        assert!(
            OWNER.lines().count() <= 340,
            "os registration facade grew beyond its 340-line budget"
        );
        assert_eq!(OWNER.matches(environ_registration).count(), 11);
        assert_eq!(ENVIRON_METHODS.len(), 11);
    }
}
