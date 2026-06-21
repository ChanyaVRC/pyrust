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
