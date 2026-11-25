"""The machinery of importlib: finders, loaders, hooks, etc."""

import os
import re
import sys
import warnings

_PYCACHE = '__pycache__'
_OPT = 'opt-'

SOURCE_SUFFIXES = ['.py', '.pyw'] if sys.platform == 'win32' else ['.py']
BYTECODE_SUFFIXES = ['.pyc']
DEBUG_BYTECODE_SUFFIXES = BYTECODE_SUFFIXES
OPTIMIZED_BYTECODE_SUFFIXES = BYTECODE_SUFFIXES
EXTENSION_SUFFIXES = ['.pyd'] if sys.platform == 'win32' else ['.so']
ALL_SUFFIXES = SOURCE_SUFFIXES + BYTECODE_SUFFIXES + EXTENSION_SUFFIXES

_PATH_SEPARATORS = '\\/' if sys.platform == 'win32' else '/'
_CODING_RE = re.compile(r'^[ \t\f]*#.*?coding[:=][ \t]*([-\w.]+)')


def all_suffixes():
    """Returns a list of all recognized module suffixes for this process"""
    return SOURCE_SUFFIXES + BYTECODE_SUFFIXES + EXTENSION_SUFFIXES


def _cache_from_source(path, debug_override=None, *, optimization=None):
    """Given the path to a .py file, return the path to its .pyc file."""
    if debug_override is not None:
        warnings.warn(
            "the debug_override parameter is deprecated; use 'optimization' instead",
            DeprecationWarning,
        )
        if optimization is not None:
            raise TypeError('debug_override or optimization must be set to None')
        optimization = '' if debug_override else 1
    path = os.fspath(path)
    head, tail = os.path.split(path)
    base, sep, rest = tail.rpartition('.')
    tag = sys.implementation.cache_tag
    if tag is None:
        raise NotImplementedError('sys.implementation.cache_tag is None')
    almost_filename = ''.join([(base if base else rest), sep, tag])
    if optimization is None:
        if sys.flags.optimize == 0:
            optimization = ''
        else:
            optimization = sys.flags.optimize
    optimization = str(optimization)
    if optimization != '':
        if not optimization.isalnum():
            raise ValueError(f'{optimization!r} is not alphanumeric')
        almost_filename = f'{almost_filename}.{_OPT}{optimization}'
    filename = almost_filename + BYTECODE_SUFFIXES[0]
    if sys.pycache_prefix is not None:
        head = os.path.abspath(head)
        if len(head) >= 2 and head[1] == ':' and head[0] not in _PATH_SEPARATORS:
            head = head[2:]
        return os.path.join(
            sys.pycache_prefix,
            head.lstrip(_PATH_SEPARATORS),
            filename,
        )
    return os.path.join(head, _PYCACHE, filename)


def _source_from_cache(path):
    """Given the path to a .pyc file, return the path to its .py file."""
    if sys.implementation.cache_tag is None:
        raise NotImplementedError('sys.implementation.cache_tag is None')
    path = os.fspath(path)
    head, pycache_filename = os.path.split(path)
    found_in_pycache_prefix = False
    if sys.pycache_prefix is not None:
        stripped_path = sys.pycache_prefix.rstrip(_PATH_SEPARATORS)
        if head.startswith(stripped_path + os.sep):
            head = head[len(stripped_path):]
            found_in_pycache_prefix = True
    if not found_in_pycache_prefix:
        head, pycache = os.path.split(head)
        if pycache != _PYCACHE:
            raise ValueError(f'{_PYCACHE} not bottom-level directory in {path!r}')
    dot_count = pycache_filename.count('.')
    if dot_count not in (2, 3):
        raise ValueError(f'expected only 2 or 3 dots in {pycache_filename!r}')
    elif dot_count == 3:
        optimization = pycache_filename.rsplit('.', 2)[-2]
        if not optimization.startswith(_OPT):
            raise ValueError(
                f'optimization portion of filename does not start with {_OPT!r}'
            )
        opt_level = optimization[len(_OPT):]
        if not opt_level.isalnum():
            raise ValueError(
                f'optimization level {optimization!r} is not an alphanumeric value'
            )
    base_filename = pycache_filename.partition('.')[0]
    return os.path.join(head, base_filename + SOURCE_SUFFIXES[0])


