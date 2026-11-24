"""Get useful information from live Python objects."""

import abc
import builtins
import collections.abc
import enum
import functools
import itertools
import os
import re
import sys
import types
from collections import OrderedDict, namedtuple
from operator import attrgetter

__all__ = [
    "AGEN_CLOSED",
    "AGEN_CREATED",
    "AGEN_RUNNING",
    "AGEN_SUSPENDED",
    "ArgInfo",
    "Arguments",
    "Attribute",
    "BoundArguments",
    "BufferFlags",
    "CORO_CLOSED",
    "CORO_CREATED",
    "CORO_RUNNING",
    "CORO_SUSPENDED",
    "CO_ASYNC_GENERATOR",
    "CO_COROUTINE",
    "CO_GENERATOR",
    "CO_ITERABLE_COROUTINE",
    "CO_NESTED",
    "CO_NEWLOCALS",
    "CO_NOFREE",
    "CO_OPTIMIZED",
    "CO_VARARGS",
    "CO_VARKEYWORDS",
    "ClassFoundException",
    "ClosureVars",
    "EndOfBlock",
    "FrameInfo",
    "FullArgSpec",
    "GEN_CLOSED",
    "GEN_CREATED",
    "GEN_RUNNING",
    "GEN_SUSPENDED",
    "Parameter",
    "Signature",
    "TPFLAGS_IS_ABSTRACT",
    "Traceback",
    "classify_class_attrs",
    "cleandoc",
    "currentframe",
    "findsource",
    "formatannotation",
    "formatannotationrelativeto",
    "formatargvalues",
    "get_annotations",
    "getabsfile",
    "getargs",
    "getargvalues",
    "getasyncgenlocals",
    "getasyncgenstate",
    "getattr_static",
    "getblock",
    "getcallargs",
    "getclasstree",
    "getclosurevars",
    "getcomments",
    "getcoroutinelocals",
    "getcoroutinestate",
    "getdoc",
    "getfile",
    "getframeinfo",
    "getfullargspec",
    "getgeneratorlocals",
    "getgeneratorstate",
    "getinnerframes",
    "getlineno",
    "getmembers",
    "getmembers_static",
    "getmodule",
    "getmodulename",
    "getmro",
    "getouterframes",
    "getsource",
    "getsourcefile",
    "getsourcelines",
    "indentsize",
    "isabstract",
    "isasyncgen",
    "isasyncgenfunction",
    "isawaitable",
    "isbuiltin",
    "isclass",
    "iscode",
    "iscoroutine",
    "iscoroutinefunction",
    "isdatadescriptor",
    "isframe",
    "isfunction",
    "isgenerator",
    "isgeneratorfunction",
    "isgetsetdescriptor",
    "ismemberdescriptor",
    "ismethod",
    "ismethoddescriptor",
    "ismethodwrapper",
    "ismodule",
    "isroutine",
    "istraceback",
    "markcoroutinefunction",
    "signature",
    "stack",
    "trace",
    "unwrap",
    "walktree",
]

# Compiler flag constants (matching CPython Include/code.h)
CO_OPTIMIZED = 0x0001
CO_NEWLOCALS = 0x0002
CO_VARARGS = 0x0004
CO_VARKEYWORDS = 0x0008
CO_NESTED = 0x0010
CO_GENERATOR = 0x0020
CO_NOFREE = 0x0040
CO_COROUTINE = 0x0080
CO_ITERABLE_COROUTINE = 0x0100
CO_ASYNC_GENERATOR = 0x0200

# See Include/object.h
TPFLAGS_IS_ABSTRACT = 1 << 20

_KEYWORDS = frozenset((
    'False', 'None', 'True', 'and', 'as', 'assert', 'async', 'await',
    'break', 'class', 'continue', 'def', 'del', 'elif', 'else', 'except',
    'finally', 'for', 'from', 'global', 'if', 'import', 'in', 'is',
    'lambda', 'nonlocal', 'not', 'or', 'pass', 'raise', 'return', 'try',
    'while', 'with', 'yield',
))


def _iskeyword(name):
    return name in _KEYWORDS


def _unwrap_partial(func):
    while isinstance(func, functools.partial):
        func = func.func
    return func


# Lightweight file-line reader (avoids requiring standalone `linecache`).
def _read_source_lines(filename):
    if not filename or (filename.startswith('<') and filename.endswith('>')):
        return []
    try:
        with open(filename, 'r', encoding='utf-8', errors='replace') as fp:
            lines = fp.readlines()
    except OSError:
        return []
    if lines and not lines[-1].endswith('\n'):
        lines[-1] += '\n'
    return lines


def get_annotations(obj, *, globals=None, locals=None, eval_str=False):
    """Compute the annotations dict for an object."""
    if isinstance(obj, type):
        obj_dict = getattr(obj, '__dict__', None)
        if obj_dict and hasattr(obj_dict, 'get'):
            ann = obj_dict.get('__annotations__', None)
            if isgetsetdescriptor(ann):
                ann = None
        else:
            ann = None

        obj_globals = None
        module_name = getattr(obj, '__module__', None)
        if module_name:
            module = sys.modules.get(module_name, None)
            if module:
                obj_globals = getattr(module, '__dict__', None)
        obj_locals = dict(vars(obj))
        unwrap_target = obj
    elif isinstance(obj, types.ModuleType):
        ann = getattr(obj, '__annotations__', None)
        obj_globals = getattr(obj, '__dict__')
        obj_locals = None
        unwrap_target = None
    elif callable(obj):
        ann = getattr(obj, '__annotations__', None)
        obj_globals = getattr(obj, '__globals__', None)
        obj_locals = None
        unwrap_target = obj
    else:
        raise TypeError(f"{obj!r} is not a module, class, or callable.")

    if ann is None:
        return {}

    if not isinstance(ann, dict):
        raise ValueError(f"{obj!r}.__annotations__ is neither a dict nor None")

    if not ann:
        return {}

    if not eval_str:
        return dict(ann)

    if unwrap_target is not None:
        while True:
            if hasattr(unwrap_target, '__wrapped__'):
                unwrap_target = unwrap_target.__wrapped__
                continue
            if isinstance(unwrap_target, functools.partial):
                unwrap_target = unwrap_target.func
                continue
            break
        if hasattr(unwrap_target, '__globals__'):
            obj_globals = unwrap_target.__globals__

    if globals is None:
        globals = obj_globals
    if locals is None:
        locals = obj_locals or {}

    type_params = getattr(obj, '__type_params__', ())
    if type_params:
        tp_locals = {param.__name__: param for param in type_params}
        tp_locals.update(locals)
        locals = tp_locals

    return {
        key: value if not isinstance(value, str) else eval(value, globals, locals)
        for key, value in ann.items()
    }


# ----------------------------------------------------------- type-checking

def ismodule(object):
    """Return true if the object is a module."""
    return isinstance(object, types.ModuleType)


def isclass(object):
    """Return true if the object is a class."""
    return isinstance(object, type)


def ismethod(object):
    """Return true if the object is an instance method."""
    return isinstance(object, types.MethodType)


def ismethoddescriptor(object):
    """Return true if the object is a method descriptor.

    But not if ismethod() or isclass() or isfunction() are true.
    """
    if isclass(object) or ismethod(object) or isfunction(object):
        return False
    tp = type(object)
    if tp in (
        types.MethodDescriptorType,
        types.WrapperDescriptorType,
        types.ClassMethodDescriptorType,
    ):
        return True
    if isinstance(object, (classmethod, staticmethod)):
        return True
    return hasattr(tp, "__get__") and not hasattr(tp, "__set__")


def isdatadescriptor(object):
    """Return true if the object is a data descriptor."""
    if isclass(object) or ismethod(object) or isfunction(object):
        return False
    if ismemberdescriptor(object) or isgetsetdescriptor(object):
        return True
    tp = type(object)
    return hasattr(tp, "__set__") or hasattr(tp, "__delete__")


def ismemberdescriptor(object):
    """Return true if the object is a member descriptor."""
    return (
        isinstance(object, types.MemberDescriptorType)
        or type(object) is types.MemberDescriptorType
    )


def isgetsetdescriptor(object):
    """Return true if the object is a getset descriptor."""
    return (
        isinstance(object, types.GetSetDescriptorType)
        or type(object) is types.GetSetDescriptorType
    )


def isfunction(object):
    """Return true if the object is a user-defined function."""
    return isinstance(object, types.FunctionType)


def _has_code_flag(f, flag):
    """Return true if ``f`` is a function (or a method or functools.partial
    wrapper wrapping a function) whose code object has the given ``flag``
    set in its flags."""
    while ismethod(f):
        f = f.__func__
    f = _unwrap_partial(f)
    if not (isfunction(f) or _signature_is_functionlike(f)):
        return False
    return bool(f.__code__.co_flags & flag)


def isgeneratorfunction(obj):
    """Return true if the object is a user-defined generator function."""
    return _has_code_flag(obj, CO_GENERATOR)


_is_coroutine_mark = object()


def _has_coroutine_mark(f):
    while ismethod(f):
        f = f.__func__
    f = _unwrap_partial(f)
    return getattr(f, "_is_coroutine_marker", None) is _is_coroutine_mark


def markcoroutinefunction(func):
    """Decorator to ensure callable is recognised as a coroutine function."""
    if hasattr(func, '__func__'):
        func = func.__func__
    func._is_coroutine_marker = _is_coroutine_mark
    return func


def iscoroutinefunction(obj):
    """Return true if the object is a coroutine function."""
    return _has_code_flag(obj, CO_COROUTINE) or _has_coroutine_mark(obj)


def isasyncgenfunction(obj):
    """Return true if the object is an asynchronous generator function."""
    return _has_code_flag(obj, CO_ASYNC_GENERATOR)


def isasyncgen(object):
    """Return true if the object is an asynchronous generator."""
    return isinstance(object, types.AsyncGeneratorType)


def isgenerator(object):
    """Return true if the object is a generator."""
    return isinstance(object, types.GeneratorType)


def iscoroutine(object):
    """Return true if the object is a coroutine."""
    return isinstance(object, types.CoroutineType)


