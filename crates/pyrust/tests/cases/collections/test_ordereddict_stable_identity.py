# `collections.OrderedDict` is a native immutable type in CPython. PyRust
# models its methods in Python source, but its runtime identity and mutability
# contract must remain native and must not depend on writable metadata.

from collections import OrderedDict as NativeOrderedDict


def mutate(label, action):
    try:
        action()
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))
    else:
        print(label, "accepted")


mutate(
    "set module",
    lambda: setattr(NativeOrderedDict, "__module__", "spoofed"),
)
mutate(
    "set name",
    lambda: setattr(NativeOrderedDict, "__name__", "Spoofed"),
)
mutate("set custom", lambda: setattr(NativeOrderedDict, "custom", 1))
mutate(
    "delete module",
    lambda: delattr(NativeOrderedDict, "__module__"),
)


# Proper Python subclasses remain mutable.
class Child(NativeOrderedDict):
    pass


mutate("child custom", lambda: setattr(Child, "custom", 1))
print("child custom value:", Child.custom)


# A user class can spoof the public name/module but not the registered runtime
# identity. It remains an ordinary mutable dict subclass with dict guard
# semantics.
class OrderedDict(dict):
    pass


OrderedDict.__module__ = "collections"
mutate("lookalike custom", lambda: setattr(OrderedDict, "custom", 2))
print("lookalike custom value:", OrderedDict.custom)

lookalike = OrderedDict((("a", 1), ("b", 2)))
lookalike_iterator = iter(lookalike)
print("lookalike first:", next(lookalike_iterator))
lookalike["c"] = 3
try:
    next(lookalike_iterator)
except RuntimeError as exc:
    print("lookalike guard:", str(exc))


# Failed metadata mutation must not alter OrderedDict-specific iterator policy.
ordered = NativeOrderedDict((("a", 1), ("b", 2)))
iterator = iter(ordered)
print("first:", next(iterator))
ordered["c"] = 3
try:
    next(iterator)
except RuntimeError as exc:
    print("guard:", str(exc))
