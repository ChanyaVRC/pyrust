"""Utilities to support packages."""

from collections import namedtuple
from functools import singledispatch as simplegeneric
import importlib
import importlib.machinery
import importlib.util
import os
import os.path
import sys
from types import ModuleType
import warnings
import zipfile

__all__ = [
    "get_importer",
    "iter_importers",
    "get_loader",
    "find_loader",
    "walk_packages",
    "iter_modules",
    "get_data",
    "read_code",
    "extend_path",
    "ModuleInfo",
]

ModuleInfo = namedtuple("ModuleInfo", "module_finder name ispkg")
ModuleInfo.__doc__ = "A namedtuple with minimal info about a module."


def _fsdecode(filename):
    filename = os.fspath(filename)
    if isinstance(filename, bytes):
        return filename.decode(sys.getfilesystemencoding())
    return filename


class _ZipImporter:
    """Minimal PEP 302 / PEP 451 zip archive finder and loader."""

    def __init__(self, archive, prefix=""):
        self.archive = archive
        self.prefix = prefix

    def __repr__(self):
        path = self.archive
        if self.prefix:
            path = os.path.join(path, self.prefix.rstrip("/"))
        return f"<zipimporter object {path!r}>"

    def _list_entries(self):
        with zipfile.ZipFile(self.archive, "r") as zf:
            return zf.namelist()

    def _entry_candidates(self, fullname):
        rel = fullname.replace(".", "/")
        if self.prefix:
            base = self.prefix + rel
        else:
            base = rel
        return (
            (base + "/__init__.py", True),
            (base + ".py", False),
        )

    def _find_entry(self, fullname):
        names = set(self._list_entries())
        for entry, is_pkg in self._entry_candidates(fullname):
            if entry in names:
                return entry, is_pkg
        return None, False

    def find_spec(self, fullname, path=None, target=None):
        entry, is_pkg = self._find_entry(fullname)
        if entry is None:
            return None
        origin = os.path.join(self.archive, *entry.split("/"))
        spec = importlib.machinery.ModuleSpec(
            fullname, self, origin=origin, is_package=is_pkg
        )
        spec.has_location = True
        if is_pkg:
            pkg_rel = entry.rsplit("/", 1)[0]
            spec.submodule_search_locations = [
                os.path.join(self.archive, *pkg_rel.split("/"))
            ]
        return spec

    def is_package(self, fullname):
        entry, is_pkg = self._find_entry(fullname)
        if entry is None:
            raise ImportError(f"can't find module {fullname!r}", name=fullname)
        return is_pkg

    def get_filename(self, fullname):
        entry, _ = self._find_entry(fullname)
        if entry is None:
            raise ImportError(f"can't find module {fullname!r}", name=fullname)
        return os.path.join(self.archive, *entry.split("/"))

    def get_data(self, pathname):
        norm_arch = os.path.abspath(self.archive)
        norm_path = os.path.abspath(pathname)
        if norm_path.startswith(norm_arch + os.sep):
            rel = norm_path[len(norm_arch) + 1 :].replace(os.sep, "/")
        else:
            rel = pathname.replace("\\", "/")
        try:
            with zipfile.ZipFile(self.archive, "r") as zf:
                return zf.read(rel)
        except KeyError as exc:
            raise OSError(f"cannot read {pathname!r} from {self.archive!r}") from exc

    def get_source(self, fullname):
        entry, _ = self._find_entry(fullname)
        if entry is None:
            raise ImportError(f"can't find module {fullname!r}", name=fullname)
        with zipfile.ZipFile(self.archive, "r") as zf:
            raw = zf.read(entry)
        return importlib.util.decode_source(raw)

    def get_code(self, fullname):
        source = self.get_source(fullname)
        origin = self.get_filename(fullname)
        return compile(source, origin, "exec")

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        code = self.get_code(module.__name__)
        exec(code, module.__dict__)

    def iter_modules(self, prefix=""):
        import inspect

        dirlist = sorted(self._list_entries())
        _prefix = self.prefix
        plen = len(_prefix)
        yielded = {}
        for fn in dirlist:
            if not fn.startswith(_prefix):
                continue
            parts = fn[plen:].split("/")
            if len(parts) == 2 and parts[1].startswith("__init__.py"):
                if parts[0] and parts[0] not in yielded:
                    yielded[parts[0]] = 1
                    yield prefix + parts[0], True
            if len(parts) != 1:
                continue
            modname = inspect.getmodulename(parts[0])
            if modname == "__init__":
                continue
            if modname and "." not in modname and modname not in yielded:
                yielded[modname] = 1
                yield prefix + modname, False


