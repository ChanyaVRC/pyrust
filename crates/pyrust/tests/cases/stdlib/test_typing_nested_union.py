# typing.Union / typing.Optional flatten + normalisation (issue #2524).
#
# `Optional[X]` is `Union[X, None]`; nested unions are flattened, `None` is
# lowered to `NoneType`, duplicate members are dropped (first-seen order), and
# a single-member union collapses to that member.  The repr reconstructs the
# `typing.Optional[X]` spelling for the two-arg `X | None` case.
from typing import Optional, Union, get_args, get_origin

# Nested flatten.
print(get_args(Optional[Optional[int]]))
print(get_args(Union[int, Union[str, float]]))
print(get_args(Optional[Optional[Optional[int]]]))
print(get_args(Union[int, Optional[str]]))

# Dedup, preserving order.
print(get_args(Union[int, int]))
print(get_args(Union[int, str, int, str]))

# Optional <-> Union[X, None] equivalence.
print(get_args(Optional[int]))
print(get_args(Union[int, None]))
print(get_args(Union[None, int]))

# Plain two/three-member unions are unchanged.
print(get_args(Union[int, str]))
print(get_args(Union[str, int, None]))

# get_origin normalises Optional/Union to Union.
print(get_origin(Optional[int]) is Union)
print(get_origin(Union[int, str]) is Union)
print(get_origin(int))

# repr: typing. prefix, Optional collapse for X | None.
print(repr(Optional[Optional[int]]))
print(repr(Optional[int]))
print(repr(Union[int, None]))
print(repr(Union[None, int]))
print(repr(Union[int, str]))
print(repr(Union[str, int, None]))
print(repr(Union[int, Optional[str]]))

# Single-member union collapses to the member itself.
print(Union[int])
print(type(Union[int]).__name__)
print(Optional[None])
print(Union[None])

# Equality / hashing of flattened aliases.
print(Union[int, str] == Union[int, str])
print(Optional[int] == Union[int, None])
print({Optional[int]: "ok"}[Union[int, None]])

# Union equality/hashing is order-insensitive (CPython compares the args as a
# frozenset), so reordered members compare equal and hash the same.
print(Union[int, str] == Union[str, int])
print(Union[int, str, float] == Union[float, int, str])
print(hash(Union[int, str]) == hash(Union[str, int]))
print({Union[int, str]: "ok"}[Union[str, int]])
# A proper subset is still unequal.
print(Union[int, str] == Union[int, str, float])
# Order matters for non-Union generic aliases (dict args are positional).
print(dict[str, int] == dict[int, str])
print(list[Union[int, str]] == list[Union[str, int]])

# Error cases.
try:
    Optional[int, str]
except TypeError as e:
    print("TypeError:", e)

try:
    Union[()]
except TypeError as e:
    print("TypeError:", e)
