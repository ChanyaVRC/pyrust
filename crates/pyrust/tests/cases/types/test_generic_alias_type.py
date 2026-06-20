"""Parity fixture for issue #2733: type(list[int]) is the types.GenericAlias
class (a real PyClass singleton), not a BuiltinFunction sentinel.

CPython 3.12 behaviour being verified:
- repr(type(list[int])) == "<class 'types.GenericAlias'>"
- type(list[int]).__name__ == "GenericAlias"
- type(list[int]).__qualname__ == "GenericAlias"
- type(list[int]).__module__ == "types"  (no AttributeError)
- list[int].__class__ is type(list[int])
- type(type(list[int])) is type
- the GenericAlias type is a shared singleton across all PEP 585 aliases
- the alias instance keeps forwarding __module__ / __doc__ to its origin
"""

ga = list[int]

# --- the type object ---
print(repr(type(ga)))                 # <class 'types.GenericAlias'>
print(type(ga).__name__)              # GenericAlias
print(type(ga).__qualname__)          # GenericAlias
print(type(ga).__module__)            # types

# metatype is `type`
print(type(type(ga)) is type)         # True
print(repr(type(type(ga))))           # <class 'type'>

# __bases__ / __mro__
print(type(ga).__bases__)             # (<class 'object'>,)
print(type(ga).__mro__)               # (<class 'types.GenericAlias'>, <class 'object'>)

# --- __class__ resolves to the same type ---
print(repr(ga.__class__))             # <class 'types.GenericAlias'>
print(ga.__class__ is type(ga))       # True

# --- singleton across all PEP 585 aliases ---
print(type(list[int]) is type(dict[str, int]))      # True
print(type(list[int]) is type(tuple[int, ...]))     # True
print(type(set[int]) is type(frozenset[int]))       # True

# --- the alias value still proxies to its origin ---
print(ga.__module__)                  # builtins  (forwarded to list)
print(ga.__doc__ == list.__doc__)     # True

# isinstance: the alias is NOT itself a class
print(isinstance(ga, type))           # False
print(isinstance(list[int], type(list[int])))  # True

# callable: the GenericAlias type object is callable
print(callable(type(ga)))             # True
