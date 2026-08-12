# Python-source members for the `types` module (injected by the `@inject`
# post-load hook, mirroring `operator` / `string`).
#
# `SimpleNamespace` is most naturally expressed as a plain Python class: it
# stores its keyword arguments in the instance `__dict__`, reprs them as
# `namespace(k=v, ...)`, and compares by `__dict__`.  Defining `__eq__` makes
# instances unhashable (Python sets `__hash__ = None`), matching CPython's
# `types.SimpleNamespace`.
#
# The native module body (`types.rs`) supplies the type-object constants
# (`NoneType`, `FunctionType`, `MappingProxyType`, …) that need access to the
# interpreter's internal singletons.

__all__ = [
    'FunctionType', 'LambdaType', 'CodeType', 'MappingProxyType',
    'SimpleNamespace', 'CellType', 'GeneratorType', 'CoroutineType',
    'AsyncGeneratorType', 'MethodType', 'BuiltinFunctionType',
    'BuiltinMethodType', 'WrapperDescriptorType', 'MethodWrapperType',
    'MethodDescriptorType', 'ClassMethodDescriptorType', 'ModuleType',
    'TracebackType', 'FrameType', 'GetSetDescriptorType', 'MemberDescriptorType',
    'new_class', 'resolve_bases', 'prepare_class', 'get_original_bases',
    'DynamicClassAttribute', 'coroutine', 'GenericAlias', 'UnionType',
    'EllipsisType', 'NoneType', 'NotImplementedType'
]

def coroutine(func):
    """Convert a regular generator function to a coroutine."""
    if not callable(func):
        raise TypeError("types.coroutine() expects a callable")

    if type(func) is FunctionType and type(getattr(func, "__code__", None)) is CodeType:
        flags = func.__code__.co_flags
        if flags & 0x180:
            return func
        if flags & 0x20:
            _mark_iterable_coroutine(func)
            return func

    # Keep these imports off the module import path, matching CPython. They are
    # needed only for callables whose result may require the protocol wrapper.
    import functools
    from collections.abc import Coroutine, Generator

    @functools.wraps(func)
    def wrapped(*args, **kwargs):
        coro = func(*args, **kwargs)
        if type(coro) is CoroutineType or (
            type(coro) is GeneratorType and _is_iterable_coroutine(coro)
        ):
            return coro
        if (
            isinstance(coro, Generator)
            and not isinstance(coro, Coroutine)
            and _is_generator_wrapper_candidate(coro)
        ):
            return _GeneratorWrapper(coro)
        return coro

    return wrapped


class _GeneratorWrapper:
    def __init__(self, gen):
        self.__wrapped = gen
        self.__isgen = type(gen) is GeneratorType
        self.__name__ = getattr(gen, "__name__", None)
        self.__qualname__ = getattr(gen, "__qualname__", None)

    def send(self, val):
        return self.__wrapped.send(val)

    def throw(self, typ, *rest):
        return self.__wrapped.throw(typ, *rest)

    def close(self):
        return self.__wrapped.close()

    @property
    def gi_code(self):
        return self.__wrapped.gi_code

    @property
    def gi_frame(self):
        return self.__wrapped.gi_frame

    @property
    def gi_running(self):
        return self.__wrapped.gi_running

    @property
    def gi_yieldfrom(self):
        return self.__wrapped.gi_yieldfrom

    cr_code = gi_code
    cr_frame = gi_frame
    cr_running = gi_running
    cr_await = gi_yieldfrom

    def __next__(self):
        return next(self.__wrapped)

    def __iter__(self):
        if self.__isgen:
            return self.__wrapped
        return self

    __await__ = __iter__


def new_class(name, bases=(), kwds=None, exec_body=None):
    """Create a class object dynamically using the appropriate metaclass."""
    resolved_bases = resolve_bases(bases)
    meta, namespace, kwds = prepare_class(name, resolved_bases, kwds)
    if exec_body is not None:
        exec_body(namespace)
    if resolved_bases is not bases:
        namespace["__orig_bases__"] = bases
    return meta(name, resolved_bases, namespace, **kwds)