def _decode_source(source_bytes):
    """Decode bytes representing source code and return the string."""
    if not isinstance(source_bytes, (bytes, bytearray)):
        source_bytes = bytes(source_bytes)
    encoding = 'utf-8'
    if source_bytes.startswith(b'\xef\xbb\xbf'):
        source_bytes = source_bytes[3:]
    else:
        lines = source_bytes.splitlines()[:2]
        for raw_line in lines:
            try:
                line = raw_line.decode('ascii')
            except UnicodeDecodeError:
                break
            match = _CODING_RE.match(line)
            if match is not None:
                encoding = match.group(1)
                break
    try:
        text = source_bytes.decode(encoding)
    except LookupError:
        text = source_bytes.decode('utf-8')
    return text.replace('\r\n', '\n').replace('\r', '\n')


def _get_cached(filename):
    if filename.endswith(tuple(SOURCE_SUFFIXES)):
        try:
            return _cache_from_source(filename)
        except NotImplementedError:
            pass
    elif filename.endswith(tuple(BYTECODE_SUFFIXES)):
        return filename
    return None


class ModuleSpec:
    """The specification for a module, used for loading."""

    def __init__(self, name, loader, *, origin=None, loader_state=None,
                 is_package=None):
        self.name = name
        self.loader = loader
        self.origin = origin
        self.loader_state = loader_state
        self.submodule_search_locations = [] if is_package else None
        self._uninitialized_submodules = []
        self._set_fileattr = False
        self._cached = None

    def __repr__(self):
        args = [f'name={self.name!r}', f'loader={self.loader!r}']
        if self.origin is not None:
            args.append(f'origin={self.origin!r}')
        if self.submodule_search_locations is not None:
            args.append(
                f'submodule_search_locations={self.submodule_search_locations}'
            )
        return f'{self.__class__.__name__}({", ".join(args)})'

    def __eq__(self, other):
        smsl = self.submodule_search_locations
        try:
            return (
                self.name == other.name
                and self.loader == other.loader
                and self.origin == other.origin
                and smsl == other.submodule_search_locations
                and self.cached == other.cached
                and self.has_location == other.has_location
            )
        except AttributeError:
            return NotImplemented

    @property
    def cached(self):
        if self._cached is None:
            if self.origin is not None and self._set_fileattr:
                self._cached = _get_cached(self.origin)
        return self._cached

    @cached.setter
    def cached(self, value):
        self._cached = value

    @property
    def parent(self):
        """The name of the module's parent."""
        if self.submodule_search_locations is None:
            return self.name.rpartition('.')[0]
        else:
            return self.name

    @property
    def has_location(self):
        return self._set_fileattr

    @has_location.setter
    def has_location(self, value):
        self._set_fileattr = bool(value)


class BuiltinImporter:
    """Meta path import for built-in modules."""

    _ORIGIN = 'built-in'

    @classmethod
    def module_repr(cls, module):
        return f'<module {module.__name__!r} ({cls._ORIGIN})>'

    @classmethod
    def create_module(cls, spec):
        return None

    @classmethod
    def exec_module(cls, module):
        return None

    @classmethod
    def get_code(cls, fullname):
        return None

    @classmethod
    def get_source(cls, fullname):
        return None

    @classmethod
    def is_package(cls, fullname):
        return False

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        if path is not None:
            return None
        if fullname in sys.builtin_module_names:
            return ModuleSpec(fullname, cls, origin=cls._ORIGIN)
        return None


class FrozenImporter:
    """Meta path import for frozen modules."""

    _ORIGIN = 'frozen'

    @classmethod
    def module_repr(cls, module):
        return f'<module {module.__name__!r} ({cls._ORIGIN})>'

    @classmethod
    def create_module(cls, spec):
        return None

    @classmethod
    def exec_module(cls, module):
        return None

    @classmethod
    def get_code(cls, fullname):
        return None

    @classmethod
    def get_source(cls, fullname):
        return None

    @classmethod
    def is_package(cls, fullname):
        return False

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        return None


