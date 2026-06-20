from typing import Final, ClassVar, Literal, Callable, List, Dict

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
print(repr(Callable[[None], None]))
print(repr(Callable[[list[None]], int]))

# A bare `None` argument: PEP 585 builtin aliases and `Literal` keep `None`;
# every other `typing.*` special form lowers it to `NoneType` at construction.
print(repr(list[None]))
print(repr(dict[str, None]))
print(repr(Literal[None]))
print(repr(Literal[1, None]))
print(repr(List[None]))
print(repr(Dict[str, None]))
print(repr(Final[None]))
