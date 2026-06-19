# Regression guard for issue #2629: the `@inject` post-load dispatcher in
# `pyrust_builtin_modules!` must inject every Python-source module's members,
# and the `collections` class-metadata fix-ups (`__module__` /
# `__class_getitem__`) must survive the move into
# `collections::inject_python_members`.
import abc
import asyncio
import collections
import dataclasses
import enum
import json
import operator
import string
import typing

# Each @inject module exposes its Python-source members.
print(collections.Counter("aabbbc"))
print(string.capwords("the quick brown fox"))
print(operator.mul(6, 7))
print(json.dumps({"k": [1, 2, 3]}, separators=(",", ":")))
print(json.loads('{"k": [1, 2, 3]}'))


@dataclasses.dataclass
class Point:
    x: int
    y: int


print(dataclasses.asdict(Point(1, 2)))


class Color(enum.Enum):
    RED = 1
    GREEN = 2


print(Color.RED.name, Color.RED.value)


class Drawable(abc.ABC):
    @abc.abstractmethod
    def draw(self): ...


print(sorted(Drawable.__abstractmethods__))
print(typing.get_origin(typing.Dict[str, int]))


async def _main():
    await asyncio.sleep(0)
    return "ok"


print(asyncio.run(_main()))

# collections class-metadata fix-ups (moved into inject_python_members).
for cls_name in ["Counter", "OrderedDict", "ChainMap", "UserDict", "UserList", "UserString"]:
    cls = getattr(collections, cls_name)
    print(cls_name, cls.__module__)

# PEP 585 subscription on a collections container (issue #2603).
print(collections.OrderedDict[int])
print(collections.ChainMap[str, int])