class _FileLoaderBase:
    """Base file loader implementing common filesystem loader protocol."""

    def __init__(self, fullname, path):
        self.name = fullname
        self.path = path

    def __eq__(self, other):
        return (
            self.__class__ == other.__class__
            and self.__dict__ == other.__dict__
        )

    def __hash__(self):
        return hash(self.name) ^ hash(self.path)

    def _check_name(self, fullname):
        if fullname is None:
            return self.name
        if self.name != fullname:
            raise ImportError(
                f'loader for {self.name} cannot handle {fullname}',
                name=fullname,
            )
        return fullname

    def get_filename(self, fullname):
        self._check_name(fullname)
        return self.path

    def get_data(self, path):
        with open(path, 'rb') as file:
            return file.read()

    def is_package(self, fullname):
        filename = os.path.split(self.get_filename(fullname))[1]
        filename_base = filename.rsplit('.', 1)[0]
        tail_name = fullname.rpartition('.')[2]
        return filename_base == '__init__' and tail_name != '__init__'

    def create_module(self, spec):
        """Use default semantics for module creation."""
        return None


class SourceFileLoader(_FileLoaderBase):
    """Concrete implementation of SourceLoader using the file system."""

    def get_source(self, fullname):
        path = self.get_filename(fullname)
        try:
            source_bytes = self.get_data(path)
        except OSError as exc:
            raise ImportError(
                f'source not available through get_data(): {exc}',
                name=fullname,
            ) from exc
        return _decode_source(source_bytes)

    def get_code(self, fullname):
        source_path = self.get_filename(fullname)
        source_bytes = self.get_data(source_path)
        source = _decode_source(source_bytes)
        return compile(source, source_path, 'exec')

    def exec_module(self, module):
        code = self.get_code(module.__name__)
        if code is None:
            raise ImportError(
                f'cannot load module {module.__name__!r} when get_code() returns None'
            )
        exec(code, module.__dict__)

    def path_stats(self, path):
        st = os.stat(path)
        return {'mtime': st.st_mtime, 'size': st.st_size}

    def set_data(self, path, data, *, _mode=0o666):
        parent = os.path.split(path)[0]
        if parent and not os.path.isdir(parent):
            os.makedirs(parent, exist_ok=True)
        with open(path, 'wb') as file:
            file.write(data)


class SourcelessFileLoader(_FileLoaderBase):
    """Loader which handles sourceless file imports."""

    def get_source(self, fullname):
        self._check_name(fullname)
        return None

    def get_code(self, fullname):
        self._check_name(fullname)
        raise ImportError(
            f'sourceless bytecode loading is not supported for {fullname!r}',
            name=fullname,
        )

    def exec_module(self, module):
        self.get_code(module.__name__)


class ExtensionFileLoader(_FileLoaderBase):
    """Loader for extension modules."""

    def get_source(self, fullname):
        self._check_name(fullname)
        return None

    def get_code(self, fullname):
        self._check_name(fullname)
        return None

    def exec_module(self, module):
        raise ImportError(
            f'extension module loading is not supported for {module.__name__!r}',
            name=module.__name__,
        )


class NamespaceLoader:
    """Loader for namespace packages."""

    def __init__(self, name, path, path_finder):
        self._path = path

    def is_package(self, fullname):
        return True

    def get_source(self, fullname):
        return ''

    def get_code(self, fullname):
        return compile('', '<string>', 'exec')

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        return None