def isawaitable(object):
    """Return true if object can be passed to an ``await`` expression."""
    return (
        isinstance(object, types.CoroutineType)
        or (
            isinstance(object, types.GeneratorType)
            and bool(object.gi_code.co_flags & CO_ITERABLE_COROUTINE)
        )
        or isinstance(object, collections.abc.Awaitable)
    )


def istraceback(object):
    """Return true if the object is a traceback."""
    return (
        isinstance(object, types.TracebackType)
        or type(object) is types.TracebackType
    )


def isframe(object):
    """Return true if the object is a frame object."""
    return (
        isinstance(object, types.FrameType)
        or type(object) is types.FrameType
    )


def iscode(object):
    """Return true if the object is a code object."""
    return (
        isinstance(object, types.CodeType)
        or type(object) is types.CodeType
    )


def isbuiltin(object):
    """Return true if the object is a built-in function or method."""
    return type(object) is types.BuiltinFunctionType


def ismethodwrapper(object):
    """Return true if the object is a method wrapper."""
    return type(object) is types.MethodWrapperType


def isroutine(object):
    """Return true if the object is any kind of function or method."""
    return (
        isbuiltin(object)
        or isfunction(object)
        or ismethod(object)
        or ismethoddescriptor(object)
        or ismethodwrapper(object)
    )


def isabstract(object):
    """Return true if the object is an abstract base class (ABC)."""
    if not isinstance(object, type):
        return False
    if getattr(object, '__flags__', 0) & TPFLAGS_IS_ABSTRACT:
        return True
    if abc.ABCMeta not in type(object).__mro__:
        return False
    if hasattr(object, '__abstractmethods__'):
        return bool(object.__abstractmethods__)
    for name, value in object.__dict__.items():
        if getattr(value, "__isabstractmethod__", False):
            return True
    for base in object.__bases__:
        for name in getattr(base, "__abstractmethods__", ()):
            value = getattr(object, name, None)
            if getattr(value, "__isabstractmethod__", False):
                return True
    return False


# ----------------------------------------------------------- member queries

def _getmembers(object, predicate, getter):
    results = []
    processed = set()
    names = dir(object)
    if isclass(object):
        mro = getmro(object)
        try:
            for base in object.__bases__:
                for k, v in base.__dict__.items():
                    if isinstance(v, types.DynamicClassAttribute):
                        names.append(k)
        except AttributeError:
            pass
    else:
        mro = ()
    for key in names:
        try:
            value = getter(object, key)
            if key in processed:
                raise AttributeError
        except AttributeError:
            for base in mro:
                if key in base.__dict__:
                    value = base.__dict__[key]
                    break
            else:
                continue
        if not predicate or predicate(value):
            results.append((key, value))
        processed.add(key)
    results.sort(key=lambda pair: pair[0])
    return results


def getmembers(object, predicate=None):
    """Return all members of an object as (name, value) pairs sorted by name.
    Optionally, only return members that satisfy a given predicate."""
    return _getmembers(object, predicate, getattr)


def getmembers_static(object, predicate=None):
    """Return all members of an object as (name, value) pairs sorted by name
    without triggering dynamic lookup via the descriptor protocol,
    __getattr__ or __getattribute__."""
    return _getmembers(object, predicate, getattr_static)


Attribute = namedtuple('Attribute', 'name kind defining_class object')


def classify_class_attrs(cls):
    """Return list of attribute-descriptor tuples."""
    mro = getmro(cls)
    metamro = getmro(type(cls))
    metamro = tuple(c for c in metamro if c not in (type, object))
    class_bases = (cls,) + mro
    all_bases = class_bases + metamro
    names = dir(cls)
    for base in mro:
        for k, v in base.__dict__.items():
            if isinstance(v, types.DynamicClassAttribute) and v.fget is not None:
                names.append(k)
    result = []
    processed = set()

    for name in names:
        homecls = None
        get_obj = None
        dict_obj = None
        if name not in processed:
            try:
                if name == '__dict__':
                    raise Exception("__dict__ is special, don't want the proxy")
                get_obj = getattr(cls, name)
            except Exception:
                pass
            else:
                homecls = getattr(get_obj, "__objclass__", homecls)
                if homecls not in class_bases:
                    homecls = None
                    last_cls = None
                    for srch_cls in class_bases:
                        srch_obj = getattr(srch_cls, name, None)
                        if srch_obj is get_obj:
                            last_cls = srch_cls
                    for srch_cls in metamro:
                        try:
                            srch_obj = srch_cls.__getattr__(cls, name)
                        except AttributeError:
                            continue
                        if srch_obj is get_obj:
                            last_cls = srch_cls
                    if last_cls is not None:
                        homecls = last_cls
        for base in all_bases:
            if name in base.__dict__:
                dict_obj = base.__dict__[name]
                if homecls not in metamro:
                    homecls = base
                break
        if homecls is None:
            continue
        obj = get_obj if get_obj is not None else dict_obj
        if isinstance(dict_obj, staticmethod) or isbuiltin(dict_obj):
            kind = "static method"
            obj = dict_obj
        elif isinstance(dict_obj, classmethod) or type(dict_obj) is types.ClassMethodDescriptorType:
            kind = "class method"
            obj = dict_obj
        elif isinstance(dict_obj, property):
            kind = "property"
            obj = dict_obj
        elif isroutine(obj):
            kind = "method"
        else:
            kind = "data"
        result.append(Attribute(name, kind, homecls, obj))
        processed.add(name)
    return result


# ----------------------------------------------------------- class helpers

def getmro(cls):
    """Return tuple of base classes (including cls) in method resolution order."""
    return cls.__mro__


# -------------------------------------------------------- function helpers

def unwrap(func, *, stop=None):
    """Get the object wrapped by *func*."""
    f = func
    memo = {id(f): f}
    recursion_limit = sys.getrecursionlimit()
    while not isinstance(func, type) and hasattr(func, '__wrapped__'):
        if stop is not None and stop(func):
            break
        func = func.__wrapped__
        id_func = id(func)
        if (id_func in memo) or (len(memo) >= recursion_limit):
            raise ValueError('wrapper loop when unwrapping {!r}'.format(f))
        memo[id_func] = func
    return func


# -------------------------------------------------- source code extraction

def indentsize(line):
    """Return the indent size, in spaces, at the start of a line of text."""
    expline = line.expandtabs()
    return len(expline) - len(expline.lstrip())


def _findclass(func):
    cls = sys.modules.get(func.__module__)
    if cls is None:
        return None
    for name in func.__qualname__.split('.')[:-1]:
        cls = getattr(cls, name)
    if not isclass(cls):
        return None
    return cls


def _finddoc(obj):
    if isclass(obj):
        for base in obj.__mro__:
            if base is not object:
                try:
                    doc = base.__doc__
                except AttributeError:
                    continue
                if doc is not None:
                    return doc
        return None

    if ismethod(obj):
        name = obj.__func__.__name__
        self = obj.__self__
        if (
            isclass(self)
            and getattr(getattr(self, name, None), '__func__', None) is obj.__func__
        ):
            cls = self
        else:
            cls = self.__class__
    elif isfunction(obj):
        name = obj.__name__
        cls = _findclass(obj)
        if cls is None or getattr(cls, name) is not obj:
            return None
    elif isbuiltin(obj):
        name = obj.__name__
        self = obj.__self__
        if (
            isclass(self)
            and self.__qualname__ + '.' + name == obj.__qualname__
        ):
            cls = self
        else:
            cls = self.__class__
    elif isinstance(obj, property):
        func = obj.fget
        name = func.__name__
        cls = _findclass(func)
        if cls is None or getattr(cls, name) is not obj:
            return None
    elif ismethoddescriptor(obj) or isdatadescriptor(obj):
        name = obj.__name__
        cls = obj.__objclass__
        if getattr(cls, name) is not obj:
            return None
        if ismemberdescriptor(obj):
            slots = getattr(cls, '__slots__', None)
            if isinstance(slots, dict) and name in slots:
                return slots[name]
    else:
        return None
    for base in cls.__mro__:
        try:
            doc = getattr(base, name).__doc__
        except AttributeError:
            continue
        if doc is not None:
            return doc
    return None


def getdoc(object):
    """Get the documentation string for an object."""
    try:
        doc = object.__doc__
    except AttributeError:
        return None
    if doc is None:
        try:
            doc = _finddoc(object)
        except (AttributeError, TypeError):
            return None
    if not isinstance(doc, str):
        return None
    return cleandoc(doc)


def cleandoc(doc):
    """Clean up indentation from docstrings."""
    try:
        lines = doc.expandtabs().split('\n')
    except UnicodeError:
        return None
    else:
        margin = sys.maxsize
        for line in lines[1:]:
            content = len(line.lstrip())
            if content:
                indent = len(line) - content
                margin = min(margin, indent)
        if lines:
            lines[0] = lines[0].lstrip()
        if margin < sys.maxsize:
            for i in range(1, len(lines)):
                lines[i] = lines[i][margin:]
        while lines and not lines[-1]:
            lines.pop()
        while lines and not lines[0]:
            lines.pop(0)
        return '\n'.join(lines)


def getfile(object):
    """Work out which source or compiled file an object was defined in."""
    if ismodule(object):
        if getattr(object, '__file__', None):
            return object.__file__
        raise TypeError('{!r} is a built-in module'.format(object))
    if isclass(object):
        if hasattr(object, '__module__'):
            module = sys.modules.get(object.__module__)
            if getattr(module, '__file__', None):
                return module.__file__
            if object.__module__ == '__main__':
                raise OSError('source code not available')
        raise TypeError('{!r} is a built-in class'.format(object))
    if ismethod(object):
        object = object.__func__
    if isfunction(object):
        object = object.__code__
    if istraceback(object):
        object = object.tb_frame
    if isframe(object):
        object = object.f_code
    if iscode(object):
        return object.co_filename
    raise TypeError(
        'module, class, method, function, traceback, frame, or '
        'code object was expected, got {}'.format(type(object).__name__)
    )


_SOURCE_SUFFIXES = ('.py',)
_BYTECODE_SUFFIXES = ('.pyc',)
_EXTENSION_SUFFIXES = ('.pyd', '.so', '.dylib')
_ALL_SUFFIXES = _SOURCE_SUFFIXES + _BYTECODE_SUFFIXES + _EXTENSION_SUFFIXES


