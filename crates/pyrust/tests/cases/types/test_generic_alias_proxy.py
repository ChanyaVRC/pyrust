# Issue #2132: a GenericAlias renders an `Ellipsis` argument as `...`
# (matching CPython's `ga_repr_item`), not as `Ellipsis`.
print(str(tuple[int, ...]))
print(repr(tuple[int, ...]))
print(str(list[...]))
print(str(dict[str, ...]))
print(str(tuple[tuple[int, ...], str]))
print(repr(list[int]))
print(repr(dict[str, int]))
print(repr(list[dict[str, int]]))

# An empty args tuple (`tuple[()]`, __args__ == ()) reprs as `tuple[()]`,
# not `tuple[]` — CPython's `ga_repr` writes `()` for the zero-arg case.
print(repr(tuple[()]))
print(repr(list[tuple[()]]))
print(repr(tuple[tuple[()], str]))

# Bare / in-container Ellipsis is unaffected (still reprs as `Ellipsis`).
print(repr(...))
print(repr([1, ..., 2]))
print(repr((1, ...)))

# Non-Ellipsis args are unchanged.
print(repr(tuple[int, str]))

# Issue #2133: a GenericAlias is callable and delegates to its origin,
# constructing a plain (unparameterized) instance.
print(list[int]([1, 2, 3]))
print(list[int]())
print(dict[str, int]([("a", 1)]))
print(dict[str, int](a=1, b=2))
print(tuple[int, ...]([1, 2, 3]))
print(type(list[int]([1, 2, 3])) is list)
print(type(dict[str, int]([("a", 1)])) is dict)

# Issue #2133: attribute access proxies to the origin, while the alias keeps
# its own __origin__ / __args__ / __parameters__.
print(list[int].__name__)
print(list[int].__qualname__)
print(list[int].__mro__)
print(dict[str, int].__name__)
print(list[int].__origin__ is list)
print(list[int].__args__ == (int,))
print(list[int].__parameters__)
print(dict[str, int].__parameters__)

# Instance methods resolved through the alias operate on the constructed
# origin instance.
x = list[int]([3, 1, 2])
x.append(4)
x.sort()
print(x)

# isinstance with a parameterized generic raises TypeError (subscripted
# generics aren't usable as the second isinstance argument).
try:
    isinstance([1, 2], list[int])
    print("no error")
except TypeError:
    print("TypeError")