def resolve_bases(bases):
    """Resolve MRO entries dynamically as specified by PEP 560."""
    new_bases = list(bases)
    updated = False
    shift = 0
    for index, base in enumerate(bases):
        if isinstance(base, type):
            continue
        if not hasattr(base, "__mro_entries__"):
            continue
        replacement = base.__mro_entries__(bases)
        updated = True
        if not isinstance(replacement, tuple):
            raise TypeError("__mro_entries__ must return a tuple")
        new_bases[index + shift:index + shift + 1] = replacement
        shift += len(replacement) - 1
    if not updated:
        return bases
    return tuple(new_bases)


def prepare_class(name, bases=(), kwds=None):
    """Call the __prepare__ method of the appropriate metaclass."""
    if kwds is None:
        kwds = {}
    else:
        kwds = dict(kwds)
    if "metaclass" in kwds:
        meta = kwds.pop("metaclass")
    elif bases:
        meta = type(bases[0])
    else:
        meta = type
    if isinstance(meta, type):
        meta = _calculate_meta(meta, bases)
    if hasattr(meta, "__prepare__"):
        namespace = meta.__prepare__(name, bases, **kwds)
    else:
        namespace = {}
    return meta, namespace, kwds


def _calculate_meta(meta, bases):
    """Calculate the most derived metaclass."""
    winner = meta
    for base in bases:
        base_meta = type(base)
        if issubclass(winner, base_meta):
            continue
        if issubclass(base_meta, winner):
            winner = base_meta
            continue
        raise TypeError(
            "metaclass conflict: the metaclass of a derived class "
            "must be a (non-strict) subclass of the metaclasses of all its bases"
        )
    return winner


def get_original_bases(cls, /):
    """Return the bases before any PEP 560 `__mro_entries__` substitution."""
    try:
        return cls.__dict__.get("__orig_bases__", cls.__bases__)
    except AttributeError:
        raise TypeError(
            f"Expected an instance of type, not {type(cls).__name__!r}"
        ) from None


class DynamicClassAttribute:
    """Route class access to `__getattr__` while retaining instance access."""

    def __init__(self, fget=None, fset=None, fdel=None, doc=None):
        self.fget = fget
        self.fset = fset
        self.fdel = fdel
        self.__doc__ = doc or fget.__doc__
        self.overwrite_doc = doc is None
        self.__isabstractmethod__ = bool(
            getattr(fget, "__isabstractmethod__", False)
        )

    def __get__(self, instance, ownerclass=None):
        if instance is None:
            if self.__isabstractmethod__:
                return self
            raise AttributeError()
        if self.fget is None:
            raise AttributeError("unreadable attribute")
        return self.fget(instance)

    def __set__(self, instance, value):
        if self.fset is None:
            raise AttributeError("can't set attribute")
        self.fset(instance, value)

    def __delete__(self, instance):
        if self.fdel is None:
            raise AttributeError("can't delete attribute")
        self.fdel(instance)

    def getter(self, fget):
        doc = fget.__doc__ if self.overwrite_doc else None
        result = type(self)(fget, self.fset, self.fdel, doc or self.__doc__)
        result.overwrite_doc = self.overwrite_doc
        return result

    def setter(self, fset):
        result = type(self)(self.fget, fset, self.fdel, self.__doc__)
        result.overwrite_doc = self.overwrite_doc
        return result

    def deleter(self, fdel):
        result = type(self)(self.fget, self.fset, fdel, self.__doc__)
        result.overwrite_doc = self.overwrite_doc
        return result


class SimpleNamespace:
    """A simple attribute-based namespace.

    SimpleNamespace(**kwargs)
    """

    def __init__(self, /, **kwargs):
        self.__dict__.update(kwargs)

    def __repr__(self):
        items = (f"{k}={v!r}" for k, v in self.__dict__.items())
        # CPython prefixes with the subclass name; the base class itself reprs
        # as `namespace(...)` rather than `SimpleNamespace(...)`.
        cls = type(self)
        name = "namespace" if cls is SimpleNamespace else cls.__name__
        return "{}({})".format(name, ", ".join(items))

    def __eq__(self, other):
        if isinstance(self, SimpleNamespace) and isinstance(other, SimpleNamespace):
            return self.__dict__ == other.__dict__
        return NotImplemented
