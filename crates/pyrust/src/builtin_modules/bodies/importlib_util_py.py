"""Utility code for constructing importers, etc."""

import os
import sys
import types

import importlib.machinery as _machinery

ModuleSpec = _machinery.ModuleSpec
BuiltinImporter = _machinery.BuiltinImporter
SourceFileLoader = _machinery.SourceFileLoader
SourcelessFileLoader = _machinery.SourcelessFileLoader
ExtensionFileLoader = _machinery.ExtensionFileLoader
NamespaceLoader = _machinery.NamespaceLoader
PathFinder = _machinery.PathFinder
SOURCE_SUFFIXES = _machinery.SOURCE_SUFFIXES
BYTECODE_SUFFIXES = _machinery.BYTECODE_SUFFIXES
EXTENSION_SUFFIXES = _machinery.EXTENSION_SUFFIXES

# CPython 3.12 bytecode magic number: 3531 (0x0dcb LE) + b'\r\n'.
MAGIC_NUMBER = b'\xcb\r\r\n'

_POPULATE = object()


def cache_from_source(path, debug_override=None, *, optimization=None):
    """Given the path to a .py file, return the path to its .pyc file."""
    return _machinery._cache_from_source(
        path, debug_override=debug_override, optimization=optimization
    )


def source_from_cache(path):
    """Given the path to a .pyc file, return the path to its .py file."""
    return _machinery._source_from_cache(path)


def decode_source(source_bytes):
    """Decode bytes representing source code and return the string."""
    return _machinery._decode_source(source_bytes)


def resolve_name(name, package):
    """Resolve a relative module name to an absolute one."""
    if not name.startswith('.'):
        return name
    elif not package:
        raise ImportError(
            f'no package specified for {repr(name)} '
            '(required for relative module names)'
        )
    level = 0
    for character in name:
        if character != '.':
            break
        level += 1
    bits = package.rsplit('.', level - 1)
    if len(bits) < level:
        raise ImportError('attempted relative import beyond top-level package')
    base = bits[0]
    tail = name[level:]
    return f'{base}.{tail}' if tail else base


def spec_from_loader(name, loader, *, origin=None, is_package=None):
    """Return a module spec based on various loader methods."""
    if origin is None:
        origin = getattr(loader, '_ORIGIN', None)

    if not origin and hasattr(loader, 'get_filename'):
        if is_package is None:
            return spec_from_file_location(name, loader=loader)
        search = [] if is_package else None
        return spec_from_file_location(
            name, loader=loader, submodule_search_locations=search
        )

    if is_package is None:
        if hasattr(loader, 'is_package'):
            try:
                is_package = loader.is_package(name)
            except ImportError:
                is_package = None
        else:
            is_package = False

    return ModuleSpec(name, loader, origin=origin, is_package=is_package)


def spec_from_file_location(name, location=None, *, loader=None,
                            submodule_search_locations=_POPULATE):
    """Return a module spec based on a file location."""
    if location is None:
        location = '<unknown>'
        if hasattr(loader, 'get_filename'):
            try:
                location = loader.get_filename(name)
            except ImportError:
                pass
    else:
        location = os.fspath(location)
        try:
            location = os.path.abspath(location)
        except OSError:
            pass

    spec = ModuleSpec(name, loader, origin=location)
    spec._set_fileattr = True

    if loader is None:
        supported = [
            (SourceFileLoader, SOURCE_SUFFIXES),
            (SourcelessFileLoader, BYTECODE_SUFFIXES),
            (ExtensionFileLoader, EXTENSION_SUFFIXES),
        ]
        for loader_class, suffixes in supported:
            if location.endswith(tuple(suffixes)):
                loader = loader_class(name, location)
                spec.loader = loader
                break
        else:
            return None

    if submodule_search_locations is _POPULATE:
        if hasattr(loader, 'is_package'):
            try:
                is_package = loader.is_package(name)
            except ImportError:
                pass
            else:
                if is_package:
                    spec.submodule_search_locations = []
    else:
        spec.submodule_search_locations = submodule_search_locations

    if spec.submodule_search_locations == []:
        if location:
            dirname = os.path.split(location)[0]
            spec.submodule_search_locations.append(dirname)

    return spec


