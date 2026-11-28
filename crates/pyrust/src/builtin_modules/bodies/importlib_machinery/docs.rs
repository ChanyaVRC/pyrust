//! Docstrings of `importlib.machinery` and the `_bootstrap` /
//! `_bootstrap_external` classes it re-exports, verbatim from CPython 3.12.3.

pub(super) const MODULE_DOC: &str = "The machinery of importlib: finders, loaders, hooks, etc.";

pub(super) const MODULE_SPEC_DOC: &str = "The specification for a module, used for loading.\n\n    A module's spec is the source for information about the module.  For\n    data associated with the module, including source, use the spec's\n    loader.\n\n    `name` is the absolute name of the module.  `loader` is the loader\n    to use when loading the module.  `parent` is the name of the\n    package the module is in.  The parent is derived from the name.\n\n    `is_package` determines if the module is considered a package or\n    not.  On modules this is reflected by the `__path__` attribute.\n\n    `origin` is the specific location used by the loader from which to\n    load the module, if that information is available.  When filename is\n    set, origin will match.\n\n    `has_location` indicates that a spec's \"origin\" reflects a location.\n    When this is True, `__file__` attribute of the module is set.\n\n    `cached` is the location of the cached bytecode file, if any.  It\n    corresponds to the `__cached__` attribute.\n\n    `submodule_search_locations` is the sequence of path entries to\n    search when importing submodules.  If set, is_package should be\n    True--and False otherwise.\n\n    Packages are simply modules that (may) have submodules.  If a spec\n    has a non-None value in `submodule_search_locations`, the import\n    system will consider modules loaded from the spec as packages.\n\n    Only finders (see importlib.abc.MetaPathFinder and\n    importlib.abc.PathEntryFinder) should modify ModuleSpec instances.\n\n    ";

pub(super) const BUILTIN_IMPORTER_DOC: &str = "Meta path import for built-in modules.\n\n    All methods are either class or static methods to avoid the need to\n    instantiate the class.\n\n    ";

pub(super) const FROZEN_IMPORTER_DOC: &str = "Meta path import for frozen modules.\n\n    All methods are either class or static methods to avoid the need to\n    instantiate the class.\n\n    ";

pub(super) const WINDOWS_REGISTRY_FINDER_DOC: &str =
    "Meta path finder for modules declared in the Windows registry.";

pub(super) const LOADER_BASICS_DOC: &str =
    "Base class of common code needed by both SourceLoader and\n    SourcelessFileLoader.";

pub(super) const FILE_LOADER_DOC: &str = "Base file loader class which implements the loader protocol methods that\n    require file system usage.";

pub(super) const SOURCE_FILE_LOADER_DOC: &str =
    "Concrete implementation of SourceLoader using the file system.";

pub(super) const SOURCELESS_FILE_LOADER_DOC: &str = "Loader which handles sourceless file imports.";

pub(super) const EXTENSION_FILE_LOADER_DOC: &str = "Loader for extension modules.\n\n    The constructor is designed to work with FileFinder.\n\n    ";

pub(super) const NAMESPACE_PATH_DOC: &str = "Represents a namespace package's path.  It uses the module name\n    to find its parent module, and from there it looks up the parent's\n    __path__.  When this changes, the module's own path is recomputed,\n    using path_finder.  For top-level modules, the parent module's path\n    is sys.path.";

pub(super) const PATH_FINDER_DOC: &str =
    "Meta path finder for sys.path and package __path__ attributes.";

pub(super) const FILE_FINDER_DOC: &str = "File-based finder.\n\n    Interactions with the file system are cached for performance, being\n    refreshed when the directory the finder is handling has been modified.\n\n    ";

pub(super) const PATH_HOOK_FOR_FILE_FINDER_DOC: &str =
    "Path hook for importlib.machinery.FileFinder.";