def getmodulename(path):
    """Return the module name for a given file, or None."""
    fname = os.path.basename(path)
    suffixes = [(-len(suffix), suffix) for suffix in _ALL_SUFFIXES]
    suffixes.sort()
    for neglen, suffix in suffixes:
        if fname.endswith(suffix):
            return fname[:neglen]
    return None


def getsourcefile(object):
    """Return the filename that can be used to locate an object's source.
    Return None if no way can be identified to get the source."""
    filename = getfile(object)
    if any(filename.endswith(s) for s in _BYTECODE_SUFFIXES):
        filename = os.path.splitext(filename)[0] + _SOURCE_SUFFIXES[0]
    elif any(filename.endswith(s) for s in _EXTENSION_SUFFIXES):
        return None
    if os.path.exists(filename):
        return filename
    module = getmodule(object, filename)
    if getattr(module, '__loader__', None) is not None:
        return filename
    elif getattr(getattr(module, "__spec__", None), "loader", None) is not None:
        return filename
    return None


def _normcase(path):
    normcase = getattr(os.path, 'normcase', None)
    if normcase is not None:
        return normcase(path)
    if sys.platform == 'win32':
        return path.replace('/', '\\').lower()
    return path


def getabsfile(object, _filename=None):
    """Return an absolute path to the source or compiled file for an object."""
    if _filename is None:
        _filename = getsourcefile(object) or getfile(object)
    return _normcase(os.path.abspath(_filename))


modulesbyfile = {}
_filesbymodname = {}


def getmodule(object, _filename=None):
    """Return the module an object was defined in, or None if not found."""
    if ismodule(object):
        return object
    if hasattr(object, '__module__'):
        return sys.modules.get(object.__module__)
    if _filename is not None and _filename in modulesbyfile:
        return sys.modules.get(modulesbyfile[_filename])
    try:
        file = getabsfile(object, _filename)
    except (TypeError, FileNotFoundError):
        return None
    if file in modulesbyfile:
        return sys.modules.get(modulesbyfile[file])
    for modname, module in sys.modules.copy().items():
        if ismodule(module) and hasattr(module, '__file__'):
            f = module.__file__
            if f == _filesbymodname.get(modname, None):
                continue
            _filesbymodname[modname] = f
            f = getabsfile(module)
            realpath = getattr(os.path, 'realpath', os.path.abspath)
            modulesbyfile[f] = modulesbyfile[realpath(f)] = module.__name__
    if file in modulesbyfile:
        return sys.modules.get(modulesbyfile[file])
    main = sys.modules.get('__main__')
    if not hasattr(object, '__name__'):
        return None
    if main is not None and hasattr(main, object.__name__):
        mainobject = getattr(main, object.__name__)
        if mainobject is object:
            return main
    builtin = sys.modules.get('builtins')
    if builtin is not None and hasattr(builtin, object.__name__):
        builtinobject = getattr(builtin, object.__name__)
        if builtinobject is object:
            return builtin
    return None


class ClassFoundException(Exception):
    pass


def _find_class_line(lines, qualname):
    """Locate the 0-based line number (including decorators) for a class."""
    parts = [p for p in qualname.split('.') if p != '<locals>']
    target_name = parts[-1] if parts else qualname
    pat = re.compile(r'^(\s*)class\s+' + re.escape(target_name) + r'\b')
    matches = []
    for idx, line in enumerate(lines):
        m = pat.match(line)
        if m:
            start = idx
            indent = len(m.group(1).expandtabs())
            while start > 0:
                prev = lines[start - 1]
                if prev.lstrip().startswith('@') and indentsize(prev) == indent:
                    start -= 1
                else:
                    break
            matches.append((indent, start))
    if not matches:
        return None
    if len(parts) == 1:
        for indent, start in matches:
            if indent == 0:
                return start
    return matches[0][1]


def findsource(object):
    """Return the entire source file and starting line number for an object."""
    file = getsourcefile(object)
    if not file:
        file = getfile(object)
        if not (file.startswith('<') and file.endswith('>')):
            raise OSError('source code not available')

    lines = _read_source_lines(file)
    if not lines:
        raise OSError('could not get source code')

    if ismodule(object):
        return lines, 0

    if isclass(object):
        qualname = object.__qualname__
        line_number = _find_class_line(lines, qualname)
        if line_number is not None:
            return lines, line_number
        raise OSError('could not find class definition')

    if ismethod(object):
        object = object.__func__
    if isfunction(object):
        object = object.__code__
    if istraceback(object):
        object = object.tb_frame
    if isframe(object):
        object = object.f_code
    if iscode(object):
        if not hasattr(object, 'co_firstlineno'):
            raise OSError('could not find function definition')
        lnum = object.co_firstlineno - 1
        pat = re.compile(
            r'^(\s*def\s)|(\s*async\s+def\s)|(.*(?<!\w)lambda(:|\s))|^(\s*@)'
        )
        while lnum > 0:
            try:
                line = lines[lnum]
            except IndexError:
                raise OSError('lineno is out of bounds')
            if pat.match(line):
                break
            lnum = lnum - 1
        # Step back over any immediately preceding decorators at the same indent.
        if 0 <= lnum < len(lines):
            base_indent = indentsize(lines[lnum])
            while lnum > 0:
                prev = lines[lnum - 1]
                if prev.lstrip().startswith('@') and indentsize(prev) == base_indent:
                    lnum -= 1
                else:
                    break
        return lines, lnum
    raise OSError('could not find code object')


def getcomments(object):
    """Get lines of comments immediately preceding an object's source code.

    Returns None when source can't be found."""
    try:
        lines, lnum = findsource(object)
    except (OSError, TypeError):
        return None

    if ismodule(object):
        start = 0
        if lines and lines[0][:2] == '#!':
            start = 1
        while start < len(lines) and lines[start].strip() in ('', '#'):
            start = start + 1
        if start < len(lines) and lines[start][:1] == '#':
            comments = []
            end = start
            while end < len(lines) and lines[end][:1] == '#':
                comments.append(lines[end].expandtabs())
                end = end + 1
            return ''.join(comments)

    elif lnum > 0:
        indent = indentsize(lines[lnum])
        end = lnum - 1
        if (
            end >= 0
            and lines[end].lstrip()[:1] == '#'
            and indentsize(lines[end]) == indent
        ):
            comments = [lines[end].expandtabs().lstrip()]
            if end > 0:
                end = end - 1
                comment = lines[end].expandtabs().lstrip()
                while comment[:1] == '#' and indentsize(lines[end]) == indent:
                    comments[:0] = [comment]
                    end = end - 1
                    if end < 0:
                        break
                    comment = lines[end].expandtabs().lstrip()
            while comments and comments[0].strip() == '#':
                comments[:1] = []
            while comments and comments[-1].strip() == '#':
                comments[-1:] = []
            return ''.join(comments)
    return None


class EndOfBlock(Exception):
    pass


def _update_bracket_and_quote_state(line, paren_depth, triple_quote):
    """Scan a single source line updating open paren depth and triple-quote state."""
    i = 0
    n = len(line)
    while i < n:
        ch = line[i]
        if triple_quote is not None:
            if ch == '\\':
                i += 2
                continue
            if line[i:i + 3] == triple_quote:
                triple_quote = None
                i += 3
                continue
            i += 1
            continue
        if ch == '#':
            break
        if ch in ('"', "'"):
            if line[i:i + 3] == ch * 3:
                triple_quote = ch * 3
                i += 3
                continue
            quote = ch
            i += 1
            while i < n:
                if line[i] == '\\':
                    i += 2
                    continue
                if line[i] == quote:
                    i += 1
                    break
                i += 1
            continue
        if ch in '([{':
            paren_depth += 1
        elif ch in ')]}':
            paren_depth = max(0, paren_depth - 1)
        i += 1
    return paren_depth, triple_quote


def getblock(lines):
    """Extract the block of code at the top of the given list of lines."""
    if not lines:
        return []
    first = lines[0]
    base_indent = indentsize(first)
    # Check if this is a lambda line (not def/class/decorator).
    stripped0 = first.lstrip()
    is_block_head = (
        stripped0.startswith('@')
        or stripped0.startswith('def ')
        or stripped0.startswith('async def ')
        or stripped0.startswith('class ')
    )
    paren_depth = 0
    triple_quote = None
    in_header = True
    header_started = False
    last_code_line = 1

    if not is_block_head:
        # Lambda or single-statement expression: track until balanced newline.
        for idx, line in enumerate(lines):
            paren_depth, triple_quote = _update_bracket_and_quote_state(
                line, paren_depth, triple_quote
            )
            last_code_line = idx + 1
            if paren_depth == 0 and triple_quote is None:
                break
        return lines[:last_code_line]

    body_indent = None
    for idx, line in enumerate(lines):
        stripped = line.lstrip()
        if paren_depth == 0 and triple_quote is None:
            if in_header:
                if stripped.startswith('@') and not header_started:
                    paren_depth, triple_quote = _update_bracket_and_quote_state(
                        line, paren_depth, triple_quote
                    )
                    last_code_line = idx + 1
                    continue
                if (
                    stripped.startswith('def ')
                    or stripped.startswith('async def ')
                    or stripped.startswith('class ')
                ):
                    header_started = True
                    paren_depth, triple_quote = _update_bracket_and_quote_state(
                        line, paren_depth, triple_quote
                    )
                    last_code_line = idx + 1
                    if paren_depth == 0 and triple_quote is None:
                        # Check for single-line suite like `def f(): return 1`
                        code_part = line.split('#', 1)[0].rstrip()
                        if ':' in code_part:
                            after_colon = code_part.rsplit(':', 1)[1].strip()
                            in_header = False
                            if after_colon:
                                return lines[:idx + 1]
                    continue
                if header_started:
                    paren_depth, triple_quote = _update_bracket_and_quote_state(
                        line, paren_depth, triple_quote
                    )
                    last_code_line = idx + 1
                    if paren_depth == 0 and triple_quote is None:
                        code_part = line.split('#', 1)[0].rstrip()
                        if code_part.endswith(':'):
                            in_header = False
                    continue
            else:
                if not stripped:
                    continue
                indent = indentsize(line)
                if stripped.startswith('#'):
                    if body_indent is not None and indent >= body_indent:
                        last_code_line = idx + 1
                    continue
                if indent <= base_indent:
                    break
                if body_indent is None:
                    body_indent = indent
                last_code_line = idx + 1
                paren_depth, triple_quote = _update_bracket_and_quote_state(
                    line, paren_depth, triple_quote
                )
        else:
            paren_depth, triple_quote = _update_bracket_and_quote_state(
                line, paren_depth, triple_quote
            )
            last_code_line = idx + 1
            if in_header and header_started and paren_depth == 0 and triple_quote is None:
                code_part = line.split('#', 1)[0].rstrip()
                if code_part.endswith(':'):
                    in_header = False

    return lines[:last_code_line]


