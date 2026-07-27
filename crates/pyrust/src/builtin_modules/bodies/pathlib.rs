// `pathlib` module — `pathlib.Path` and `pathlib.PosixPath` classes.
//
// Keep the macro-generated registration surface in this owner. Private Rust
// modules implement four independent responsibilities:
//
// - `class_registry`: generation-aware `Path`/`PosixPath` identity and factory;
// - `lexical`: POSIX parsing, normalization, components, and derived paths;
// - `filesystem`: host I/O, directory traversal, globbing, cwd, and home;
// - `presentation`: repr/str, equality, hashing, and `__fspath__`.
//
// Delegation passes the macro's borrowed argument slice straight through, so
// the split adds no per-call collection or allocation.
//
// Reference: <https://docs.python.org/3/library/pathlib.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "pathlib/class_registry.rs"]
mod class_registry;
#[path = "pathlib/filesystem.rs"]
mod filesystem;
#[path = "pathlib/lexical.rs"]
mod lexical;
#[path = "pathlib/presentation.rs"]
mod presentation;

// All Python-visible functions remain declared here. `PosixPath` inherits the
// registered `Path.*` callables from the exact `Path` class exported in the
// same module generation.
pyrust_module! {
    constants {
        "Path" => class_registry::new_path_class_value(),
        "PosixPath" => class_registry::current_posix_path_class_value(),
    }

    /// Allocate a concrete path for the requested `Path` generation/class.
    #[py_name = "Path.__new__"]
    fn path_new(args) -> Result<Value> {
        class_registry::path_new(args)
    }

    /// Initialize a path from lexical string components.
    #[py_name = "Path.__init__"]
    fn path_init(args) -> Result<Value> {
        lexical::path_init(args, FN_NAME)
    }

    /// Return the path string.
    #[py_name = "Path.__str__"]
    fn path_str(args) -> Result<Value> {
        presentation::path_str(args, FN_NAME)
    }

    /// Return a class-aware `PosixPath('...')`-style representation.
    #[py_name = "Path.__repr__"]
    fn path_repr(args) -> Result<Value> {
        presentation::path_repr(args, FN_NAME)
    }

    /// Join one component with `/`, preserving the receiver subclass.
    #[py_name = "Path.__truediv__"]
    fn path_truediv(args) -> Result<Value> {
        lexical::path_truediv(args, FN_NAME)
    }

    /// Compare only against instances of a canonical `Path` generation.
    #[py_name = "Path.__eq__"]
    fn path_eq(args) -> Result<Value> {
        presentation::path_eq(args, FN_NAME)
    }

    /// Hash the lexical path string.
    #[py_name = "Path.__hash__"]
    fn path_hash(args) -> Result<Value> {
        presentation::path_hash(args, FN_NAME)
    }

    /// Implement the `os.fspath` protocol.
    #[py_name = "Path.__fspath__"]
    fn path_fspath(args) -> Result<Value> {
        presentation::path_fspath(args, FN_NAME)
    }

    /// Join zero or more lexical path components.
    #[py_name = "Path.joinpath"]
    fn path_joinpath(args) -> Result<Value> {
        lexical::path_joinpath(args, FN_NAME)
    }

    /// Return the final path component.
    #[py_name = "Path.name"]
    fn path_name(args) -> Result<Value> {
        lexical::path_name(args, FN_NAME)
    }

    /// Return the logical parent path.
    #[py_name = "Path.parent"]
    fn path_parent(args) -> Result<Value> {
        lexical::path_parent(args, FN_NAME)
    }

    /// Return the final component without its suffix.
    #[py_name = "Path.stem"]
    fn path_stem(args) -> Result<Value> {
        lexical::path_stem(args, FN_NAME)
    }

    /// Return the final suffix, including its leading dot.
    #[py_name = "Path.suffix"]
    fn path_suffix(args) -> Result<Value> {
        lexical::path_suffix(args, FN_NAME)
    }

    /// Return the lexical path components.
    #[py_name = "Path.parts"]
    fn path_parts(args) -> Result<Value> {
        lexical::path_parts(args, FN_NAME)
    }

    /// Return whether the path exists.
    #[py_name = "Path.exists"]
    fn path_exists(args) -> Result<Value> {
        filesystem::path_exists(args, FN_NAME)
    }

    /// Return whether the path is a regular file.
    #[py_name = "Path.is_file"]
    fn path_is_file(args) -> Result<Value> {
        filesystem::path_is_file(args, FN_NAME)
    }

    /// Return whether the path is a directory.
    #[py_name = "Path.is_dir"]
    fn path_is_dir(args) -> Result<Value> {
        filesystem::path_is_dir(args, FN_NAME)
    }

    /// Read UTF-8 text from the path.
    #[py_name = "Path.read_text"]
    fn path_read_text(args) -> Result<Value> {
        filesystem::path_read_text(args, FN_NAME)
    }

    /// Write UTF-8 text to the path.
    #[py_name = "Path.write_text"]
    fn path_write_text(args) -> Result<Value> {
        filesystem::path_write_text(args, FN_NAME)
    }

    /// Create the path as a directory.
    #[py_name = "Path.mkdir"]
    fn path_mkdir(args) -> Result<Value> {
        filesystem::path_mkdir(args, FN_NAME)
    }

    /// Return the current directory through the class receiver.
    #[py_name = "Path.cwd"]
    fn path_cwd(args) -> Result<Value> {
        filesystem::path_cwd(_interp, args)
    }

    /// Return the home directory through the class receiver.
    #[py_name = "Path.home"]
    fn path_home(args) -> Result<Value> {
        filesystem::path_home(_interp, args)
    }

    /// Return whether the lexical path is absolute.
    #[py_name = "Path.is_absolute"]
    fn path_is_absolute(args) -> Result<Value> {
        lexical::path_is_absolute(args, FN_NAME)
    }

    /// Resolve the path against the host filesystem.
    #[py_name = "Path.resolve"]
    fn path_resolve(args) -> Result<Value> {
        filesystem::path_resolve(args, FN_NAME)
    }

    /// Read raw bytes from the path.
    #[py_name = "Path.read_bytes"]
    fn path_read_bytes(args) -> Result<Value> {
        filesystem::path_read_bytes(args, FN_NAME)
    }

    /// Write bytes-like data to the path.
    #[py_name = "Path.write_bytes"]
    fn path_write_bytes(args) -> Result<Value> {
        filesystem::path_write_bytes(args, FN_NAME)
    }

    /// Open the path as a file object.
    #[py_name = "Path.open"]
    fn path_open(args) -> Result<Value> {
        filesystem::path_open(args, FN_NAME)
    }

    /// Remove the file.
    #[py_name = "Path.unlink"]
    fn path_unlink(args) -> Result<Value> {
        filesystem::path_unlink(args, FN_NAME)
    }

    /// Yield directory children as receiver-class instances.
    #[py_name = "Path.iterdir"]
    fn path_iterdir(args) -> Result<Value> {
        filesystem::path_iterdir(args, FN_NAME)
    }

    /// Yield paths matching a relative glob pattern.
    #[py_name = "Path.glob"]
    fn path_glob(args) -> Result<Value> {
        filesystem::path_glob(args, FN_NAME)
    }

    /// Replace the final path component.
    #[py_name = "Path.with_name"]
    fn path_with_name(args) -> Result<Value> {
        lexical::path_with_name(args, FN_NAME)
    }

    /// Replace the final component's stem.
    #[py_name = "Path.with_stem"]
    fn path_with_stem(args) -> Result<Value> {
        lexical::path_with_stem(args, FN_NAME)
    }

    /// Replace or remove the final component's suffix.
    #[py_name = "Path.with_suffix"]
    fn path_with_suffix(args) -> Result<Value> {
        lexical::path_with_suffix(args, FN_NAME)
    }
}