class FileFinder:
    """File-based finder."""

    def __init__(self, path, *loader_details):
        self.path = path or '.'
        self._loaders = []
        for loader, suffixes in loader_details:
            self._loaders.extend((suffix, loader) for suffix in suffixes)

    def invalidate_caches(self):
        return None

    def find_spec(self, fullname, target=None):
        tail_module = fullname.rpartition('.')[2]
        base_path = os.path.join(self.path, tail_module)
        if os.path.isdir(base_path):
            for suffix, loader_class in self._loaders:
                init_filename = '__init__' + suffix
                full_path = os.path.join(base_path, init_filename)
                if os.path.isfile(full_path):
                    loader = loader_class(fullname, full_path)
                    spec = ModuleSpec(
                        fullname, loader, origin=full_path, is_package=True
                    )
                    spec.has_location = True
                    spec.submodule_search_locations = [base_path]
                    return spec
        for suffix, loader_class in self._loaders:
            full_path = base_path + suffix
            if os.path.isfile(full_path):
                loader = loader_class(fullname, full_path)
                spec = ModuleSpec(
                    fullname, loader, origin=full_path, is_package=False
                )
                spec.has_location = True
                return spec
        return None


    @classmethod
    def path_hook(cls, *loader_details):
        """A class method which returns a closure to use on sys.path_hooks
        which will return an instance using the specified loaders and the path
        called on the closure."""
        def path_hook_for_FileFinder(path):
            if not os.path.isdir(path or '.'):
                raise ImportError('only directories are supported', path=path)
            return cls(path, *loader_details)

        return path_hook_for_FileFinder


class PathFinder:
    """Meta path finder for sys.path and package __path__ attributes."""

    @classmethod
    def invalidate_caches(cls):
        for finder in sys.path_importer_cache.values():
            if hasattr(finder, 'invalidate_caches'):
                finder.invalidate_caches()

    @classmethod
    def _path_hooks(cls, path):
        hooks = sys.path_hooks
        if hooks is not None:
            for hook in hooks:
                try:
                    return hook(path)
                except ImportError:
                    continue
        return None

    @classmethod
    def _path_importer_cache(cls, path):
        if path == '':
            try:
                path = os.getcwd()
            except FileNotFoundError:
                return None
        cache = sys.path_importer_cache
        if path in cache:
            return cache[path]
        if sys.path_hooks:
            finder = cls._path_hooks(path)
            if finder is not None:
                cache[path] = finder
                return finder
            if not os.path.isdir(path):
                cache[path] = None
                return None
        return _UNSET_PATH_FINDER

    @classmethod
    def find_spec(cls, fullname, path=None, target=None):
        search_dirs = sys.path if path is None else path
        tail_module = fullname.rpartition('.')[2]
        namespace_path = []
        for entry in search_dirs:
            try:
                entry_str = os.fspath(entry)
            except TypeError:
                continue
            finder = cls._path_importer_cache(entry_str)
            if finder is None:
                continue
            if finder is not _UNSET_PATH_FINDER:
                if hasattr(finder, 'find_spec'):
                    spec = finder.find_spec(fullname, target)
                    if spec is not None:
                        if spec.loader is not None:
                            return spec
                        if spec.submodule_search_locations is not None:
                            namespace_path.extend(
                                spec.submodule_search_locations
                            )
                continue
            base_path = os.path.join(entry_str, tail_module)
            if os.path.isdir(base_path):
                for suffix in SOURCE_SUFFIXES:
                    init_path = os.path.join(base_path, '__init__' + suffix)
                    if os.path.isfile(init_path):
                        abs_init = os.path.abspath(init_path)
                        abs_dir = os.path.abspath(base_path)
                        loader = SourceFileLoader(fullname, abs_init)
                        spec = ModuleSpec(
                            fullname, loader, origin=abs_init, is_package=True
                        )
                        spec.has_location = True
                        spec.submodule_search_locations = [abs_dir]
                        return spec
            for suffix in SOURCE_SUFFIXES:
                mod_path = base_path + suffix
                if os.path.isfile(mod_path):
                    abs_mod = os.path.abspath(mod_path)
                    loader = SourceFileLoader(fullname, abs_mod)
                    spec = ModuleSpec(
                        fullname, loader, origin=abs_mod, is_package=False
                    )
                    spec.has_location = True
                    return spec
            if os.path.isdir(base_path):
                namespace_path.append(os.path.abspath(base_path))
        if namespace_path:
            loader = NamespaceLoader(fullname, namespace_path, cls)
            spec = ModuleSpec(fullname, loader, origin=None)
            spec.submodule_search_locations = namespace_path
            return spec
        return None


_UNSET_PATH_FINDER = object()
