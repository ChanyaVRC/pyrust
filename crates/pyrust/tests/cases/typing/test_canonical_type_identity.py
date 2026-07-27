import builtins
import types
import typing


# A user class may reuse every visible spelling of NoneType. It is still not
# the canonical type(None), so neither PEP 604 nor typing.Union may render it as
# None/Optional.
class FakeNone:
    pass


FakeNone.__name__ = "NoneType"
FakeNone.__qualname__ = "NoneType"
FakeNone.__module__ = "builtins"
print("fake NoneType PEP 604:", repr(int | FakeNone))
print("fake NoneType typing:", repr(typing.Union[int, FakeNone]))
print("real NoneType PEP 604:", repr(int | type(None)))
print("real NoneType typing:", repr(typing.Union[int, type(None)]))


# GenericAlias accepts arbitrary origins. A class whose presentation metadata
# says "typing.Union" must not gain the native typing.Union Optional shortcut.
class FakeUnion:
    pass


FakeUnion.__name__ = "Union"
FakeUnion.__qualname__ = "Union"
FakeUnion.__module__ = "typing"
fake_union_alias = types.GenericAlias(FakeUnion, (int, type(None)))
fake_union_reordered = types.GenericAlias(FakeUnion, (type(None), int))
print("fake Union origin:", repr(fake_union_alias))
print("fake Union equality:", fake_union_alias == fake_union_reordered)
try:
    isinstance(1, fake_union_alias)
except Exception as error:
    print("fake Union isinstance:", type(error).__name__)


# A genuine primitive subclass keeps its primitive backing after its own
# visible class name changes.
class RenamedList(builtins.list):
    pass


RenamedList.__name__ = "ChangedList"
RenamedList.__qualname__ = "ChangedList"
renamed = RenamedList([1, 2])
print(
    "renamed true subclass:",
    repr(renamed),
    str(renamed),
    type(renamed).__name__,
    isinstance(renamed, builtins.list),
)
