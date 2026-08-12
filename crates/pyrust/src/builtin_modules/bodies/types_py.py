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
