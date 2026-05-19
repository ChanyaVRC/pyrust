# PEP 585: built-in collection types support `list[int]`-style subscripting,
# returning a `types.GenericAlias` value.

# Basic repr output
print(repr(list[int]))
print(repr(dict[str, int]))
print(repr(tuple[int, str]))
print(repr(set[float]))
print(repr(frozenset[bytes]))

# Nested generic alias
print(repr(dict[str, list[int]]))

# __origin__ and __args__ attributes
ga = list[int]
print(ga.__origin__ is list)
print(ga.__args__ == (int,))

ga2 = dict[str, int]
print(ga2.__origin__ is dict)
print(ga2.__args__ == (str, int))

# Direct __class_getitem__ call
print(repr(list.__class_getitem__(int)))

# hasattr checks: only collection types have __class_getitem__
print(hasattr(list, "__class_getitem__"))
print(hasattr(dict, "__class_getitem__"))
print(hasattr(tuple, "__class_getitem__"))
print(hasattr(set, "__class_getitem__"))
print(hasattr(frozenset, "__class_getitem__"))
print(hasattr(int, "__class_getitem__"))
print(hasattr(str, "__class_getitem__"))
print(hasattr(bytes, "__class_getitem__"))

# type(ga).__name__ == "GenericAlias"
print(type(list[int]).__name__)

# Equality: two aliases with the same origin and args must be equal
print(list[int] == list[int])
print(list[int] == list[str])
print(list[int] == dict[str, int])

# Hashing: GenericAlias is hashable and consistent with equality
print(hash(list[int]) == hash(list[int]))
print(hash(list[int]) == hash(list[str]))

# Usable as dict key and in set
d = {list[int]: "ok"}
print(d[list[int]])
print(list[int] in {list[int]})

# User class without __class_getitem__ raises TypeError
class MyClass:
    pass

try:
    MyClass[int]
except TypeError as e:
    print("TypeError caught")
