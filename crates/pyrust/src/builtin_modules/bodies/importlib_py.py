"""A pure Python implementation of import."""

import builtins as _builtins
import sys

__all__ = ['__import__', 'import_module', 'invalidate_caches', 'reload']

__import__ = _builtins.__import__

_RELOADING = {}


def invalidate_caches():
    """Call the invalidate_caches() method on all meta path finders stored in
    sys.meta_path (where implemented)."""
    for finder in sys.meta_path:
        if hasattr(finder, 'invalidate_caches'):
            finder.invalidate_caches()


def import_module(name, package=None):
    """Import a module.

    The 'package' argument is required when performing a relative import. It
    specifies the package to use as the anchor point from which to resolve the
    relative import to an absolute import.
    """
    if not isinstance(name, str):
        raise TypeError(f'module name must be str, not {type(name)}')
    level = 0
    if name.startswith('.'):
        if not package:
            raise TypeError(
                "the 'package' argument is required to perform a "
                f'relative import for {name!r}'
            )
        for character in name:
            if character != '.':
                break
            level += 1
    rel_name = name[level:]
    if level > 0:
        if not isinstance(package, str):
            raise TypeError('__package__ not set to a string')
        bits = package.rsplit('.', level - 1)
        if len(bits) < level:
            raise ImportError(
                'attempted relative import beyond top-level package'
            )
        base = bits[0]
        if not base:
            raise ImportError(
                'attempted relative import beyond top-level package'
            )
        fullname = f'{base}.{rel_name}' if rel_name else base
    else:
        if not rel_name:
            raise ValueError('Empty module name')
        fullname = rel_name

    return _load_module(fullname)


def reload(module):
    """Reload the module and return it.

    The module must have been successfully imported before.
    """
    try:
        name = module.__spec__.name
    except AttributeError:
        try:
            name = module.__name__
        except AttributeError:
            raise TypeError('reload() argument must be a module') from None

    if sys.modules.get(name) is not module:
        raise ImportError(f'module {name} not in sys.modules', name=name)
    if name in _RELOADING:
        return _RELOADING[name]
    _RELOADING[name] = module
    try:
        parent_name = name.rpartition('.')[0]
        if parent_name:
            try:
                parent = sys.modules[parent_name]
            except KeyError:
                raise ImportError(
                    f'parent {parent_name!r} not in sys.modules',
                    name=parent_name,
                ) from None
            else:
                try:
                    pkgpath = parent.__path__
                except AttributeError:
                    if _is_builtin_module(name):
                        pkgpath = None
                    else:
                        raise ModuleNotFoundError(
                            f"No module named {name!r}; {parent_name!r} is not a package",
                            name=name,
                        ) from None
        else:
            pkgpath = None

        import importlib.util as _util

        spec = _util._find_spec(name, pkgpath, module)
        if spec is None:
            raise ModuleNotFoundError(
                f'spec not found for the module {name!r}', name=name
            )
        module.__spec__ = spec
        _util._init_module_attrs(spec, module, override=True)
        if spec.loader is not None and hasattr(spec.loader, 'exec_module'):
            spec.loader.exec_module(module)
        # The module may have replaced itself in sys.modules.
        return sys.modules[name]
    finally:
        try:
            del _RELOADING[name]
        except KeyError:
            pass
