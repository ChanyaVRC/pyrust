"""runpy.py - locating and running Python code using the module namespace."""

import importlib.machinery
import importlib.util
import os
import pkgutil
import sys
import warnings

__all__ = [
    "run_module",
    "run_path",
]

ModuleType = type(sys)


def _fsdecode(filename):
    filename = os.fspath(filename)
    if isinstance(filename, bytes):
        return filename.decode(sys.getfilesystemencoding())
    return filename


class _TempModule:
    """Temporarily replace a module in sys.modules with an empty namespace."""

    def __init__(self, mod_name):
        self.mod_name = mod_name
        self.module = ModuleType(mod_name)
        self._saved_module = []

    def __enter__(self):
        mod_name = self.mod_name
        try:
            self._saved_module.append(sys.modules[mod_name])
        except KeyError:
            pass
        sys.modules[mod_name] = self.module
        return self

    def __exit__(self, *args):
        if self._saved_module:
            sys.modules[self.mod_name] = self._saved_module[0]
        else:
            sys.modules.pop(self.mod_name, None)
        self._saved_module = []


class _ModifiedArgv0:
    def __init__(self, value):
        self.value = value
        self._saved_value = self._sentinel = object()
        self._was_empty = False

    def __enter__(self):
        if self._saved_value is not self._sentinel:
            raise RuntimeError("Already preserving saved value")
        if sys.argv:
            self._saved_value = sys.argv[0]
            sys.argv[0] = self.value
        else:
            self._was_empty = True
            self._saved_value = None
            sys.argv.append(self.value)

    def __exit__(self, *args):
        self.value = self._sentinel
        if self._was_empty:
            if sys.argv:
                sys.argv.pop(0)
        else:
            sys.argv[0] = self._saved_value


def _run_code(
    code,
    run_globals,
    init_globals=None,
    mod_name=None,
    mod_spec=None,
    pkg_name=None,
    script_name=None,
):
    """Helper to run code in nominated namespace."""
    if init_globals is not None:
        run_globals.update(init_globals)
    if mod_spec is None:
        loader = None
        fname = script_name
        cached = None
    else:
        loader = mod_spec.loader
        fname = mod_spec.origin
        cached = mod_spec.cached
        if pkg_name is None:
            pkg_name = mod_spec.parent
    run_globals.update(
        __name__=mod_name,
        __file__=fname,
        __cached__=cached,
        __doc__=None,
        __loader__=loader,
        __package__=pkg_name,
        __spec__=mod_spec,
    )
    exec(code, run_globals)
    return run_globals


def _run_module_code(
    code,
    init_globals=None,
    mod_name=None,
    mod_spec=None,
    pkg_name=None,
    script_name=None,
):
    """Helper to run code in new namespace with sys modified."""
    fname = script_name if mod_spec is None else mod_spec.origin
    with _TempModule(mod_name) as temp_module, _ModifiedArgv0(fname):
        mod_globals = temp_module.module.__dict__
        _run_code(
            code,
            mod_globals,
            init_globals,
            mod_name,
            mod_spec,
            pkg_name,
            script_name,
        )
    return mod_globals.copy()


def _get_module_details(mod_name, error=ImportError):
    if mod_name.startswith("."):
        raise error("Relative module names not supported")
    pkg_name, _, _ = mod_name.rpartition(".")
    if pkg_name:
        try:
            __import__(pkg_name)
        except ImportError as e:
            if e.name is None or (
                e.name != pkg_name and not pkg_name.startswith(e.name + ".")
            ):
                raise
        existing = sys.modules.get(mod_name)
        if existing is not None and not hasattr(existing, "__path__"):
            msg = (
                "{mod_name!r} found in sys.modules after import of "
                "package {pkg_name!r}, but prior to execution of "
                "{mod_name!r}; this may result in unpredictable "
                "behaviour"
            ).format(mod_name=mod_name, pkg_name=pkg_name)
            warnings.warn(RuntimeWarning(msg))

    try:
        spec = importlib.util.find_spec(mod_name)
    except (ImportError, AttributeError, TypeError, ValueError) as ex:
        msg = "Error while finding module specification for {!r} ({}: {})"
        if mod_name.endswith(".py"):
            msg += (
                f". Try using '{mod_name[:-3]}' instead of "
                f"'{mod_name}' as the module name."
            )
        raise error(msg.format(mod_name, type(ex).__name__, ex)) from ex
    if spec is None:
        raise error("No module named %s" % mod_name)
    if spec.submodule_search_locations is not None:
        if mod_name == "__main__" or mod_name.endswith(".__main__"):
            raise error("Cannot use package as __main__ module")
        try:
            pkg_main_name = mod_name + ".__main__"
            return _get_module_details(pkg_main_name, error)
        except error as e:
            if mod_name not in sys.modules:
                raise
            raise error(
                ("%s; %r is a package and cannot be directly executed")
                % (e, mod_name)
            )
    loader = spec.loader
    if loader is None:
        raise error("%r is a namespace package and cannot be executed" % mod_name)
    try:
        code = loader.get_code(mod_name)
    except ImportError as e:
        raise error(format(e)) from e
    if code is None:
        raise error("No code object available for %s" % mod_name)
    return mod_name, spec, code


