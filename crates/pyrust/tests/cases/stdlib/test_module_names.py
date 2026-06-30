from typing import ParamSpec, TypeVarTuple
import dataclasses
import enum

# typing classes defined in typing_py.py
P = ParamSpec('P')
print(type(P).__module__)                         # typing
print(TypeVarTuple.__module__)                    # typing

# dataclasses
print(dataclasses.FrozenInstanceError.__module__) # dataclasses
print(dataclasses.Field.__module__)               # dataclasses

# enum
print(enum.EnumType.__module__)  # enum
print(enum.EnumMeta.__module__)  # enum

# A class defined at top level still reports __main__.
class _LocalClass:
    pass


print(_LocalClass.__module__)  # __main__

print("module names ok")
