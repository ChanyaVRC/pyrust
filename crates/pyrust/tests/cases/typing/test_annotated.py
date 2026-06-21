from typing import Annotated, get_args, get_origin, get_type_hints

# Basic subscription
a = Annotated[int, "positive"]
print(repr(a))        # typing.Annotated[int, 'positive']
print(get_args(a))    # (int, 'positive')
print(get_origin(a) is Annotated)  # True
print(a.__origin__)   # <class 'int'>
print(a.__metadata__)  # ('positive',)
print(a.__args__)     # (<class 'int'>,) -- metadata is NOT in __args__

# Multiple metadata
b = Annotated[str, "max_len=100", "utf-8"]
print(repr(b))
print(get_args(b))    # (str, 'max_len=100', 'utf-8')
print(b.__args__)     # (<class 'str'>,)
print(get_origin(b) is Annotated)  # True

# Single argument is an error
try:
    Annotated[int]
except TypeError as e:
    print("TypeError:", e)

# In class annotations
class Model:
    name: Annotated[str, "required"]
    age: Annotated[int, "positive", ">=0"]


hints = get_type_hints(Model, include_extras=True)
print(get_args(hints["name"]))  # (str, 'required')
print(get_args(hints["age"]))   # (int, 'positive', '>=0')
print(repr(hints["name"]))      # typing.Annotated[str, 'required']

# Without include_extras strips annotations down to the type
hints_plain = get_type_hints(Model)
print(hints_plain["name"])  # <class 'str'>
print(hints_plain["age"])   # <class 'int'>

# Nested aliases flatten at construction (PEP 593 / CPython 3.12)
nested = Annotated[Annotated[int, "x"], "y"]
print(repr(nested))            # typing.Annotated[int, 'x', 'y']
print(get_args(nested))        # (int, 'x', 'y')
print(nested.__origin__)       # <class 'int'>
print(nested.__metadata__)     # ('x', 'y')
print(nested == Annotated[int, "x", "y"])  # True

deep = Annotated[Annotated[Annotated[str, 1], 2], 3]
print(repr(deep))              # typing.Annotated[str, 1, 2, 3]
print(get_args(deep))          # (str, 1, 2, 3)

# Equality and hashing
print(Annotated[int, "x"] == Annotated[int, "x"])
print(Annotated[int, "x"] == Annotated[int, "y"])
print(Annotated[int, "x"] == int)
print(hash(Annotated[int, "x"]) == hash(Annotated[int, "x"]))
