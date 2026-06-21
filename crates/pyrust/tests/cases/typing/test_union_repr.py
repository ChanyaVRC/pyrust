# Issue #2720: bare special-form sentinels repr as `typing.<name>`, not the
# default `<class 'typing.Union'>` form, and subscripted aliases keep the
# `typing.` prefix.
import typing

# Bare special forms.
print(repr(typing.Union))
print(repr(typing.Optional))
print(repr(typing.Final))
print(repr(typing.ClassVar))
print(repr(typing.Literal))
print(repr(typing.Callable))

# Subscripted Union/Optional aliases.
print(repr(typing.Union[int, str]))
print(repr(typing.Optional[int]))
print(repr(typing.Union[int, str, None]))
print(str(typing.Union[int, str]))

# Other subscripted special forms.
print(repr(typing.Final[int]))
print(repr(typing.ClassVar[int]))

# Single-type / collapsing unions are not aliases at all.
print(repr(typing.Union[int]))
print(repr(typing.Union[int, int]))
print(repr(typing.Optional[None]))

# PEP 604 `X | Y` builds a types.UnionType, repr'd without the `typing.` prefix.
print(repr(int | str))
print(repr(int | None))
print(repr(int | str | None))