def getsourcelines(object):
    """Return a list of source lines and starting line number for an object."""
    object = unwrap(object)
    lines, lnum = findsource(object)

    if istraceback(object):
        object = object.tb_frame

    if (
        ismodule(object)
        or (isframe(object) and object.f_code.co_name == "<module>")
    ):
        return lines, 0
    else:
        return getblock(lines[lnum:]), lnum + 1


def getsource(object):
    """Return the text of the source code for an object."""
    lines, lnum = getsourcelines(object)
    return ''.join(lines)


# --------------------------------------------------- class tree extraction

def walktree(classes, children, parent):
    """Recursive helper function for getclasstree()."""
    results = []
    classes.sort(key=attrgetter('__module__', '__name__'))
    for c in classes:
        results.append((c, c.__bases__))
        if c in children:
            results.append(walktree(children[c], children, c))
    return results


def getclasstree(classes, unique=False):
    """Arrange the given list of classes into a hierarchy of nested lists."""
    children = {}
    roots = []
    for c in classes:
        if c.__bases__:
            for parent in c.__bases__:
                if parent not in children:
                    children[parent] = []
                if c not in children[parent]:
                    children[parent].append(c)
                if unique and parent in classes:
                    break
        elif c not in roots:
            roots.append(c)
    for parent in children:
        if parent not in classes:
            roots.append(parent)
    return walktree(roots, children, None)


# ------------------------------------------------ argument list extraction

Arguments = namedtuple('Arguments', 'args, varargs, varkw')


def getargs(co):
    """Get information about the arguments accepted by a code object."""
    if not iscode(co):
        raise TypeError('{!r} is not a code object'.format(co))

    names = co.co_varnames
    nargs = co.co_argcount
    nkwargs = co.co_kwonlyargcount
    args = list(names[:nargs])
    kwonlyargs = list(names[nargs:nargs + nkwargs])

    nargs += nkwargs
    varargs = None
    if co.co_flags & CO_VARARGS:
        varargs = co.co_varnames[nargs]
        nargs = nargs + 1
    varkw = None
    if co.co_flags & CO_VARKEYWORDS:
        varkw = co.co_varnames[nargs]
    return Arguments(args + kwonlyargs, varargs, varkw)


FullArgSpec = namedtuple(
    'FullArgSpec',
    'args, varargs, varkw, defaults, kwonlyargs, kwonlydefaults, annotations',
)


def getfullargspec(func):
    """Get the names and default values of a callable object's parameters."""
    try:
        sig = _signature_from_callable(
            func,
            follow_wrapper_chains=False,
            skip_bound_arg=False,
            sigcls=Signature,
            eval_str=False,
        )
    except Exception as ex:
        raise TypeError('unsupported callable') from ex

    args = []
    varargs = None
    varkw = None
    posonlyargs = []
    kwonlyargs = []
    annotations = {}
    defaults = ()
    kwdefaults = {}

    if sig.return_annotation is not sig.empty:
        annotations['return'] = sig.return_annotation

    for param in sig.parameters.values():
        kind = param.kind
        name = param.name

        if kind is _POSITIONAL_ONLY:
            posonlyargs.append(name)
            if param.default is not param.empty:
                defaults += (param.default,)
        elif kind is _POSITIONAL_OR_KEYWORD:
            args.append(name)
            if param.default is not param.empty:
                defaults += (param.default,)
        elif kind is _VAR_POSITIONAL:
            varargs = name
        elif kind is _KEYWORD_ONLY:
            kwonlyargs.append(name)
            if param.default is not param.empty:
                kwdefaults[name] = param.default
        elif kind is _VAR_KEYWORD:
            varkw = name

        if param.annotation is not param.empty:
            annotations[name] = param.annotation

    if not kwdefaults:
        kwdefaults = None

    if not defaults:
        defaults = None

    return FullArgSpec(
        posonlyargs + args,
        varargs,
        varkw,
        defaults,
        kwonlyargs,
        kwdefaults,
        annotations,
    )


ArgInfo = namedtuple('ArgInfo', 'args varargs keywords locals')


def getargvalues(frame):
    """Get information about arguments passed into a particular frame."""
    args, varargs, varkw = getargs(frame.f_code)
    return ArgInfo(args, varargs, varkw, frame.f_locals)


def formatannotation(annotation, base_module=None):
    if getattr(annotation, '__module__', None) == 'typing':
        def repl(match):
            text = match.group()
            return text.removeprefix('typing.')
        return re.sub(r'[\w\.]+', repl, repr(annotation))
    if isinstance(annotation, types.GenericAlias):
        return str(annotation)
    if isinstance(annotation, type):
        if annotation.__module__ in ('builtins', base_module):
            return annotation.__qualname__
        return annotation.__module__ + '.' + annotation.__qualname__
    return repr(annotation)


def formatannotationrelativeto(object):
    module = getattr(object, '__module__', None)

    def _formatannotation(annotation):
        return formatannotation(annotation, module)

    return _formatannotation


def formatargvalues(args, varargs, varkw, locals,
                    formatarg=str,
                    formatvarargs=lambda name: '*' + name,
                    formatvarkw=lambda name: '**' + name,
                    formatvalue=lambda value: '=' + repr(value)):
    """Format an argument spec from the 4 values returned by getargvalues."""
    def convert(name, locals=locals,
                formatarg=formatarg, formatvalue=formatvalue):
        return formatarg(name) + formatvalue(locals[name])
    specs = []
    for i in range(len(args)):
        specs.append(convert(args[i]))
    if varargs:
        specs.append(formatvarargs(varargs) + formatvalue(locals[varargs]))
    if varkw:
        specs.append(formatvarkw(varkw) + formatvalue(locals[varkw]))
    return '(' + ', '.join(specs) + ')'


def _missing_arguments(f_name, argnames, pos, values):
    names = [repr(name) for name in argnames if name not in values]
    missing = len(names)
    if missing == 1:
        s = names[0]
    elif missing == 2:
        s = "{} and {}".format(*names)
    else:
        tail = ", {} and {}".format(*names[-2:])
        del names[-2:]
        s = ", ".join(names) + tail
    raise TypeError(
        "%s() missing %i required %s argument%s: %s"
        % (
            f_name,
            missing,
            "positional" if pos else "keyword-only",
            "" if missing == 1 else "s",
            s,
        )
    )


def _too_many(f_name, args, kwonly, varargs, defcount, given, values):
    atleast = len(args) - defcount
    kwonly_given = len([arg for arg in kwonly if arg in values])
    if varargs:
        plural = atleast != 1
        sig = "at least %d" % (atleast,)
    elif defcount:
        plural = True
        sig = "from %d to %d" % (atleast, len(args))
    else:
        plural = len(args) != 1
        sig = str(len(args))
    kwonly_sig = ""
    if kwonly_given:
        msg = " positional argument%s (and %d keyword-only argument%s)"
        kwonly_sig = msg % (
            "s" if given != 1 else "",
            kwonly_given,
            "s" if kwonly_given != 1 else "",
        )
    raise TypeError(
        "%s() takes %s positional argument%s but %d%s %s given"
        % (
            f_name,
            sig,
            "s" if plural else "",
            given,
            kwonly_sig,
            "was" if given == 1 and not kwonly_given else "were",
        )
    )


def getcallargs(func, /, *positional, **named):
    """Get the mapping of arguments to values."""
    spec = getfullargspec(func)
    args, varargs, varkw, defaults, kwonlyargs, kwonlydefaults, ann = spec
    f_name = func.__name__
    arg2value = {}

    if ismethod(func) and func.__self__ is not None:
        positional = (func.__self__,) + positional
    num_pos = len(positional)
    num_args = len(args)
    num_defaults = len(defaults) if defaults else 0

    n = min(num_pos, num_args)
    for i in range(n):
        arg2value[args[i]] = positional[i]
    if varargs:
        arg2value[varargs] = tuple(positional[n:])
    possible_kwargs = set(args + kwonlyargs)
    if varkw:
        arg2value[varkw] = {}
    for kw, value in named.items():
        if kw not in possible_kwargs:
            if not varkw:
                raise TypeError(
                    "%s() got an unexpected keyword argument %r" % (f_name, kw)
                )
            arg2value[varkw][kw] = value
            continue
        if kw in arg2value:
            raise TypeError(
                "%s() got multiple values for argument %r" % (f_name, kw)
            )
        arg2value[kw] = value
    if num_pos > num_args and not varargs:
        _too_many(f_name, args, kwonlyargs, varargs, num_defaults, num_pos, arg2value)
    if num_pos < num_args:
        req = args[:num_args - num_defaults]
        for arg in req:
            if arg not in arg2value:
                _missing_arguments(f_name, req, True, arg2value)
        for i, arg in enumerate(args[num_args - num_defaults:]):
            if arg not in arg2value:
                arg2value[arg] = defaults[i]
    missing = 0
    for kwarg in kwonlyargs:
        if kwarg not in arg2value:
            if kwonlydefaults and kwarg in kwonlydefaults:
                arg2value[kwarg] = kwonlydefaults[kwarg]
            else:
                missing += 1
    if missing:
        _missing_arguments(f_name, kwonlyargs, False, arg2value)
    return arg2value


ClosureVars = namedtuple('ClosureVars', 'nonlocals globals builtins unbound')


