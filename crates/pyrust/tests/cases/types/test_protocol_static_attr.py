"""Protocol isinstance uses static attribute lookup, not dynamic getattr (#2551).

CPython's ``_ProtocolMeta.__instancecheck__`` resolves each protocol member with
``inspect.getattr_static`` semantics: it scans the instance ``__dict__`` and the
type's MRO dicts directly, never invoking ``__getattr__`` or descriptor
``__get__``.  So a member supplied only via ``__getattr__`` does not count, and a
``__getattr__`` that raises a non-``AttributeError`` is treated as "absent"
rather than aborting the check.
"""

from typing import Protocol, runtime_checkable


@runtime_checkable
class HasFoo(Protocol):
    def foo(self): ...


class DynAttr:
    """Supplies ``foo`` only dynamically — static lookup must miss it."""

    def __getattr__(self, name):
        if name == "foo":
            return lambda: None
        raise AttributeError(name)


class BadGetattr:
    """A raising ``__getattr__`` must not abort the check."""

    def __getattr__(self, name):
        raise TypeError("dynamic attrs broken")


class RealFoo:
    def foo(self):
        return 1


class DictFoo:
    pass


# __getattr__-supplied member does not satisfy the protocol.
print(isinstance(DynAttr(), HasFoo))  # False
# A raising __getattr__ is treated as "member absent", not propagated.
print(isinstance(BadGetattr(), HasFoo))  # False
# A genuine class-body method still matches.
print(isinstance(RealFoo(), HasFoo))  # True

# A member living in the instance __dict__ (not the class) still matches:
# static lookup consults the instance dict before the class MRO.
d = DictFoo()
d.foo = lambda: None
print(isinstance(d, HasFoo))  # True


# Built-in types resolve their slot dunders via the type MRO, not __getattr__.
@runtime_checkable
class Sized(Protocol):
    def __len__(self): ...


print(isinstance([1, 2], Sized))  # True
print(isinstance(42, Sized))  # False


# A descriptor whose __get__ would raise must not break the check: static lookup
# never invokes __get__, so the attribute is still "present".
class Boom:
    def __get__(self, obj, objtype=None):
        raise RuntimeError("descriptor exploded")


@runtime_checkable
class HasBar(Protocol):
    def bar(self): ...


class WithDescriptor:
    bar = Boom()


print(isinstance(WithDescriptor(), HasBar))  # True
