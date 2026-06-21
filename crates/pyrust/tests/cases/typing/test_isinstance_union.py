from typing import Union, get_origin
import typing

# isinstance with typing.Union (CPython 3.12 treats it like a tuple of __args__)
x = 5
print(isinstance(x, Union[int, str]))       # True
print(isinstance("hi", Union[int, str]))    # True
print(isinstance(3.14, Union[int, str]))    # False

# isinstance with nested Union (flattened)
print(isinstance(5, Union[int, Union[float, str]]))  # True
print(isinstance("s", Union[int, Union[float, str]]))  # True
print(isinstance(b"b", Union[int, Union[float, str]]))  # False

# Ensure normal isinstance still works
print(isinstance(5, int))         # True
print(isinstance(5, (int, str)))  # True
print(isinstance(5, str))         # False

# issubclass with typing.Union
print(issubclass(int, Union[int, str]))   # True
print(issubclass(float, Union[int, str]))  # False

# repr of the Union special form and of a Union alias
print(repr(typing.Union))         # typing.Union
print(str(Union[int, str]))       # typing.Union[int, str]
print(repr(typing.Optional))      # typing.Optional
print(get_origin(Union[int, str]) is Union)  # True