class _Error(Exception):
    """Error that _run_module_as_main() should report without a traceback."""


def _run_module_as_main(mod_name, alter_argv=True):
    """Runs the designated module in the __main__ namespace."""
    try:
        if alter_argv or mod_name != "__main__":
            mod_name, mod_spec, code = _get_module_details(mod_name, _Error)
        else:
            mod_name, mod_spec, code = _get_main_module_details(_Error)
    except _Error as exc:
        msg = "%s: %s" % (sys.executable, exc)
        sys.exit(msg)
    main_globals = sys.modules["__main__"].__dict__
    if alter_argv:
        if sys.argv:
            sys.argv[0] = mod_spec.origin
        else:
            sys.argv.append(mod_spec.origin)
    return _run_code(code, main_globals, None, "__main__", mod_spec)


def run_module(mod_name, init_globals=None, run_name=None, alter_sys=False):
    """Execute a module's code without importing it.

    Returns the resulting module globals dictionary.
    """
    mod_name, mod_spec, code = _get_module_details(mod_name)
    if run_name is None:
        run_name = mod_name
    if alter_sys:
        return _run_module_code(code, init_globals, run_name, mod_spec)
    return _run_code(code, {}, init_globals, run_name, mod_spec)


def _get_main_module_details(error=ImportError):
    """Locate the __main__ module for directory or zipfile execution."""
    main_name = "__main__"
    has_saved = main_name in sys.modules
    saved_main = sys.modules.pop(main_name, None)
    try:
        return _get_module_details(main_name)
    except ImportError as exc:
        if main_name in str(exc):
            raise error(
                "can't find %r module in %r" % (main_name, sys.path[0])
            ) from exc
        raise
    finally:
        if has_saved:
            sys.modules[main_name] = saved_main


def _get_code_from_file(fname):
    code_path = os.path.abspath(fname)
    with open(code_path, "rb") as f:
        code = pkgutil.read_code(f)
    if code is None:
        with open(code_path, "rb") as f:
            source_bytes = f.read()
        source = importlib.util.decode_source(source_bytes)
        code = compile(source, fname, "exec")
    return code


class _ImporterMetaPathBridge:
    """Temporarily expose a path importer (e.g., zip archive) via sys.meta_path."""

    def __init__(self, importer):
        self._importer = importer

    def find_spec(self, fullname, path=None, target=None):
        if hasattr(self._importer, "find_spec"):
            return self._importer.find_spec(fullname, path, target)
        return None


def run_path(path_name, init_globals=None, run_name=None):
    """Execute code located at the specified filesystem location.

    path_name may refer to a Python script file, a directory containing
    __main__.py, or a zip archive containing __main__.py.
    """
    if run_name is None:
        run_name = "<run_path>"
    pkg_name = run_name.rpartition(".")[0]
    importer = pkgutil.get_importer(path_name)
    path_name = _fsdecode(path_name)
    if importer is None:
        code = _get_code_from_file(path_name)
        return _run_module_code(
            code,
            init_globals,
            run_name,
            pkg_name=pkg_name,
            script_name=path_name,
        )

    sys.path.insert(0, path_name)
    meta_bridge = None
    if not isinstance(importer, importlib.machinery.FileFinder):
        meta_bridge = _ImporterMetaPathBridge(importer)
        sys.meta_path.insert(0, meta_bridge)
    try:
        mod_name, mod_spec, code = _get_main_module_details()
        with _TempModule(run_name) as temp_module, _ModifiedArgv0(path_name):
            mod_globals = temp_module.module.__dict__
            return _run_code(
                code, mod_globals, init_globals, run_name, mod_spec, pkg_name
            ).copy()
    finally:
        if meta_bridge is not None:
            try:
                sys.meta_path.remove(meta_bridge)
            except ValueError:
                pass
        try:
            sys.path.remove(path_name)
        except ValueError:
            pass