#[cfg(test)]
mod boundary_tests {
    const OWNER: &str = include_str!("pathlib.rs");
    const CLASS_REGISTRY: &str = include_str!("pathlib/class_registry.rs");
    const FILESYSTEM: &str = include_str!("pathlib/filesystem.rs");
    const LEXICAL: &str = include_str!("pathlib/lexical.rs");
    const PRESENTATION: &str = include_str!("pathlib/presentation.rs");
    const IMPLEMENTATIONS: &str = concat!(
        include_str!("pathlib/class_registry.rs"),
        include_str!("pathlib/filesystem.rs"),
        include_str!("pathlib/lexical.rs"),
        include_str!("pathlib/presentation.rs"),
    );

    #[test]
    fn owner_keeps_the_only_registration_surface() {
        let registration_macro = concat!("pyrust_", "module!");
        assert_eq!(
            OWNER.matches(registration_macro).count(),
            1,
            "pathlib.rs must remain the only registration owner"
        );
        assert!(
            !IMPLEMENTATIONS.contains(registration_macro),
            "private implementation modules must not create a second registry"
        );
        assert!(
            !IMPLEMENTATIONS
                .lines()
                .any(|line| line.trim_start().starts_with(concat!("#[py_", "name"))),
            "Python-visible names belong to the owner registration facade"
        );
    }

    #[test]
    fn implementation_modules_declare_parent_dependencies_explicitly() {
        let wildcard_parent_import = concat!("use super::", "*");
        assert!(
            !IMPLEMENTATIONS.contains(wildcard_parent_import),
            "pathlib implementation contains a wildcard parent import"
        );
    }

    #[test]
    fn responsibilities_stay_on_their_side_of_the_boundary() {
        assert!(CLASS_REGISTRY.contains("PATH_CLASS_GENERATIONS"));
        assert!(LEXICAL.contains("fn normalize_path"));
        assert!(FILESYSTEM.contains("std::fs::"));
        assert!(PRESENTATION.contains("fn fnv1a_hash"));

        for (name, source) in [
            ("class_registry", CLASS_REGISTRY),
            ("lexical", LEXICAL),
            ("presentation", PRESENTATION),
        ] {
            assert!(
                !source.contains("std::fs::"),
                "{name} must not grow host-filesystem responsibilities"
            );
        }
    }

    #[test]
    fn registration_facade_stays_small_and_complete() {
        let path_registration = concat!("#[py_", "name = \"Path.");
        assert!(
            OWNER.lines().count() <= 360,
            "pathlib registration facade grew beyond its 360-line budget"
        );
        assert_eq!(
            OWNER.matches(path_registration).count(),
            33,
            "every Path registration must remain explicit in pathlib.rs"
        );
    }
}
