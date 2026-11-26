// `pathlib` module — `PurePath`, `PurePosixPath`, `PureWindowsPath`,
// `Path`, `PosixPath`, and `WindowsPath` classes.
//
// Keep the macro-generated registration surface in this owner. Private Rust
// modules implement four independent responsibilities:
//
// - `class_registry`: generation-aware 6-class hierarchy identity and factory;
// - `lexical`: POSIX/Windows parsing, normalization, components, match, and parents;
// - `filesystem`: host I/O, stat/lstat and file-kind predicates, permissions,
//   links, ownership, glob/rglob, absolute/expanduser, cwd, home;
// - `presentation`: repr/str, equality/ordering, hashing, and `__fspath__`;
// - `walk`: the lazy `Path.walk()` directory-tree generator.
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
#[path = "pathlib/walk.rs"]
mod walk;

pyrust_module! {
    constants {
        "PurePath" => class_registry::new_generation_pure_path_value(),
        "PurePosixPath" => class_registry::current_pure_posix_value(),
        "PureWindowsPath" => class_registry::current_pure_windows_value(),
        "Path" => class_registry::current_path_value(),
        "PosixPath" => class_registry::current_posix_value(),
        "WindowsPath" => class_registry::current_windows_value(),
    }

    // ── PurePath methods & properties (inherited by all 6 path classes) ──────

    #[py_name = "PurePath.__new__"]
    fn pure_path_new(args) -> Result<Value> {
        class_registry::path_new(args)
    }

    #[py_name = "PurePath.__init__"]
    fn pure_path_init(args) -> Result<Value> {
        lexical::path_init(_interp, args, FN_NAME)
    }

    #[py_name = "PurePath.__str__"]
    fn pure_path_str(args) -> Result<Value> {
        presentation::path_str(args, FN_NAME)
    }

    #[py_name = "PurePath.__repr__"]
    fn pure_path_repr(args) -> Result<Value> {
        presentation::path_repr(args, FN_NAME)
    }

    #[py_name = "PurePath.__truediv__"]
    fn pure_path_truediv(args) -> Result<Value> {
        lexical::path_truediv(_interp, args, FN_NAME)
    }

    #[py_name = "PurePath.__rtruediv__"]
    fn pure_path_rtruediv(args) -> Result<Value> {
        lexical::path_rtruediv(_interp, args, FN_NAME)
    }

    #[py_name = "PurePath.__eq__"]
    fn pure_path_eq(args) -> Result<Value> {
        presentation::path_eq(args, FN_NAME)
    }

    #[py_name = "PurePath.__lt__"]
    fn pure_path_lt(args) -> Result<Value> {
        presentation::path_lt(args, FN_NAME)
    }

    #[py_name = "PurePath.__le__"]
    fn pure_path_le(args) -> Result<Value> {
        presentation::path_le(args, FN_NAME)
    }

    #[py_name = "PurePath.__gt__"]
    fn pure_path_gt(args) -> Result<Value> {
        presentation::path_gt(args, FN_NAME)
    }

    #[py_name = "PurePath.__ge__"]
    fn pure_path_ge(args) -> Result<Value> {
        presentation::path_ge(args, FN_NAME)
    }

    #[py_name = "PurePath.__hash__"]
    fn pure_path_hash(args) -> Result<Value> {
        presentation::path_hash(args, FN_NAME)
    }

    #[py_name = "PurePath.__fspath__"]
    fn pure_path_fspath(args) -> Result<Value> {
        presentation::path_fspath(args, FN_NAME)
    }

    #[py_name = "PurePath.as_posix"]
    fn pure_path_as_posix(args) -> Result<Value> {
        lexical::path_as_posix(args, FN_NAME)
    }

    #[py_name = "PurePath.joinpath"]
    fn pure_path_joinpath(args) -> Result<Value> {
        lexical::path_joinpath(_interp, args, FN_NAME)
    }

    #[py_name = "PurePath.is_absolute"]
    fn pure_path_is_absolute(args) -> Result<Value> {
        lexical::path_is_absolute(args, FN_NAME)
    }

    #[py_name = "PurePath.with_name"]
    fn pure_path_with_name(args) -> Result<Value> {
        lexical::path_with_name(args, FN_NAME)
    }

    #[py_name = "PurePath.with_stem"]
    fn pure_path_with_stem(args) -> Result<Value> {
        lexical::path_with_stem(args, FN_NAME)
    }

    #[py_name = "PurePath.with_suffix"]
    fn pure_path_with_suffix(args) -> Result<Value> {
        lexical::path_with_suffix(args, FN_NAME)
    }

    #[py_name = "PurePath.relative_to"]
    fn pure_path_relative_to(args) -> Result<Value> {
        lexical::path_relative_to(_interp, args, FN_NAME)
    }

    #[py_name = "PurePath.is_relative_to"]
    fn pure_path_is_relative_to(args) -> Result<Value> {
        lexical::path_is_relative_to(_interp, args, FN_NAME)
    }

    #[py_name = "PurePath.match"]
    fn pure_path_match(args) -> Result<Value> {
        lexical::path_match(_interp, args, FN_NAME)
    }

    #[py_name = "PurePath.name"]
    fn pure_path_name(args) -> Result<Value> {
        lexical::path_name(args, FN_NAME)
    }

    #[py_name = "PurePath.parent"]
    fn pure_path_parent(args) -> Result<Value> {
        lexical::path_parent(args, FN_NAME)
    }

    #[py_name = "PurePath.parents"]
    fn pure_path_parents(args) -> Result<Value> {
        lexical::path_parents(args, FN_NAME)
    }

    #[py_name = "PurePath.stem"]
    fn pure_path_stem(args) -> Result<Value> {
        lexical::path_stem(args, FN_NAME)
    }

    #[py_name = "PurePath.suffix"]
    fn pure_path_suffix(args) -> Result<Value> {
        lexical::path_suffix(args, FN_NAME)
    }

    #[py_name = "PurePath.suffixes"]
    fn pure_path_suffixes(args) -> Result<Value> {
        lexical::path_suffixes(args, FN_NAME)
    }

    #[py_name = "PurePath.parts"]
    fn pure_path_parts(args) -> Result<Value> {
        lexical::path_parts(args, FN_NAME)
    }

    #[py_name = "PurePath.drive"]
    fn pure_path_drive(args) -> Result<Value> {
        lexical::path_drive(args, FN_NAME)
    }

    #[py_name = "PurePath.root"]
    fn pure_path_root(args) -> Result<Value> {
        lexical::path_root(args, FN_NAME)
    }

    #[py_name = "PurePath.anchor"]
    fn pure_path_anchor(args) -> Result<Value> {
        lexical::path_anchor(args, FN_NAME)
    }

    // ── _PathParents sequence methods ────────────────────────────────────────

    #[py_name = "_PathParents.__len__"]
    fn path_parents_len(args) -> Result<Value> {
        lexical::parents_len(args, FN_NAME)
    }

    #[py_name = "_PathParents.__getitem__"]
    fn path_parents_getitem(args) -> Result<Value> {
        lexical::parents_getitem(args, FN_NAME)
    }

    #[py_name = "_PathParents.__iter__"]
    fn path_parents_iter(args) -> Result<Value> {
        lexical::parents_iter(args, FN_NAME)
    }

    #[py_name = "_PathParents.__contains__"]
    fn path_parents_contains(args) -> Result<Value> {
        lexical::parents_contains(_interp, args, FN_NAME)
    }

    #[py_name = "_PathParents.__repr__"]
    fn path_parents_repr(args) -> Result<Value> {
        lexical::parents_repr(args, FN_NAME)
    }

    // ── Path concrete filesystem methods ─────────────────────────────────────

    #[py_name = "Path.exists"]
    fn path_exists(args) -> Result<Value> {
        filesystem::path_exists(args, FN_NAME)
    }

    #[py_name = "Path.is_file"]
    fn path_is_file(args) -> Result<Value> {
        filesystem::path_is_file(args, FN_NAME)
    }

    #[py_name = "Path.is_dir"]
    fn path_is_dir(args) -> Result<Value> {
        filesystem::path_is_dir(args, FN_NAME)
    }

    #[py_name = "Path.is_symlink"]
    fn path_is_symlink(args) -> Result<Value> {
        filesystem::path_is_symlink(args, FN_NAME)
    }

    #[py_name = "Path.readlink"]
    fn path_readlink(args) -> Result<Value> {
        filesystem::path_readlink(args, FN_NAME)
    }

    #[py_name = "Path.stat"]
    fn path_stat(args) -> Result<Value> {
        filesystem::path_stat(_interp, args, FN_NAME)
    }

    #[py_name = "Path.lstat"]
    fn path_lstat(args) -> Result<Value> {
        filesystem::path_lstat(args, FN_NAME)
    }

    #[py_name = "Path.samefile"]
    fn path_samefile(args) -> Result<Value> {
        filesystem::path_samefile(_interp, args, FN_NAME)
    }

    #[py_name = "Path.read_text"]
    fn path_read_text(args) -> Result<Value> {
        filesystem::path_read_text(args, FN_NAME)
    }

    #[py_name = "Path.write_text"]
    fn path_write_text(args) -> Result<Value> {
        filesystem::path_write_text(args, FN_NAME)
    }

    #[py_name = "Path.read_bytes"]
    fn path_read_bytes(args) -> Result<Value> {
        filesystem::path_read_bytes(args, FN_NAME)
    }

    #[py_name = "Path.write_bytes"]
    fn path_write_bytes(args) -> Result<Value> {
        filesystem::path_write_bytes(args, FN_NAME)
    }

    #[py_name = "Path.open"]
    fn path_open(args) -> Result<Value> {
        filesystem::path_open(args, FN_NAME)
    }

    #[py_name = "Path.mkdir"]
    fn path_mkdir(args) -> Result<Value> {
        filesystem::path_mkdir(args, FN_NAME)
    }

    #[py_name = "Path.rmdir"]
    fn path_rmdir(args) -> Result<Value> {
        filesystem::path_rmdir(args, FN_NAME)
    }

    #[py_name = "Path.unlink"]
    fn path_unlink(args) -> Result<Value> {
        filesystem::path_unlink(args, FN_NAME)
    }

    #[py_name = "Path.touch"]
    fn path_touch(args) -> Result<Value> {
        filesystem::path_touch(_interp, args, FN_NAME)
    }

    #[py_name = "Path.rename"]
    fn path_rename(args) -> Result<Value> {
        filesystem::path_rename(_interp, args, FN_NAME)
    }

    #[py_name = "Path.replace"]
    fn path_replace(args) -> Result<Value> {
        filesystem::path_replace(_interp, args, FN_NAME)
    }

    #[py_name = "Path.iterdir"]
    fn path_iterdir(args) -> Result<Value> {
        filesystem::path_iterdir(args, FN_NAME)
    }

    #[py_name = "Path.glob"]
    fn path_glob(args) -> Result<Value> {
        filesystem::path_glob(_interp, args, FN_NAME)
    }

    #[py_name = "Path.rglob"]
    fn path_rglob(args) -> Result<Value> {
        filesystem::path_rglob(_interp, args, FN_NAME)
    }

    #[py_name = "Path.resolve"]
    fn path_resolve(args) -> Result<Value> {
        filesystem::path_resolve(args, FN_NAME)
    }

    #[py_name = "Path.absolute"]
    fn path_absolute(args) -> Result<Value> {
        filesystem::path_absolute(args, FN_NAME)
    }

    #[py_name = "Path.expanduser"]
    fn path_expanduser(args) -> Result<Value> {
        filesystem::path_expanduser(args, FN_NAME)
    }

    #[py_name = "Path.chmod"]
    fn path_chmod(args) -> Result<Value> {
        filesystem::path_chmod(_interp, args, FN_NAME)
    }

    #[py_name = "Path.lchmod"]
    fn path_lchmod(args) -> Result<Value> {
        filesystem::path_lchmod(_interp, args, FN_NAME)
    }

    #[py_name = "Path.symlink_to"]
    fn path_symlink_to(args) -> Result<Value> {
        filesystem::path_symlink_to(_interp, args, FN_NAME)
    }

    #[py_name = "Path.hardlink_to"]
    fn path_hardlink_to(args) -> Result<Value> {
        filesystem::path_hardlink_to(_interp, args, FN_NAME)
    }

    #[py_name = "Path.walk"]
    fn path_walk(args) -> Result<Value> {
        walk::path_walk(args, FN_NAME)
    }

    #[py_name = "Path.is_mount"]
    fn path_is_mount(args) -> Result<Value> {
        filesystem::path_is_mount(_interp, args, FN_NAME)
    }

    #[py_name = "Path.is_junction"]
    fn path_is_junction(args) -> Result<Value> {
        filesystem::path_is_junction(_interp, args, FN_NAME)
    }

    #[py_name = "Path.is_block_device"]
    fn path_is_block_device(args) -> Result<Value> {
        filesystem::path_is_special_kind(args, FN_NAME, filesystem::SpecialFileKind::BlockDevice)
    }

    #[py_name = "Path.is_char_device"]
    fn path_is_char_device(args) -> Result<Value> {
        filesystem::path_is_special_kind(args, FN_NAME, filesystem::SpecialFileKind::CharDevice)
    }

    #[py_name = "Path.is_fifo"]
    fn path_is_fifo(args) -> Result<Value> {
        filesystem::path_is_special_kind(args, FN_NAME, filesystem::SpecialFileKind::Fifo)
    }

    #[py_name = "Path.is_socket"]
    fn path_is_socket(args) -> Result<Value> {
        filesystem::path_is_special_kind(args, FN_NAME, filesystem::SpecialFileKind::Socket)
    }

    #[py_name = "Path.owner"]
    fn path_owner(args) -> Result<Value> {
        filesystem::path_owner(args, FN_NAME)
    }

    #[py_name = "Path.group"]
    fn path_group(args) -> Result<Value> {
        filesystem::path_group(args, FN_NAME)
    }

    #[py_name = "Path.cwd"]
    fn path_cwd(args) -> Result<Value> {
        filesystem::path_cwd(_interp, args)
    }

    #[py_name = "Path.home"]
    fn path_home(args) -> Result<Value> {
        filesystem::path_home(_interp, args)
    }
}