def getclosurevars(func):
    """Get the mapping of free variables to their current values."""
    if ismethod(func):
        func = func.__func__

    if not isfunction(func):
        raise TypeError("{!r} is not a Python function".format(func))

    code = func.__code__
    if func.__closure__ is None:
        nonlocal_vars = {}
    else:
        nonlocal_vars = {
            var: cell.cell_contents
            for var, cell in zip(code.co_freevars, func.__closure__)
        }

    global_ns = func.__globals__
    builtin_ns = global_ns.get("__builtins__", builtins.__dict__)
    if ismodule(builtin_ns):
        builtin_ns = builtin_ns.__dict__
    global_vars = {}
    builtin_vars = {}
    unbound_names = set()
    global_names, attr_names = _code_names_by_kind(func)
    for name in attr_names:
        unbound_names.add(name)
    for name in global_names:
        try:
            global_vars[name] = global_ns[name]
        except KeyError:
            try:
                builtin_vars[name] = builtin_ns[name]
            except KeyError:
                unbound_names.add(name)

    return ClosureVars(nonlocal_vars, global_vars, builtin_vars, unbound_names)


# -------------------------------------------------- stack frame extraction

_Positions = namedtuple(
    'Positions',
    'lineno end_lineno col_offset end_col_offset',
    defaults=(None, None, None, None),
)

_Traceback = namedtuple('_Traceback', 'filename lineno function code_context index')


class Traceback(_Traceback):
    def __new__(cls, filename, lineno, function, code_context, index, *, positions=None):
        instance = super().__new__(cls, filename, lineno, function, code_context, index)
        instance.positions = positions
        return instance

    def __repr__(self):
        return (
            'Traceback(filename={!r}, lineno={!r}, function={!r}, '
            'code_context={!r}, index={!r}, positions={!r})'.format(
                self.filename,
                self.lineno,
                self.function,
                self.code_context,
                self.index,
                self.positions,
            )
        )


