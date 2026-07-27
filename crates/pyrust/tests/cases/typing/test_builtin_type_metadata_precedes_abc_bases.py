from collections.abc import Hashable
from typing import Annotated


print(issubclass(int, Hashable))
print(int.__module__)
print(str.__module__)
print("__module__" in int.__dict__)
print(Annotated[int, "positive"])
print(Annotated[str, "required"])