def _init_module_attrs(spec, module, *, override=False):
    """Initialize module metadata attributes from a ModuleSpec."""
    if override or getattr(module, '__name__', None) is None:
        try:
            module.__name__ = spec.name
        except AttributeError:
            pass

    if override or getattr(module, '__loader__', None) is None:
        loader = spec.loader
        if loader is None:
            if spec.submodule_search_locations is not None:
                loader = NamespaceLoader(
                    spec.name, spec.submodule_search_locations, None
                )
                spec.loader = loader
                try:
                    module.__file__ = None
                except AttributeError:
                    pass
        try:
            module.__loader__ = loader
        except AttributeError:
            pass

    if override or getattr(module, '__package__', None) is None:
        try:
            module.__package__ = spec.parent
        except AttributeError:
            pass

    try:
        module.__spec__ = spec
    except AttributeError:
        pass

    if override or getattr(module, '__path__', None) is None:
        if spec.submodule_search_locations is not None:
            try:
                module.__path__ = spec.submodule_search_locations
            except AttributeError:
                pass

    if spec.has_location:
        if override or getattr(module, '__file__', None) is None:
            try:
                module.__file__ = spec.origin
            except AttributeError:
                pass
        if override or getattr(module, '__cached__', None) is None:
            try:
                module.__cached__ = spec.cached
            except AttributeError:
                pass

    return module


def module_from_spec(spec):
    """Create a module based on the provided spec."""
    module = None
    if hasattr(spec.loader, 'create_module'):
        module = spec.loader.create_module(spec)
    elif hasattr(spec.loader, 'exec_module'):
        raise ImportError(
            'loaders that define exec_module() must also define create_module()'
        )
    if module is None:
        module = types.ModuleType(spec.name)
    _init_module_attrs(spec, module)
    return module


def _builtin_spec(name):
    return ModuleSpec(
        name,
        BuiltinImporter,
        origin='built-in',
        is_package=_is_builtin_package(name),
    )


def _find_spec(name, path, target=None):
    """Find a ModuleSpec for `name` using meta_path, built-in modules, and
    filesystem search paths."""
    if not name:
        return None
    meta_path = sys.meta_path
    if meta_path is None:
        raise ImportError(
            'sys.meta_path is None, Python is likely shutting down'
        )
    for finder in meta_path:
        if hasattr(finder, 'find_spec'):
            spec = finder.find_spec(name, path, target)
            if spec is not None:
                return spec

    if _is_builtin_module(name):
        return _builtin_spec(name)

    return PathFinder.find_spec(name, path, target)


def _synthesize_loaded_module_spec(fullname, module):
    """Synthesize a ModuleSpec for a PyRust-loaded module whose `__spec__` slot
    is still None. Returns None if the module appears to be user-constructed
    (e.g. `types.ModuleType` or `__main__`), so CPython's ValueError fires."""
    if fullname == '__main__':
        return None
    if _is_builtin_module(fullname):
        return _builtin_spec(fullname)
    file_attr = getattr(module, '__file__', None)
    if isinstance(file_attr, str) and file_attr:
        pkg_path = getattr(module, '__path__', None)
        if pkg_path is not None:
            return spec_from_file_location(
                fullname,
                file_attr,
                submodule_search_locations=list(pkg_path),
            )
        return spec_from_file_location(
            fullname, file_attr, submodule_search_locations=None
        )
    return None


def find_spec(name, package=None):
    """Return the spec for the specified module."""
    fullname = resolve_name(name, package) if name.startswith('.') else name
    if not fullname:
        return None
    if fullname not in sys.modules:
        parent_name = fullname.rpartition('.')[0]
        if parent_name:
            parent = __import__(parent_name, fromlist=['__path__'])
            # Importing the parent may have populated fullname in sys.modules.
            if fullname in sys.modules:
                module = sys.modules[fullname]
                if module is None:
                    return None
                try:
                    spec = module.__spec__
                except AttributeError:
                    raise ValueError(f'{name}.__spec__ is not set') from None
                if spec is None:
                    spec = _synthesize_loaded_module_spec(fullname, module)
                    if spec is None:
                        raise ValueError(f'{name}.__spec__ is None')
                return spec
            if _is_builtin_module(fullname):
                return _builtin_spec(fullname)
            try:
                parent_path = parent.__path__
            except AttributeError as exc:
                if _is_builtin_package(parent_name):
                    return None
                raise ModuleNotFoundError(
                    f"__path__ attribute not found on {parent_name!r} "
                    f"while trying to find {fullname!r}",
                    name=fullname,
                ) from exc
        else:
            parent_path = None
        return _find_spec(fullname, parent_path)
    else:
        module = sys.modules[fullname]
        if module is None:
            return None
        try:
            spec = module.__spec__
        except AttributeError:
            raise ValueError(f'{name}.__spec__ is not set') from None
        else:
            if spec is None:
                spec = _synthesize_loaded_module_spec(fullname, module)
                if spec is None:
                    raise ValueError(f'{name}.__spec__ is None')
            return spec
