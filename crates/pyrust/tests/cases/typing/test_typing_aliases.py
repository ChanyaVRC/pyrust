import typing
from typing import List, Dict, Tuple, Set, FrozenSet, Type

# Bare aliases repr with a `typing.` prefix (not `<class 'list'>`).
print(repr(typing.List))
print(repr(typing.Dict))
print(repr(typing.Tuple))
print(repr(typing.Set))
print(repr(typing.FrozenSet))
print(repr(typing.Type))

# Subscripted aliases keep the `typing.` prefix.
print(repr(List[int]))
print(repr(Dict[str, int]))
print(repr(Tuple[int, str]))
print(repr(Set[int]))
print(repr(FrozenSet[int]))
print(repr(Type[int]))

# Distinct identity from the underlying builtin.
print(typing.List is list)
print(typing.Dict is dict)
print(typing.Tuple is tuple)
print(typing.Set is set)
print(typing.FrozenSet is frozenset)

# isinstance delegates to the underlying builtin.
print(isinstance([], typing.List))
print(isinstance({}, typing.Dict))
print(isinstance((), typing.Tuple))
print(isinstance(set(), typing.Set))
print(isinstance(frozenset(), typing.FrozenSet))
print(isinstance(int, typing.Type))
print(isinstance([], typing.Dict))

# issubclass delegates to the underlying builtin.
print(issubclass(list, typing.List))
print(issubclass(dict, typing.Dict))
print(issubclass(tuple, typing.Tuple))
print(issubclass(set, typing.Set))
print(issubclass(frozenset, typing.FrozenSet))
print(issubclass(bool, typing.Type))
print(issubclass(dict, typing.List))
