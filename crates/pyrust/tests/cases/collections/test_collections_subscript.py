# Parity fixture for issue #2603.
#
# The `collections` container classes define `__class_getitem__` (PEP 585), so
# subscripting them at runtime produces a `types.GenericAlias` whose repr is
# `collections.<ClassName>[args]`.  This is used by runtime-evaluated type
# annotations like `x: collections.defaultdict[str, list]`.
#
# Exercised: single- and multi-arg subscript, repr, the explicit
# `Cls.__class_getitem__(arg)` call form, and the GenericAlias attributes
# (`__origin__`, `__args__`) plus `typing.get_args`.
import collections
from typing import get_args

names = [
    "OrderedDict",
    "defaultdict",
    "Counter",
    "deque",
    "ChainMap",
    "UserDict",
    "UserList",
    "UserString",
]

# Single type argument.
for n in names:
    cls = getattr(collections, n)
    print(n, repr(cls[int]))

# Multiple type arguments.
print(repr(collections.OrderedDict[str, int]))
print(repr(collections.defaultdict[str, list]))
print(repr(collections.ChainMap[str, int]))

# Nested aliases.
print(repr(collections.deque[collections.Counter[str]]))

# Explicit __class_getitem__ call form.
print(repr(collections.deque.__class_getitem__(int)))
print(repr(collections.Counter.__class_getitem__(str)))

# GenericAlias attributes and typing.get_args.
alias = collections.OrderedDict[str, int]
print(alias.__origin__ is collections.OrderedDict)
print(alias.__args__)
print(get_args(alias))

# Aliases are equal and hashable.
print(collections.deque[int] == collections.deque[int])
print(collections.deque[int] == collections.deque[str])
print({collections.deque[int]: 1}[collections.deque[int]])