def _default_path_hook(path_item):
    target = path_item if path_item else os.getcwd()
    if os.path.isdir(target):
        return importlib.machinery.FileFinder(
            target,
            (
                importlib.machinery.SourceFileLoader,
                importlib.machinery.SOURCE_SUFFIXES,
            ),
            (
                importlib.machinery.SourcelessFileLoader,
                importlib.machinery.BYTECODE_SUFFIXES,
            ),
            (
                importlib.machinery.ExtensionFileLoader,
                importlib.machinery.EXTENSION_SUFFIXES,
            ),
        )
    if os.path.isfile(target) and zipfile.is_zipfile(target):
        return _ZipImporter(target)
    return None


def read_code(stream):
    """Read a compiled code object from a .pyc binary stream if supported."""
    magic = stream.read(4)
    if magic != importlib.util.MAGIC_NUMBER:
        return None
    # PyRust executes from Python source; bytecode unmarshalling is unavailable.
    return None


def walk_packages(path=None, prefix="", onerror=None):
    """Yields ModuleInfo for all modules recursively on path,
    or, if path is None, all accessible modules.
    """
    seen_paths = set()

    def _seen(p):
        if p in seen_paths:
            return True
        seen_paths.add(p)
        return False

    for info in iter_modules(path, prefix):
        yield info

        if info.ispkg:
            try:
                __import__(info.name)
            except ImportError:
                if onerror is not None:
                    onerror(info.name)
            except Exception:
                if onerror is not None:
                    onerror(info.name)
                else:
                    raise
            else:
                subpath = getattr(sys.modules[info.name], "__path__", None) or []
                subpath = [p for p in subpath if not _seen(p)]
                yield from walk_packages(subpath, info.name + ".", onerror)


def iter_modules(path=None, prefix=""):
    """Yields ModuleInfo for all submodules on path,
    or, if path is None, all top-level modules on sys.path.
    """
    if path is None:
        importers = iter_importers()
    elif isinstance(path, str):
        raise ValueError(
            "path must be None or list of paths to look for modules in"
        )
    else:
        importers = map(get_importer, path)

    yielded = {}
    for i in importers:
        for name, ispkg in iter_importer_modules(i, prefix):
            if name not in yielded:
                yielded[name] = 1
                yield ModuleInfo(i, name, ispkg)


@simplegeneric
def iter_importer_modules(importer, prefix=""):
    if not hasattr(importer, "iter_modules"):
        return []
    return importer.iter_modules(prefix)


def _iter_file_finder_modules(importer, prefix=""):
    if importer.path is None or not os.path.isdir(importer.path):
        return

    yielded = {}
    import inspect

    try:
        filenames = os.listdir(importer.path)
    except OSError:
        filenames = []
    filenames.sort()  # handle packages before same-named modules

    for fn in filenames:
        modname = inspect.getmodulename(fn)
        if modname == "__init__" or modname in yielded:
            continue

        full = os.path.join(importer.path, fn)
        ispkg = False

        if not modname and os.path.isdir(full) and "." not in fn:
            modname = fn
            try:
                dircontents = os.listdir(full)
            except OSError:
                dircontents = []
            for subfn in dircontents:
                subname = inspect.getmodulename(subfn)
                if subname == "__init__":
                    ispkg = True
                    break
            else:
                continue  # not a package

        if modname and "." not in modname:
            yielded[modname] = 1
            yield prefix + modname, ispkg


iter_importer_modules.register(
    importlib.machinery.FileFinder, _iter_file_finder_modules
)


def get_importer(path_item):
    """Retrieve a finder for the given path item.

    The returned finder is cached in sys.path_importer_cache if it was
    newly created by a path hook.
    """
    path_item = _fsdecode(path_item)
    try:
        importer = sys.path_importer_cache[path_item]
    except KeyError:
        for path_hook in sys.path_hooks:
            try:
                importer = path_hook(path_item)
                sys.path_importer_cache.setdefault(path_item, importer)
                break
            except ImportError:
                pass
        else:
            importer = _default_path_hook(path_item)
            if importer is not None:
                sys.path_importer_cache.setdefault(path_item, importer)
    return importer


def iter_importers(fullname=""):
    """Yield finders for the given module name."""
    if fullname.startswith("."):
        msg = "Relative module name {!r} not supported".format(fullname)
        raise ImportError(msg)
    if "." in fullname:
        pkg_name = fullname.rpartition(".")[0]
        pkg = importlib.import_module(pkg_name)
        path = getattr(pkg, "__path__", None)
        if path is None:
            return
    else:
        yield from sys.meta_path
        path = sys.path
    for item in path:
        yield get_importer(item)


def get_loader(module_or_name):
    """Get a loader object for module_or_name (deprecated in Python 3.12)."""
    warnings.warn(
        "'pkgutil.get_loader' is deprecated and slated for removal in "
        "Python 3.14; use importlib.util.find_spec() instead",
        DeprecationWarning,
        stacklevel=2,
    )
    if module_or_name in sys.modules:
        module_or_name = sys.modules[module_or_name]
        if module_or_name is None:
            return None
    if isinstance(module_or_name, ModuleType):
        module = module_or_name
        loader = getattr(module, "__loader__", None)
        if loader is not None:
            return loader
        if getattr(module, "__spec__", None) is None:
            if (
                getattr(module, "__file__", None) is None
                and module.__name__ not in sys.builtin_module_names
            ):
                return None
        fullname = module.__name__
    else:
        fullname = module_or_name
    return find_loader(fullname)


