import sys
import typing


T = typing.TypeVar("T")

old_any = typing.Any
old_any_class = type(old_any)
old_generic_alias_class = typing._GenericAlias
old_generic = typing.Generic
old_protocol = typing.Protocol
old_union = typing.Union
old_optional = typing.Optional
old_special_forms = (
    typing.Callable,
    typing.ClassVar,
    typing.Final,
    typing.Literal,
)
old_legacy_aliases = (
    typing.List,
    typing.Dict,
    typing.Set,
    typing.FrozenSet,
    typing.Tuple,
    typing.Type,
)
old_list = typing.List
old_named_tuple = typing.NamedTuple
old_typed_dict = typing.TypedDict

old_generic_alias = old_generic[T]
old_union_alias = old_union[int, str]
old_optional_alias = old_optional[int]
old_list_alias = old_list[int]


class OldBox(old_generic[T]):
    pass


@typing.runtime_checkable
class OldProtocol(old_protocol):
    def ping(self):
        pass


class HasPing:
    def ping(self):
        return "pong"


print(
    "typing generation:",
    type(old_generic_alias) is old_generic_alias_class,
    old_generic_alias.__origin__ is old_generic,
    typing.get_origin(old_union_alias) is old_union,
    typing.get_origin(old_optional_alias) is old_union,
    typing.get_origin(old_list_alias) is list,
    issubclass(OldBox, old_generic),
    isinstance(HasPing(), OldProtocol),
)

del sys.modules["typing"]
import typing as reloaded_typing

new_special_forms = (
    reloaded_typing.Callable,
    reloaded_typing.ClassVar,
    reloaded_typing.Final,
    reloaded_typing.Literal,
)
new_legacy_aliases = (
    reloaded_typing.List,
    reloaded_typing.Dict,
    reloaded_typing.Set,
    reloaded_typing.FrozenSet,
    reloaded_typing.Tuple,
    reloaded_typing.Type,
)

print(
    "typing identities:",
    old_any is reloaded_typing.Any,
    old_any_class is type(reloaded_typing.Any),
    old_generic_alias_class is reloaded_typing._GenericAlias,
    old_generic is reloaded_typing.Generic,
    old_protocol is reloaded_typing.Protocol,
    old_union is reloaded_typing.Union,
    old_optional is reloaded_typing.Optional,
    any(old is new for old, new in zip(old_special_forms, new_special_forms)),
    any(old is new for old, new in zip(old_legacy_aliases, new_legacy_aliases)),
    old_named_tuple is reloaded_typing.NamedTuple,
    old_typed_dict is reloaded_typing.TypedDict,
)

late_old_union = old_union[old_union_alias, float]
late_old_optional = old_optional[float]
late_generic_alias = old_generic[T]
new_generic_alias = reloaded_typing.Generic[T]

print(
    "typing old semantics:",
    type(old_generic_alias) is old_generic_alias_class,
    type(old_generic_alias) is reloaded_typing._GenericAlias,
    type(late_generic_alias) is reloaded_typing._GenericAlias,
    type(new_generic_alias) is reloaded_typing._GenericAlias,
    late_old_union.__origin__ is old_union,
    late_old_union.__args__ == (int, str, float),
    late_old_optional.__origin__ is old_union,
    repr(late_old_union) == "typing.Union[int, str, float]",
    repr(late_old_optional) == "typing.Optional[float]",
    late_old_union == old_union[float, str, int],
    isinstance([], old_list),
    issubclass(list, old_list),
    isinstance(HasPing(), OldProtocol),
    issubclass(OldProtocol, old_protocol),
    issubclass(OldProtocol, reloaded_typing.Protocol),
)


# Old functional/class-base bindings retain their semantics after re-import.
class LatePoint(old_named_tuple):
    value: int


class LateMovie(old_typed_dict):
    title: str


point = LatePoint(7)
movie = LateMovie(title="old marker")
print("typing runtime markers:", point.value, movie["title"])


# The process-canonical Generic receiver resolves the active interpreter's
# module generation, including a deliberate replacement of its implementation
# class. A fresh TypeVar avoids CPython's subscription-result cache.
class ReplacementGenericAlias:
    def __init__(self, *args, **kwargs):
        pass


U = reloaded_typing.TypeVar("U")
reloaded_typing._GenericAlias = ReplacementGenericAlias
patched_generic_alias = old_generic[U]
print("typing monkeypatch:", type(patched_generic_alias) is ReplacementGenericAlias)
