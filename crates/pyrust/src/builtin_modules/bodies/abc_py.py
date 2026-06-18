"""
Abstract Base Classes (pyrust port of CPython 3.12's ``abc`` module).

A minimal but faithful implementation built on a Python-level metaclass.
``ABCMeta.__new__`` collects the names of every method/attribute flagged
``__isabstractmethod__`` (set by the ``@abstractmethod`` decorator) — both from
the class body and from any base whose abstract methods remain unoverridden —
into ``cls.__abstractmethods__`` (a ``frozenset``).  Instantiating a class
whose ``__abstractmethods__`` is non-empty raises ``TypeError``, mirroring
CPython.

This source is exec'd once into a throwaway namespace at first ``import abc``;
the public names are copied onto the module by
``abc.rs::inject_python_members`` (mirrors ``operator`` / ``string``).

Reference: <https://docs.python.org/3/library/abc.html>
"""


def abstractmethod(funcobj):
    """A decorator indicating abstract methods.

    Requires that the metaclass is ABCMeta or derived from it.  A class that
    has a metaclass derived from ABCMeta cannot be instantiated unless all of
    its abstract methods are overridden.
    """
    funcobj.__isabstractmethod__ = True
    return funcobj


def abstractclassmethod(callable):
    """A decorator indicating abstract classmethods.

    Deprecated since Python 3.3: use ``classmethod`` with ``abstractmethod``.
    """
    callable.__isabstractmethod__ = True
    return classmethod(callable)


def abstractstaticmethod(callable):
    """A decorator indicating abstract staticmethods.

    Deprecated since Python 3.3: use ``staticmethod`` with ``abstractmethod``.
    """
    callable.__isabstractmethod__ = True
    return staticmethod(callable)


def abstractproperty(fget):
    """A decorator indicating abstract properties.

    Deprecated since Python 3.3: use ``property`` with ``abstractmethod``.
    """
    p = property(fget)
    return p


# A monotonically increasing token bumped whenever the ABC virtual-subclass
# graph could have changed.  CPython exposes this so caches keyed on
# isinstance / issubclass results can be invalidated.
_abc_invalidation_counter = 0


class ABCMeta(type):
    """Metaclass for defining Abstract Base Classes (ABCs).

    Use this metaclass to create an ABC.  An ABC can be subclassed directly,
    and then acts as a mix-in class.
    """

    def __new__(mcls, name, bases, namespace, **kwargs):
        cls = super().__new__(mcls, name, bases, namespace, **kwargs)
        # Compute the set of abstract method names.
        abstracts = set()
        # Names flagged abstract in this class body.
        for key, value in namespace.items():
            if getattr(value, "__isabstractmethod__", False):
                abstracts.add(key)
        # Inherited abstract methods that have not been overridden with a
        # concrete implementation.
        for base in bases:
            for key in getattr(base, "__abstractmethods__", set()):
                value = getattr(cls, key, None)
                if getattr(value, "__isabstractmethod__", False):
                    abstracts.add(key)
        cls.__abstractmethods__ = frozenset(abstracts)
        # Per-class registry of virtual subclasses registered via .register().
        cls._abc_registry = set()
        return cls

    def __call__(cls, *args, **kwargs):
        # CPython enforces this in ``object.__new__`` / ``type.__call__``; we
        # reproduce it on the metaclass so an abstract class can't be
        # instantiated.  Error wording matches CPython 3.12.
        abstracts = getattr(cls, "__abstractmethods__", None)
        if abstracts:
            names = sorted(abstracts)
            if len(names) == 1:
                methods = "method " + repr(names[0])
            else:
                methods = "methods " + ", ".join(repr(n) for n in names)
            raise TypeError(
                "Can't instantiate abstract class %s without an implementation "
                "for abstract %s" % (cls.__name__, methods))
        return super().__call__(*args, **kwargs)

    def register(cls, subclass):
        """Register a virtual subclass of an ABC.

        Returns the subclass, to allow usage as a class decorator.
        """
        if not isinstance(subclass, type):
            raise TypeError("Can only register classes")
        if issubclass(subclass, cls):
            return subclass  # Already a subclass.
        cls._abc_registry.add(subclass)
        global _abc_invalidation_counter
        _abc_invalidation_counter += 1
        return subclass

    def __instancecheck__(cls, instance):
        """Override for isinstance(instance, cls)."""
        return cls.__subclasscheck__(type(instance))

    def __subclasscheck__(cls, subclass):
        """Override for issubclass(subclass, cls)."""
        if not isinstance(subclass, type):
            raise TypeError("issubclass() arg 1 must be a class")
        # Real subclass via the normal MRO.
        for klass in subclass.__mro__:
            if klass is cls:
                return True
        # Virtual subclasses registered on cls or any class in cls's MRO.
        for klass in cls.__mro__:
            registry = getattr(klass, "_abc_registry", None)
            if registry:
                for registered in registry:
                    if issubclass(subclass, registered):
                        return True
        return False


class ABC(metaclass=ABCMeta):
    """Helper class that provides a standard way to create an ABC using
    inheritance.
    """
    __slots__ = ()


def get_cache_token():
    """Returns the current ABC cache token.

    The token is an opaque object (supporting equality testing) identifying the
    current version of the ABC cache for virtual subclasses.  The token changes
    with every call to ``register()`` on any ABC.
    """
    return _abc_invalidation_counter


def update_abstractmethods(cls):
    """Recalculate the set of abstract methods of an abstract class.

    Should be called if a class's abstract methods have been implemented or
    changed after it was created.  Returns ``cls`` for use as a class
    decorator.
    """
    if not hasattr(cls, "__abstractmethods__"):
        # Not an ABC; nothing to do.
        return cls
    abstracts = set()
    for base in cls.__bases__:
        for key in getattr(base, "__abstractmethods__", set()):
            value = getattr(cls, key, None)
            if getattr(value, "__isabstractmethod__", False):
                abstracts.add(key)
    for key, value in cls.__dict__.items():
        if getattr(value, "__isabstractmethod__", False):
            abstracts.add(key)
    cls.__abstractmethods__ = frozenset(abstracts)
    return cls