def find_loader(fullname):
    """Find a loader object for fullname (deprecated in Python 3.12)."""
    warnings.warn(
        "'pkgutil.find_loader' is deprecated and slated for removal in "
        "Python 3.14; use importlib.util.find_spec() instead",
        DeprecationWarning,
        stacklevel=2,
    )
    if fullname.startswith("."):
        msg = "Relative module name {!r} not supported".format(fullname)
        raise ImportError(msg)
    try:
        spec = importlib.util.find_spec(fullname)
    except (ImportError, AttributeError, TypeError, ValueError) as ex:
        msg = "Error while finding loader for {!r} ({}: {})"
        raise ImportError(msg.format(fullname, type(ex), ex)) from ex
    return spec.loader if spec is not None else None


def extend_path(path, name):
    """Extend a package's path with matching subdirectories and .pkg files."""
    if not isinstance(path, list):
        return path

    sname_pkg = name + ".pkg"
    path = path[:]  # Start with a copy of the existing path

    parent_package, _, final_name = name.rpartition(".")
    if parent_package:
        try:
            search_path = sys.modules[parent_package].__path__
        except (KeyError, AttributeError):
            return path
    else:
        search_path = sys.path

    for directory in search_path:
        if not isinstance(directory, str):
            continue

        finder = get_importer(directory)
        if finder is not None:
            portions = []
            if hasattr(finder, "find_spec"):
                spec = finder.find_spec(final_name)
                if spec is not None:
                    portions = spec.submodule_search_locations or []
                elif isinstance(finder, importlib.machinery.FileFinder):
                    subdir = os.path.join(finder.path, final_name)
                    if os.path.isdir(subdir):
                        portions = [subdir]
            elif hasattr(finder, "find_loader"):
                _, portions = finder.find_loader(final_name)

            for portion in portions:
                if portion not in path:
                    path.append(portion)

        pkgfile = os.path.join(directory, sname_pkg)
        if os.path.isfile(pkgfile):
            try:
                f = open(pkgfile)
            except OSError as msg:
                sys.stderr.write("Can't open %s: %s\n" % (pkgfile, msg))
            else:
                with f:
                    for line in f:
                        line = line.rstrip("\n")
                        if not line or line.startswith("#"):
                            continue
                        path.append(line)  # Don't check for existence!

    return path


def get_data(package, resource):
    """Get a binary resource from a package."""
    spec = importlib.util.find_spec(package)
    if spec is None:
        return None
    loader = spec.loader
    if loader is None or not hasattr(loader, "get_data"):
        return None
    mod = sys.modules.get(package)
    if mod is None:
        mod = importlib.import_module(package)
    if mod is None:
        return None
    mod_file = getattr(mod, "__file__", None)
    if mod_file is not None:
        base_dir = os.path.dirname(mod_file)
    else:
        pkg_path = getattr(mod, "__path__", None)
        if pkg_path:
            base_dir = pkg_path[0]
        else:
            return None

    parts = resource.split("/")
    parts.insert(0, base_dir)
    resource_name = os.path.join(*parts)
    return loader.get_data(resource_name)


def _is_valid_dotted_name(text):
    if not text:
        return False
    for part in text.split("."):
        if not part or not part.isidentifier():
            return False
    return True


def resolve_name(name):
    """Resolve a name to an object.

    Accepted formats:
      W(.W)*
      W(.W)*:(W(.W)*)?
    """
    if not isinstance(name, str):
        raise TypeError(
            f"expected string or bytes-like object, got {type(name).__name__!r}"
        )
    if ":" in name:
        colon_parts = name.split(":")
        if len(colon_parts) != 2:
            raise ValueError(f"invalid format: {name!r}")
        pkg, obj = colon_parts
        if not _is_valid_dotted_name(pkg):
            raise ValueError(f"invalid format: {name!r}")
        if obj and not _is_valid_dotted_name(obj):
            raise ValueError(f"invalid format: {name!r}")
        mod = importlib.import_module(pkg)
        parts = obj.split(".") if obj else []
    else:
        if not _is_valid_dotted_name(name):
            raise ValueError(f"invalid format: {name!r}")
        parts = name.split(".")
        modname = parts.pop(0)
        mod = importlib.import_module(modname)
        while parts:
            p = parts[0]
            s = f"{modname}.{p}"
            try:
                mod = importlib.import_module(s)
                parts.pop(0)
                modname = s
            except ImportError:
                break

    result = mod
    for p in parts:
        result = getattr(result, p)
    return result
