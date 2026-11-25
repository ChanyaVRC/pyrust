"""Helper to provide extensibility for pickle.

This is only useful to add pickle support for extension types defined in
C, not for instances of user-defined classes.
"""

__all__ = [
    "pickle",
    "constructor",
    "add_extension",
    "remove_extension",
    "clear_extension_cache",
]

dispatch_table = {}


def pickle(ob_type, pickle_function, constructor_ob=None):
    if not callable(pickle_function):
        raise TypeError("reduction functions must be callable")
    dispatch_table[ob_type] = pickle_function

    # The constructor_ob function is a vestige of safe for unpickling.
    # There is no reason for the caller to pass it anymore.
    if constructor_ob is not None:
        constructor(constructor_ob)


def constructor(object):
    if not callable(object):
        raise TypeError("constructors must be callable")


# Example: provide pickling support for complex numbers.


def pickle_complex(c):
    return complex, (c.real, c.imag)


pickle(complex, pickle_complex, complex)


def pickle_union(obj):
    import functools
    import operator

    return functools.reduce, (operator.or_, obj.__args__)


pickle(type(int | str), pickle_union)


# Support for pickling new-style objects

_BUILTIN_BASES = (
    int,
    float,
    complex,
    str,
    bytes,
    bytearray,
    tuple,
    list,
    dict,
    set,
    frozenset,
)


def _reconstructor(cls, base, state):
    if base is object:
        obj = object.__new__(cls)
    elif base in _BUILTIN_BASES:
        obj = _native_reconstructor(cls, base, state)
    else:
        obj = base.__new__(cls, state)
        if base.__init__ != object.__init__:
            base.__init__(obj, state)
    return obj


_HEAPTYPE = 1 << 9
_new_type = type(int.__new__)


# Python code for object.__reduce_ex__ for protocols 0 and 1


def _reduce_ex(self, proto):
    assert proto < 2
    cls = self.__class__
    for base in cls.__mro__:
        if hasattr(base, "__flags__") and not base.__flags__ & _HEAPTYPE:
            break
        new = base.__new__
        if isinstance(new, _new_type) and getattr(new, "__self__", None) is base:
            break
    else:
        base = object  # not really reachable
    if base is object:
        state = None
    else:
        if base is cls:
            raise TypeError(f"cannot pickle {cls.__name__!r} object")
        state = base(self)
    args = (cls, base, state)
    try:
        getstate = self.__getstate__
    except AttributeError:
        if getattr(self, "__slots__", None):
            raise TypeError(
                f"cannot pickle {cls.__name__!r} object: "
                f"a class that defines __slots__ without "
                f"defining __getstate__ cannot be pickled "
                f"with protocol {proto}"
            ) from None
        try:
            dct = self.__dict__
        except AttributeError:
            dct = None
    else:
        if type(self).__getstate__ is object.__getstate__ and getattr(
            self, "__slots__", None
        ):
            raise TypeError(
                "a class that defines __slots__ without "
                "defining __getstate__ cannot be pickled"
            )
        dct = getstate()
    if dct:
        return _reconstructor, args, dct
    else:
        return _reconstructor, args


# Helper for __reduce_ex__ protocol 2


def __newobj__(cls, *args):
    return _native_newobj(cls, *args)


def __newobj_ex__(cls, args, kwargs):
    """Used by pickle protocol 4, instead of __newobj__ to allow classes with
    keyword-only arguments to be pickled correctly.
    """
    if not kwargs:
        return _native_newobj(cls, *args)
    return cls.__new__(cls, *args, **kwargs)


def _slotnames(cls):
    """Return a list of slot names for a given class.

    This needs to find slots defined by the class and its bases, so we
    can't simply return the __slots__ attribute.  We must walk down
    the Method Resolution Order and concatenate the __slots__ of each
    class found there.
    """
    # Get the value from a cache in the class if possible
    names = cls.__dict__.get("__slotnames__")
    if names is not None:
        return names

    # Not cached -- calculate the value
    names = []
    if not hasattr(cls, "__slots__"):
        # This class has no slots
        pass
    else:
        # Slots found -- gather slot names from all base classes
        for c in cls.__mro__:
            if "__slots__" in c.__dict__:
                slots = c.__dict__["__slots__"]
                # if class has a single slot, it can be given as a string
                if isinstance(slots, str):
                    slots = (slots,)
                for name in slots:
                    # special descriptors
                    if name in ("__dict__", "__weakref__"):
                        continue
                    # mangled names
                    elif name.startswith("__") and not name.endswith("__"):
                        stripped = c.__name__.lstrip("_")
                        if stripped:
                            names.append("_%s%s" % (stripped, name))
                        else:
                            names.append(name)
                    else:
                        names.append(name)

    # Cache the outcome in the class if at all possible
    try:
        cls.__slotnames__ = names
    except Exception:
        pass  # But don't die if we can't

    return names


# A registry of extension codes.  Codes are positive ints; 0 is reserved.

_extension_registry = {}  # key -> code
_inverted_registry = {}  # code -> key
_extension_cache = {}  # code -> object


def add_extension(module, name, code):
    """Register an extension code."""
    code = int(code)
    if not 1 <= code <= 0x7FFFFFFF:
        raise ValueError("code out of range")
    key = (module, name)
    if (
        _extension_registry.get(key) == code
        and _inverted_registry.get(code) == key
    ):
        return  # Redundant registrations are benign
    if key in _extension_registry:
        raise ValueError(
            "key %s is already registered with code %s"
            % (key, _extension_registry[key])
        )
    if code in _inverted_registry:
        raise ValueError(
            "code %s is already in use for key %s"
            % (code, _inverted_registry[code])
        )
    _extension_registry[key] = code
    _inverted_registry[code] = key


def remove_extension(module, name, code):
    """Unregister an extension code.  For testing only."""
    key = (module, name)
    if (
        _extension_registry.get(key) != code
        or _inverted_registry.get(code) != key
    ):
        raise ValueError(
            "key %s is not registered with code %s" % (key, code)
        )
    del _extension_registry[key]
    del _inverted_registry[code]
    if code in _extension_cache:
        del _extension_cache[code]


def clear_extension_cache():
    _extension_cache.clear()