#[cfg(test)]
mod boundary_tests {
    const OWNER: &str = include_str!("pathlib.rs");
    const CLASS_REGISTRY: &str = include_str!("pathlib/class_registry.rs");
    const FILESYSTEM: &str = include_str!("pathlib/filesystem.rs");
    const LEXICAL: &str = include_str!("pathlib/lexical.rs");
    const PRESENTATION: &str = include_str!("pathlib/presentation.rs");
    const WALK: &str = include_str!("pathlib/walk.rs");
    const IMPLEMENTATIONS: &str = concat!(
        include_str!("pathlib/class_registry.rs"),
        include_str!("pathlib/filesystem.rs"),
        include_str!("pathlib/lexical.rs"),
        include_str!("pathlib/presentation.rs"),
        include_str!("pathlib/walk.rs"),
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
        assert!(WALK.contains("ProviderIterator::new"));

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
    fn registration_facade_stays_complete() {
        let pure_reg = concat!("#[py_", "name = \"PurePath.");
        let parents_reg = concat!("#[py_", "name = \"_PathParents.");
        let path_reg = concat!("#[py_", "name = \"Path.");
        assert_eq!(OWNER.matches(pure_reg).count(), 32);
        assert_eq!(OWNER.matches(parents_reg).count(), 5);
        assert_eq!(OWNER.matches(path_reg).count(), 40);
    }

    #[test]
    fn every_path_registration_is_installed_on_the_class() {
        let path_reg = concat!("#[py_", "name = \"Path.");
        let mut installed: Vec<&str> = super::class_registry::installed_path_method_names();
        installed.sort_unstable();
        let mut registered: Vec<&str> = OWNER
            .split(path_reg)
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect();
        registered.sort_unstable();
        assert_eq!(installed, registered);
    }
}