def _get_code_position(code, instruction_index):
    if instruction_index < 0:
        return (None, None, None, None)
    co_positions = getattr(code, 'co_positions', None)
    if co_positions is None:
        return (None, None, None, None)
    positions_gen = co_positions()
    return next(itertools.islice(positions_gen, instruction_index // 2, None))


def _get_code_position_from_tb(tb):
    return _get_code_position(tb.tb_frame.f_code, tb.tb_lasti)


def getframeinfo(frame, context=1):
    """Get information about a frame or traceback object."""
    if istraceback(frame):
        positions = _get_code_position_from_tb(frame)
        lineno = frame.tb_lineno
        frame = frame.tb_frame
    else:
        lineno = frame.f_lineno
        positions = _get_code_position(frame.f_code, frame.f_lasti)

    if positions[0] is None:
        positions = (lineno,) + positions[1:]
    lineno = positions[0]

    if not isframe(frame):
        raise TypeError('{!r} is not a frame or traceback object'.format(frame))

    filename = getsourcefile(frame) or getfile(frame)
    if context > 0 and lineno and lineno > 0:
        start = lineno - 1 - context // 2
        try:
            lines, lnum = findsource(frame)
        except OSError:
            lines = index = None
        else:
            start = max(0, min(start, len(lines) - context))
            lines = lines[start:start + context]
            index = lineno - 1 - start
    else:
        lines = index = None

    return Traceback(
        filename,
        lineno,
        frame.f_code.co_name,
        lines,
        index,
        positions=_Positions(*positions),
    )


def getlineno(frame):
    """Get the line number from a frame object."""
    return frame.f_lineno


_FrameInfo = namedtuple('_FrameInfo', ('frame',) + Traceback._fields)


class FrameInfo(_FrameInfo):
    def __new__(cls, frame, filename, lineno, function, code_context, index, *, positions=None):
        instance = super().__new__(
            cls, frame, filename, lineno, function, code_context, index
        )
        instance.positions = positions
        return instance

    def __repr__(self):
        return (
            'FrameInfo(frame={!r}, filename={!r}, lineno={!r}, function={!r}, '
            'code_context={!r}, index={!r}, positions={!r})'.format(
                self.frame,
                self.filename,
                self.lineno,
                self.function,
                self.code_context,
                self.index,
                self.positions,
            )
        )


def getouterframes(frame, context=1):
    """Get a list of records for a frame and all higher (calling) frames."""
    framelist = []
    while frame:
        traceback_info = getframeinfo(frame, context)
        frameinfo = (frame,) + traceback_info
        framelist.append(FrameInfo(*frameinfo, positions=traceback_info.positions))
        frame = frame.f_back
    return framelist


def getinnerframes(tb, context=1):
    """Get a list of records for a traceback's frame and all lower frames."""
    framelist = []
    while tb:
        traceback_info = getframeinfo(tb, context)
        frameinfo = (tb.tb_frame,) + traceback_info
        framelist.append(FrameInfo(*frameinfo, positions=traceback_info.positions))
        tb = tb.tb_next
    return framelist


def currentframe():
    """Return the frame of the caller or None if this is not possible."""
    return sys._getframe(1) if hasattr(sys, "_getframe") else None


def stack(context=1):
    """Return a list of records for the stack above the caller's frame."""
    return getouterframes(sys._getframe(1), context)


def trace(context=1):
    """Return a list of records for the stack below the current exception."""
    exc = sys.exception()
    tb = None if exc is None else exc.__traceback__
    return getinnerframes(tb, context)


# ------------------------------------------------ static version of getattr

_sentinel = object()


def _static_getmro(klass):
    return object.__getattribute__(klass, '__mro__')


def _get_dunder_dict_of_class(klass):
    return object.__getattribute__(klass, '__dict__')


def _check_instance(obj, attr):
    instance_dict = {}
    try:
        instance_dict = object.__getattribute__(obj, "__dict__")
    except AttributeError:
        pass
    if isinstance(instance_dict, dict):
        return dict.get(instance_dict, attr, _sentinel)
    try:
        return instance_dict[attr]
    except (KeyError, TypeError):
        return _sentinel


def _shadowed_dict(klass):
    for entry in _static_getmro(klass):
        dunder_dict = _get_dunder_dict_of_class(entry)
        if '__dict__' in dunder_dict:
            class_dict = dunder_dict['__dict__']
            if not (
                isgetsetdescriptor(class_dict)
                and getattr(class_dict, '__name__', None) == '__dict__'
                and getattr(class_dict, '__objclass__', None) is entry
            ):
                return class_dict
    return _sentinel


def _check_class(klass, attr):
    for entry in _static_getmro(klass):
        if _shadowed_dict(type(entry)) is _sentinel:
            entry_dict = _get_dunder_dict_of_class(entry)
            if attr in entry_dict:
                return entry_dict[attr]
    return _sentinel


def getattr_static(obj, attr, default=_sentinel):
    """Retrieve attributes without triggering dynamic lookup via the
    descriptor protocol, __getattr__ or __getattribute__."""
    instance_result = _sentinel

    objtype = type(obj)
    if type not in _static_getmro(objtype):
        klass = objtype
        dict_attr = _shadowed_dict(klass)
        if dict_attr is _sentinel or ismemberdescriptor(dict_attr):
            instance_result = _check_instance(obj, attr)
    else:
        klass = obj

    klass_result = _check_class(klass, attr)

    if instance_result is not _sentinel and klass_result is not _sentinel:
        if isdatadescriptor(klass_result) or (
            _check_class(type(klass_result), "__get__") is not _sentinel
            and (
                _check_class(type(klass_result), "__set__") is not _sentinel
                or _check_class(type(klass_result), "__delete__") is not _sentinel
            )
        ):
            return klass_result

    if instance_result is not _sentinel:
        return instance_result
    if klass_result is not _sentinel:
        return klass_result

    if obj is klass:
        for entry in _static_getmro(type(klass)):
            if _shadowed_dict(type(entry)) is _sentinel:
                entry_dict = _get_dunder_dict_of_class(entry)
                if attr in entry_dict:
                    return entry_dict[attr]
    if default is not _sentinel:
        return default
    raise AttributeError(attr)


# ------------------------------------------------ generator introspection

GEN_CREATED = 'GEN_CREATED'
GEN_RUNNING = 'GEN_RUNNING'
GEN_SUSPENDED = 'GEN_SUSPENDED'
GEN_CLOSED = 'GEN_CLOSED'


def getgeneratorstate(generator):
    """Get current state of a generator-iterator."""
    if generator.gi_running:
        return GEN_RUNNING
    if generator.gi_suspended:
        return GEN_SUSPENDED
    if generator.gi_frame is None:
        return GEN_CLOSED
    return GEN_CREATED


def getgeneratorlocals(generator):
    """Get the mapping of generator local variables to their current values."""
    if not isgenerator(generator):
        raise TypeError("{!r} is not a Python generator".format(generator))

    frame = getattr(generator, "gi_frame", None)
    if frame is not None:
        return generator.gi_frame.f_locals
    else:
        return {}


# ------------------------------------------------ coroutine introspection

CORO_CREATED = 'CORO_CREATED'
CORO_RUNNING = 'CORO_RUNNING'
CORO_SUSPENDED = 'CORO_SUSPENDED'
CORO_CLOSED = 'CORO_CLOSED'


def getcoroutinestate(coroutine):
    """Get current state of a coroutine object."""
    if coroutine.cr_running:
        return CORO_RUNNING
    if coroutine.cr_suspended:
        return CORO_SUSPENDED
    if coroutine.cr_frame is None:
        return CORO_CLOSED
    return CORO_CREATED


def getcoroutinelocals(coroutine):
    """Get the mapping of coroutine local variables to their current values."""
    frame = getattr(coroutine, "cr_frame", None)
    if frame is not None:
        return frame.f_locals
    else:
        return {}


# ----------------------------------- asynchronous generator introspection

AGEN_CREATED = 'AGEN_CREATED'
AGEN_RUNNING = 'AGEN_RUNNING'
AGEN_SUSPENDED = 'AGEN_SUSPENDED'
AGEN_CLOSED = 'AGEN_CLOSED'


def getasyncgenstate(agen):
    """Get current state of an asynchronous generator object."""
    if agen.ag_running:
        return AGEN_RUNNING
    if agen.ag_suspended:
        return AGEN_SUSPENDED
    if agen.ag_frame is None:
        return AGEN_CLOSED
    return AGEN_CREATED


def getasyncgenlocals(agen):
    """Get the mapping of asynchronous generator local variables to their current values."""
    if not isasyncgen(agen):
        raise TypeError(f"{agen!r} is not a Python async generator")

    frame = getattr(agen, "ag_frame", None)
    if frame is not None:
        return agen.ag_frame.f_locals
    else:
        return {}


###############################################################################
### Function Signature Object (PEP 362)
###############################################################################

_NonUserDefinedCallables = (
    types.WrapperDescriptorType,
    types.MethodWrapperType,
    types.ClassMethodDescriptorType,
    types.BuiltinFunctionType,
)


class _BoundFunction:
    """Internal lightweight stand-in for `types.MethodType(func, obj)`."""

    __slots__ = ('__func__', '__self__')

    def __init__(self, func, self_obj):
        self.__func__ = func
        self.__self__ = self_obj

    def __call__(self, *args, **kwargs):
        return self.__func__(self.__self__, *args, **kwargs)


def _is_non_user_defined_callable(obj):
    return (
        type(obj) in _NonUserDefinedCallables
        or type(obj) is types.MethodDescriptorType
    )


def _descriptor_get(descriptor, obj):
    if isclass(descriptor):
        return descriptor
    if isfunction(descriptor):
        return _BoundFunction(descriptor, obj)
    get = getattr(type(descriptor), '__get__', _sentinel)
    if get is not _sentinel:
        return get(descriptor, obj, type(obj))
    bound_get = getattr(descriptor, '__get__', _sentinel)
    if bound_get is not _sentinel:
        return bound_get(obj, type(obj))
    return descriptor


def _signature_get_user_defined_method(cls, method_name):
    """Private helper. Checks if ``cls`` has an attribute named ``method_name``
    and returns it only if it is a pure python function."""
    if method_name == '__new__':
        meth = getattr(cls, method_name, None)
    else:
        meth = getattr_static(cls, method_name, None)
    if meth is None or _is_non_user_defined_callable(meth):
        return None
    if method_name != '__new__':
        meth = _descriptor_get(meth, cls)
    return meth


def _signature_get_partial(wrapped_sig, partial, extra_args=()):
    """Private helper to calculate how 'wrapped_sig' signature will
    look like after applying a 'functools.partial' object (or alike) on it."""
    old_params = wrapped_sig.parameters
    new_params = OrderedDict(old_params.items())

    partial_args = partial.args or ()
    partial_keywords = partial.keywords or {}

    if extra_args:
        partial_args = extra_args + partial_args

    try:
        ba = wrapped_sig.bind_partial(*partial_args, **partial_keywords)
    except TypeError as ex:
        msg = 'partial object {!r} has incorrect arguments'.format(partial)
        raise ValueError(msg) from ex

    transform_to_kwonly = False
    for param_name, param in old_params.items():
        try:
            arg_value = ba.arguments[param_name]
        except KeyError:
            pass
        else:
            if param.kind is _POSITIONAL_ONLY:
                new_params.pop(param_name)
                continue

            if param.kind is _POSITIONAL_OR_KEYWORD:
                if param_name in partial_keywords:
                    transform_to_kwonly = True
                    new_params[param_name] = param.replace(default=arg_value)
                else:
                    new_params.pop(param.name)
                    continue

            if param.kind is _KEYWORD_ONLY:
                new_params[param_name] = param.replace(default=arg_value)

        if transform_to_kwonly:
            assert param.kind is not _POSITIONAL_ONLY

            if param.kind is _POSITIONAL_OR_KEYWORD:
                new_param = new_params[param_name].replace(kind=_KEYWORD_ONLY)
                new_params[param_name] = new_param
                new_params.move_to_end(param_name)
            elif param.kind in (_KEYWORD_ONLY, _VAR_KEYWORD):
                new_params.move_to_end(param_name)
            elif param.kind is _VAR_POSITIONAL:
                new_params.pop(param.name)

    return wrapped_sig.replace(parameters=new_params.values())


def _signature_bound_method(sig):
    """Private helper to transform signatures for unbound functions to bound methods."""
    params = tuple(sig.parameters.values())

    if not params or params[0].kind in (_VAR_KEYWORD, _KEYWORD_ONLY):
        raise ValueError('invalid method signature')

    kind = params[0].kind
    if kind in (_POSITIONAL_OR_KEYWORD, _POSITIONAL_ONLY):
        params = params[1:]
    else:
        if kind is not _VAR_POSITIONAL:
            raise ValueError('invalid argument type')

    return sig.replace(parameters=params)


def _signature_is_builtin(obj):
    """Private helper to test if `obj` is a callable that might
    support Argument Clinic's __text_signature__ protocol."""
    return (
        isbuiltin(obj)
        or ismethoddescriptor(obj)
        or _is_non_user_defined_callable(obj)
        or obj is type
        or obj is object
    )


def _signature_is_functionlike(obj):
    """Private helper to test if `obj` is a duck type of FunctionType."""
    if not callable(obj) or isclass(obj):
        return False

    name = getattr(obj, '__name__', None)
    code = getattr(obj, '__code__', None)
    defaults = getattr(obj, '__defaults__', _void)
    kwdefaults = getattr(obj, '__kwdefaults__', _void)
    annotations = getattr(obj, '__annotations__', None)

    return (
        iscode(code)
        and isinstance(name, str)
        and (defaults is None or isinstance(defaults, tuple))
        and (kwdefaults is None or isinstance(kwdefaults, dict))
        and (isinstance(annotations, dict) or annotations is None)
    )


def _signature_fromstr(cls, obj, s, skip_bound_arg=True):
    """Parse a `__text_signature__` string into a Signature."""
    if not s or not isinstance(s, str):
        raise ValueError("{!r} builtin has invalid signature".format(obj))
    s_clean = s.strip()
    self_parameter = None
    if s_clean.startswith('(') and s_clean.endswith(')'):
        inner = s_clean[1:-1].strip()
        if inner.startswith('$'):
            self_parameter = 0
            s_clean = '(' + inner[1:] + ')'
    ns = {}
    module_name = getattr(obj, '__module__', None)
    if module_name and module_name in sys.modules:
        ns.update(getattr(sys.modules[module_name], '__dict__', {}))
    try:
        exec("def _sig_fn" + s_clean + ":\n    pass\n", ns)
    except Exception:
        raise ValueError("{!r} builtin has invalid signature".format(obj)) from None
    sig = _signature_from_function(cls, ns['_sig_fn'], skip_bound_arg=skip_bound_arg)
    if self_parameter is not None:
        params = list(sig.parameters.values())
        if params:
            _self = getattr(obj, '__self__', None)
            self_isbound = _self is not None
            self_ismodule = ismodule(_self)
            if self_isbound and (self_ismodule or skip_bound_arg):
                params.pop(0)
            else:
                params[0] = params[0].replace(kind=cls._parameter_cls.POSITIONAL_ONLY)
            sig = sig.replace(parameters=params)
    return sig


def _signature_from_builtin(cls, func, skip_bound_arg=True):
    """Private helper function to get signature for builtin callables."""
    if not _signature_is_builtin(func):
        raise TypeError("{!r} is not a Python builtin function".format(func))

    if func is object:
        return cls()

    s = getattr(func, "__text_signature__", None)
    if not s:
        raise ValueError("no signature found for builtin {!r}".format(func))

    return _signature_fromstr(cls, func, s, skip_bound_arg)


def _signature_from_function(cls, func, skip_bound_arg=True,
                             globals=None, locals=None, eval_str=False):
    """Private helper: constructs Signature for the given python function."""
    is_duck_function = False
    if not isfunction(func):
        if _signature_is_functionlike(func):
            is_duck_function = True
        else:
            raise TypeError('{!r} is not a Python function'.format(func))

    s = getattr(func, "__text_signature__", None)
    if s:
        return _signature_fromstr(cls, func, s, skip_bound_arg)

    Parameter = cls._parameter_cls

    func_code = func.__code__
    pos_count = func_code.co_argcount
    arg_names = func_code.co_varnames
    posonly_count = func_code.co_posonlyargcount
    positional = arg_names[:pos_count]
    keyword_only_count = func_code.co_kwonlyargcount
    keyword_only = arg_names[pos_count:pos_count + keyword_only_count]
    annotations = get_annotations(func, globals=globals, locals=locals, eval_str=eval_str)
    defaults = func.__defaults__
    kwdefaults = func.__kwdefaults__

    if defaults:
        pos_default_count = len(defaults)
    else:
        pos_default_count = 0

    parameters = []

    non_default_count = pos_count - pos_default_count
    posonly_left = posonly_count

    for name in positional[:non_default_count]:
        kind = _POSITIONAL_ONLY if posonly_left else _POSITIONAL_OR_KEYWORD
        annotation = annotations.get(name, _empty)
        parameters.append(Parameter(name, annotation=annotation, kind=kind))
        if posonly_left:
            posonly_left -= 1

    for offset, name in enumerate(positional[non_default_count:]):
        kind = _POSITIONAL_ONLY if posonly_left else _POSITIONAL_OR_KEYWORD
        annotation = annotations.get(name, _empty)
        parameters.append(
            Parameter(name, annotation=annotation, kind=kind, default=defaults[offset])
        )
        if posonly_left:
            posonly_left -= 1

    if func_code.co_flags & CO_VARARGS:
        name = arg_names[pos_count + keyword_only_count]
        annotation = annotations.get(name, _empty)
        parameters.append(Parameter(name, annotation=annotation, kind=_VAR_POSITIONAL))

    for name in keyword_only:
        default = _empty
        if kwdefaults is not None:
            default = kwdefaults.get(name, _empty)
        annotation = annotations.get(name, _empty)
        parameters.append(
            Parameter(name, annotation=annotation, kind=_KEYWORD_ONLY, default=default)
        )

    if func_code.co_flags & CO_VARKEYWORDS:
        index = pos_count + keyword_only_count
        if func_code.co_flags & CO_VARARGS:
            index += 1
        name = arg_names[index]
        annotation = annotations.get(name, _empty)
        parameters.append(Parameter(name, annotation=annotation, kind=_VAR_KEYWORD))

    return cls(
        parameters,
        return_annotation=annotations.get('return', _empty),
        __validate_parameters__=is_duck_function,
    )


def _signature_from_callable(obj, *,
                             follow_wrapper_chains=True,
                             skip_bound_arg=True,
                             globals=None,
                             locals=None,
                             eval_str=False,
                             sigcls):
    """Private helper function to get signature for arbitrary callable objects."""
    _get_signature_of = functools.partial(
        _signature_from_callable,
        follow_wrapper_chains=follow_wrapper_chains,
        skip_bound_arg=skip_bound_arg,
        globals=globals,
        locals=locals,
        sigcls=sigcls,
        eval_str=eval_str,
    )

    if not callable(obj):
        raise TypeError('{!r} is not a callable object'.format(obj))

    if isinstance(obj, (types.MethodType, _BoundFunction)):
        sig = _get_signature_of(obj.__func__)
        if skip_bound_arg:
            return _signature_bound_method(sig)
        else:
            return sig

    if follow_wrapper_chains:
        obj = unwrap(
            obj,
            stop=(
                lambda f: hasattr(f, "__signature__")
                or isinstance(f, (types.MethodType, _BoundFunction))
            ),
        )
        if isinstance(obj, (types.MethodType, _BoundFunction)):
            return _get_signature_of(obj)

    try:
        sig = obj.__signature__
    except AttributeError:
        pass
    else:
        if sig is not None:
            o_sig = sig
            if not isinstance(sig, (Signature, str)) and callable(sig):
                sig = sig()
            if isinstance(sig, str):
                sig = _signature_fromstr(sigcls, obj, sig)
            if not isinstance(sig, Signature):
                raise TypeError(
                    'unexpected object {!r} in __signature__ attribute'.format(o_sig)
                )
            return sig

    partialmethod_cls = getattr(functools, 'partialmethod', None)
    if partialmethod_cls is not None:
        try:
            partialmethod = obj._partialmethod
        except AttributeError:
            pass
        else:
            if isinstance(partialmethod, partialmethod_cls):
                wrapped_sig = _get_signature_of(partialmethod.func)
                sig = _signature_get_partial(wrapped_sig, partialmethod, (None,))
                first_wrapped_param = tuple(wrapped_sig.parameters.values())[0]
                if first_wrapped_param.kind is Parameter.VAR_POSITIONAL:
                    return sig
                else:
                    sig_params = tuple(sig.parameters.values())
                    new_params = (first_wrapped_param,) + sig_params
                    return sig.replace(parameters=new_params)

    if isfunction(obj) or _signature_is_functionlike(obj):
        return _signature_from_function(
            sigcls,
            obj,
            skip_bound_arg=skip_bound_arg,
            globals=globals,
            locals=locals,
            eval_str=eval_str,
        )

    if _signature_is_builtin(obj):
        return _signature_from_builtin(sigcls, obj, skip_bound_arg=skip_bound_arg)

    if isinstance(obj, functools.partial):
        wrapped_sig = _get_signature_of(obj.func)
        return _signature_get_partial(wrapped_sig, obj)

    if isinstance(obj, type):
        call = _signature_get_user_defined_method(type(obj), '__call__')
        if call is not None:
            return _get_signature_of(call)

        new = _signature_get_user_defined_method(obj, '__new__')
        init = _signature_get_user_defined_method(obj, '__init__')

        for base in obj.__mro__:
            if new is not None and '__new__' in base.__dict__:
                sig = _get_signature_of(new)
                if skip_bound_arg:
                    sig = _signature_bound_method(sig)
                return sig
            elif init is not None and '__init__' in base.__dict__:
                return _get_signature_of(init)

        for base in obj.__mro__[:-1]:
            try:
                text_sig = base.__text_signature__
            except AttributeError:
                pass
            else:
                if text_sig:
                    return _signature_fromstr(sigcls, base, text_sig)

        if type not in obj.__mro__:
            if obj.__init__ is object.__init__ and obj.__new__ is object.__new__:
                return sigcls.from_callable(object)
            else:
                raise ValueError(
                    'no signature found for builtin type {!r}'.format(obj)
                )
    else:
        call = getattr_static(type(obj), '__call__', None)
        if call is not None:
            call = _descriptor_get(call, obj)
            return _get_signature_of(call)

    raise ValueError('callable {!r} is not supported by signature'.format(obj))


class _void:
    """A private marker - used in Parameter & Signature."""


class _empty:
    """Marker object for Signature.empty and Parameter.empty."""


class _ParameterKind(enum.IntEnum):
    POSITIONAL_ONLY = 0
    POSITIONAL_OR_KEYWORD = 1
    VAR_POSITIONAL = 2
    KEYWORD_ONLY = 3
    VAR_KEYWORD = 4

    def __str__(self):
        return self.name


_ParameterKind.POSITIONAL_ONLY.description = 'positional-only'
_ParameterKind.POSITIONAL_OR_KEYWORD.description = 'positional or keyword'
_ParameterKind.VAR_POSITIONAL.description = 'variadic positional'
_ParameterKind.KEYWORD_ONLY.description = 'keyword-only'
_ParameterKind.VAR_KEYWORD.description = 'variadic keyword'

_POSITIONAL_ONLY = _ParameterKind.POSITIONAL_ONLY
_POSITIONAL_OR_KEYWORD = _ParameterKind.POSITIONAL_OR_KEYWORD
_VAR_POSITIONAL = _ParameterKind.VAR_POSITIONAL
_KEYWORD_ONLY = _ParameterKind.KEYWORD_ONLY
_VAR_KEYWORD = _ParameterKind.VAR_KEYWORD


class Parameter:
    """Represents a parameter in a function signature."""

    __slots__ = ('_name', '_kind', '_default', '_annotation')

    POSITIONAL_ONLY = _POSITIONAL_ONLY
    POSITIONAL_OR_KEYWORD = _POSITIONAL_OR_KEYWORD
    VAR_POSITIONAL = _VAR_POSITIONAL
    KEYWORD_ONLY = _KEYWORD_ONLY
    VAR_KEYWORD = _VAR_KEYWORD

    empty = _empty

    def __init__(self, name, kind, *, default=_empty, annotation=_empty):
        if isinstance(kind, _ParameterKind):
            self._kind = kind
        else:
            try:
                self._kind = _ParameterKind(kind)
            except ValueError:
                raise ValueError(f'value {kind!r} is not a valid Parameter.kind')
        if default is not _empty:
            if self._kind in (_VAR_POSITIONAL, _VAR_KEYWORD):
                msg = '{} parameters cannot have default values'.format(
                    self._kind.description
                )
                raise ValueError(msg)
        self._default = default
        self._annotation = annotation

        if name is _empty:
            raise ValueError('name is a required attribute for Parameter')

        if not isinstance(name, str):
            msg = 'name must be a str, not a {}'.format(type(name).__name__)
            raise TypeError(msg)

        if name and name[0] == '.' and name[1:].isdigit():
            if self._kind != _POSITIONAL_OR_KEYWORD:
                msg = (
                    'implicit arguments must be passed as '
                    'positional or keyword arguments, not {}'
                ).format(self._kind.description)
                raise ValueError(msg)
            self._kind = _POSITIONAL_ONLY
            name = 'implicit{}'.format(name[1:])

        is_kw = _iskeyword(name) and self._kind is not _POSITIONAL_ONLY
        if is_kw or not name.isidentifier():
            raise ValueError('{!r} is not a valid parameter name'.format(name))

        self._name = name

    def __reduce__(self):
        return (
            type(self),
            (self._name, self._kind),
            {'_default': self._default, '_annotation': self._annotation},
        )

    def __setstate__(self, state):
        self._default = state['_default']
        self._annotation = state['_annotation']

    @property
    def name(self):
        return self._name

    @property
    def default(self):
        return self._default

    @property
    def annotation(self):
        return self._annotation

    @property
    def kind(self):
        return self._kind

    def replace(self, *, name=_void, kind=_void, annotation=_void, default=_void):
        """Creates a customized copy of the Parameter."""
        if name is _void:
            name = self._name
        if kind is _void:
            kind = self._kind
        if annotation is _void:
            annotation = self._annotation
        if default is _void:
            default = self._default
        return type(self)(name, kind, default=default, annotation=annotation)

    def __str__(self):
        kind = self.kind
        formatted = self._name

        if self._annotation is not _empty:
            formatted = '{}: {}'.format(
                formatted, formatannotation(self._annotation)
            )

        if self._default is not _empty:
            if self._annotation is not _empty:
                formatted = '{} = {}'.format(formatted, repr(self._default))
            else:
                formatted = '{}={}'.format(formatted, repr(self._default))

        if kind == _VAR_POSITIONAL:
            formatted = '*' + formatted
        elif kind == _VAR_KEYWORD:
            formatted = '**' + formatted

        return formatted

    def __repr__(self):
        return '<{} "{}">'.format(self.__class__.__name__, self)

    def __hash__(self):
        return hash((self._name, self._kind, self._annotation, self._default))

    def __eq__(self, other):
        if self is other:
            return True
        if not isinstance(other, Parameter):
            return NotImplemented
        return (
            self._name == other._name
            and self._kind == other._kind
            and self._default == other._default
            and self._annotation == other._annotation
        )


class BoundArguments:
    """Result of `Signature.bind` call."""

    __slots__ = ('arguments', '_signature')

    def __init__(self, signature, arguments):
        self.arguments = arguments
        self._signature = signature

    @property
    def signature(self):
        return self._signature

    @property
    def args(self):
        args = []
        for param_name, param in self._signature.parameters.items():
            if param.kind in (_VAR_KEYWORD, _KEYWORD_ONLY):
                break
            try:
                arg = self.arguments[param_name]
            except KeyError:
                break
            else:
                if param.kind == _VAR_POSITIONAL:
                    args.extend(arg)
                else:
                    args.append(arg)
        return tuple(args)

    @property
    def kwargs(self):
        kwargs = {}
        kwargs_started = False
        for param_name, param in self._signature.parameters.items():
            if not kwargs_started:
                if param.kind in (_VAR_KEYWORD, _KEYWORD_ONLY):
                    kwargs_started = True
                else:
                    if param_name not in self.arguments:
                        kwargs_started = True
                        continue

            if not kwargs_started:
                continue

            try:
                arg = self.arguments[param_name]
            except KeyError:
                pass
            else:
                if param.kind == _VAR_KEYWORD:
                    kwargs.update(arg)
                else:
                    kwargs[param_name] = arg

        return kwargs

    def apply_defaults(self):
        """Set default values for missing arguments."""
        arguments = self.arguments
        new_arguments = []
        for name, param in self._signature.parameters.items():
            try:
                new_arguments.append((name, arguments[name]))
            except KeyError:
                if param.default is not _empty:
                    val = param.default
                elif param.kind is _VAR_POSITIONAL:
                    val = ()
                elif param.kind is _VAR_KEYWORD:
                    val = {}
                else:
                    continue
                new_arguments.append((name, val))
        self.arguments = dict(new_arguments)

    def __eq__(self, other):
        if self is other:
            return True
        if not isinstance(other, BoundArguments):
            return NotImplemented
        return (
            self.signature == other.signature
            and self.arguments == other.arguments
        )

    def __setstate__(self, state):
        self._signature = state['_signature']
        self.arguments = state['arguments']

    def __getstate__(self):
        return {'_signature': self._signature, 'arguments': self.arguments}

    def __repr__(self):
        args = []
        for arg, value in self.arguments.items():
            args.append('{}={!r}'.format(arg, value))
        return '<{} ({})>'.format(self.__class__.__name__, ', '.join(args))


class Signature:
    """A Signature object represents the overall signature of a function."""

    __slots__ = ('_return_annotation', '_parameters')

    _parameter_cls = Parameter
    _bound_arguments_cls = BoundArguments

    empty = _empty

    def __init__(self, parameters=None, *, return_annotation=_empty,
                 __validate_parameters__=True):
        if parameters is None:
            params = OrderedDict()
        else:
            if __validate_parameters__:
                params = OrderedDict()
                top_kind = _POSITIONAL_ONLY
                seen_default = False

                for param in parameters:
                    kind = param.kind
                    name = param.name

                    if kind < top_kind:
                        msg = (
                            'wrong parameter order: {} parameter before {} '
                            'parameter'
                        ).format(top_kind.description, kind.description)
                        raise ValueError(msg)
                    elif kind > top_kind:
                        top_kind = kind

                    if kind in (_POSITIONAL_ONLY, _POSITIONAL_OR_KEYWORD):
                        if param.default is _empty:
                            if seen_default:
                                raise ValueError(
                                    'non-default argument follows default argument'
                                )
                        else:
                            seen_default = True

                    if name in params:
                        raise ValueError(
                            'duplicate parameter name: {!r}'.format(name)
                        )

                    params[name] = param
            else:
                params = OrderedDict((param.name, param) for param in parameters)

        self._parameters = types.MappingProxyType(params)
        self._return_annotation = return_annotation

    @classmethod
    def from_callable(cls, obj, *, follow_wrapped=True, globals=None,
                      locals=None, eval_str=False):
        """Constructs Signature for the given callable object."""
        return _signature_from_callable(
            obj,
            sigcls=cls,
            follow_wrapper_chains=follow_wrapped,
            globals=globals,
            locals=locals,
            eval_str=eval_str,
        )

    @property
    def parameters(self):
        return self._parameters

    @property
    def return_annotation(self):
        return self._return_annotation

    def replace(self, *, parameters=_void, return_annotation=_void):
        """Creates a customized copy of the Signature."""
        if parameters is _void:
            parameters = self.parameters.values()
        if return_annotation is _void:
            return_annotation = self._return_annotation
        return type(self)(parameters, return_annotation=return_annotation)

    def _hash_basis(self):
        params = tuple(
            param for param in self.parameters.values()
            if param.kind != _KEYWORD_ONLY
        )
        kwo_params = {
            param.name: param for param in self.parameters.values()
            if param.kind == _KEYWORD_ONLY
        }
        return params, kwo_params, self.return_annotation

    def __hash__(self):
        params, kwo_params, return_annotation = self._hash_basis()
        kwo_params = frozenset(kwo_params.values())
        return hash((params, kwo_params, return_annotation))

    def __eq__(self, other):
        if self is other:
            return True
        if not isinstance(other, Signature):
            return NotImplemented
        return self._hash_basis() == other._hash_basis()

    def _bind(self, args, kwargs, *, partial=False):
        arguments = {}

        parameters = iter(self.parameters.values())
        parameters_ex = ()
        arg_vals = iter(args)

        while True:
            try:
                arg_val = next(arg_vals)
            except StopIteration:
                try:
                    param = next(parameters)
                except StopIteration:
                    break
                else:
                    if param.kind == _VAR_POSITIONAL:
                        break
                    elif param.name in kwargs:
                        if param.kind == _POSITIONAL_ONLY:
                            msg = (
                                '{arg!r} parameter is positional only, '
                                'but was passed as a keyword'
                            ).format(arg=param.name)
                            raise TypeError(msg) from None
                        parameters_ex = (param,)
                        break
                    elif param.kind == _VAR_KEYWORD or param.default is not _empty:
                        parameters_ex = (param,)
                        break
                    else:
                        if partial:
                            parameters_ex = (param,)
                            break
                        else:
                            if param.kind == _KEYWORD_ONLY:
                                argtype = ' keyword-only'
                            else:
                                argtype = ''
                            msg = 'missing a required{argtype} argument: {arg!r}'.format(
                                arg=param.name, argtype=argtype
                            )
                            raise TypeError(msg) from None
            else:
                try:
                    param = next(parameters)
                except StopIteration:
                    raise TypeError('too many positional arguments') from None
                else:
                    if param.kind in (_VAR_KEYWORD, _KEYWORD_ONLY):
                        raise TypeError('too many positional arguments') from None

                    if param.kind == _VAR_POSITIONAL:
                        values = [arg_val]
                        values.extend(arg_vals)
                        arguments[param.name] = tuple(values)
                        break

                    if param.name in kwargs and param.kind != _POSITIONAL_ONLY:
                        raise TypeError(
                            'multiple values for argument {arg!r}'.format(
                                arg=param.name
                            )
                        ) from None

                    arguments[param.name] = arg_val

        kwargs_param = None
        for param in itertools.chain(parameters_ex, parameters):
            if param.kind == _VAR_KEYWORD:
                kwargs_param = param
                continue

            if param.kind == _VAR_POSITIONAL:
                continue

            param_name = param.name
            try:
                arg_val = kwargs.pop(param_name)
            except KeyError:
                if (
                    not partial
                    and param.kind != _VAR_POSITIONAL
                    and param.default is _empty
                ):
                    raise TypeError(
                        'missing a required argument: {arg!r}'.format(
                            arg=param_name
                        )
                    ) from None
            else:
                if param.kind == _POSITIONAL_ONLY:
                    raise TypeError(
                        '{arg!r} parameter is positional only, '
                        'but was passed as a keyword'.format(arg=param.name)
                    )
                arguments[param_name] = arg_val

        if kwargs:
            if kwargs_param is not None:
                arguments[kwargs_param.name] = kwargs
            else:
                raise TypeError(
                    'got an unexpected keyword argument {arg!r}'.format(
                        arg=next(iter(kwargs))
                    )
                )

        return self._bound_arguments_cls(self, arguments)

    def bind(self, /, *args, **kwargs):
        """Get a BoundArguments object mapping args and kwargs to parameters."""
        return self._bind(args, kwargs)

    def bind_partial(self, /, *args, **kwargs):
        """Get a BoundArguments object partially mapping args and kwargs."""
        return self._bind(args, kwargs, partial=True)

    def __reduce__(self):
        return (
            type(self),
            (tuple(self._parameters.values()),),
            {'_return_annotation': self._return_annotation},
        )

    def __setstate__(self, state):
        self._return_annotation = state['_return_annotation']

    def __repr__(self):
        return '<{} {}>'.format(self.__class__.__name__, self)

    def __str__(self):
        result = []
        render_pos_only_separator = False
        render_kw_only_separator = True
        for param in self.parameters.values():
            formatted = str(param)
            kind = param.kind

            if kind == _POSITIONAL_ONLY:
                render_pos_only_separator = True
            elif render_pos_only_separator:
                result.append('/')
                render_pos_only_separator = False

            if kind == _VAR_POSITIONAL:
                render_kw_only_separator = False
            elif kind == _KEYWORD_ONLY and render_kw_only_separator:
                result.append('*')
                render_kw_only_separator = False

            result.append(formatted)

        if render_pos_only_separator:
            result.append('/')

        rendered = '({})'.format(', '.join(result))

        if self.return_annotation is not _empty:
            anno = formatannotation(self.return_annotation)
            rendered += ' -> {}'.format(anno)

        return rendered


def signature(obj, *, follow_wrapped=True, globals=None, locals=None, eval_str=False):
    """Get a signature object for the passed callable."""
    return Signature.from_callable(
        obj,
        follow_wrapped=follow_wrapped,
        globals=globals,
        locals=locals,
        eval_str=eval_str,
    )


class BufferFlags(enum.IntFlag):
    SIMPLE = 0x0
    WRITABLE = 0x1
    FORMAT = 0x4
    ND = 0x8
    STRIDES = 0x10 | 0x8
    C_CONTIGUOUS = 0x20 | 0x18
    F_CONTIGUOUS = 0x40 | 0x18
    ANY_CONTIGUOUS = 0x80 | 0x18
    INDIRECT = 0x100 | 0x18
    CONTIG = 0x8 | 0x1
    CONTIG_RO = 0x8
    STRIDED = 0x18 | 0x1
    STRIDED_RO = 0x18
    RECORDS = 0x18 | 0x1 | 0x4
    RECORDS_RO = 0x18 | 0x4
    FULL = 0x118 | 0x1 | 0x4
    FULL_RO = 0x118 | 0x4
    READ = 0x100
    WRITE = 0x200
