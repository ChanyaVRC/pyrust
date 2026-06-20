from typing import Final, ClassVar, Literal, Callable

# PEP 585: `type` is subscriptable (issue #2723).
print(type[int])
print(type[str])

# Special-form subscript reprs carry the `typing.` prefix (issue #2724).
print(repr(Final[int]))
print(repr(ClassVar[str]))
print(repr(Literal[1, 2, 3]))

# `Callable` argument-list reprs use type names, not `<class 'int'>`, and a
# bare `None` return lowers to `NoneType`.
print(repr(Callable[[int], str]))
print(repr(Callable[[], None]))
print(repr(Callable[[int, str], bool]))
print(repr(Callable[..., int]))
