import collections.abc as old_abc
import os as old_os
import sys
import types as old_types
import typing as old_typing


# Public ABC wrappers are reloadable, but their classes come from one
# process-canonical backing registry.  Existing subclasses keep the same base.
old_sequence = old_abc.Sequence


class RetainedSequence(old_sequence):
    pass


del sys.modules["collections.abc"]
import collections.abc as new_abc

print(
    "abc canonical:",
    old_abc is new_abc,
    old_sequence is new_abc.Sequence,
    issubclass(RetainedSequence, new_abc.Sequence),
    isinstance([], old_sequence),
    old_sequence.__module__,
    old_sequence.__qualname__,
)


# sys struct-sequence class metadata belongs to sys even though the singleton
# values themselves are installed directly on the interpreter's sys module.
print(
    "sys class metadata:",
    type(sys.flags).__module__,
    type(sys.flags).__qualname__,
    type(sys.version_info).__module__,
    type(sys.version_info).__qualname__,
)


# os result classes are tuple-derived, public, and canonical across public
# module generations.  Direct construction and host factories use one class.
old_terminal_class = old_os.terminal_size
old_stat_class = old_os.stat_result
retained_terminal = old_terminal_class((80, 24))
retained_stat = old_stat_class(tuple(range(10)))

del sys.modules["os"]
import os as new_os

new_terminal = new_os.terminal_size(sequence=(100, 40))
new_stat = new_os.stat_result(sequence=tuple(range(10)))
supplemented_stat = new_os.stat_result(
    tuple(range(10)),
    {"st_atime": 42, "st_atime_ns": 99},
)
print(
    "os canonical:",
    old_os is new_os,
    old_terminal_class is new_os.terminal_size,
    old_stat_class is new_os.stat_result,
    type(retained_terminal) is new_os.terminal_size,
    type(retained_stat) is new_os.stat_result,
    type(new_terminal) is old_terminal_class,
    type(new_stat) is old_stat_class,
)
print(
    "os terminal sequence:",
    isinstance(new_terminal, tuple),
    tuple(new_terminal),
    len(new_terminal),
    new_terminal.columns,
    new_terminal.lines,
    repr(new_terminal),
)
print(
    "os stat sequence:",
    isinstance(new_stat, tuple),
    tuple(new_stat),
    len(new_stat),
    new_stat.st_mode,
    new_stat.st_ctime,
    repr(new_stat),
)
print(
    "os stat supplement:",
    tuple(supplemented_stat),
    supplemented_stat.st_atime,
    supplemented_stat.st_atime_ns,
    repr(supplemented_stat),
)
print(
    "os class metadata:",
    new_os.terminal_size.__module__,
    new_os.terminal_size.__qualname__,
    new_os.stat_result.__module__,
    new_os.stat_result.__qualname__,
    new_os.terminal_size.__base__ is tuple,
    new_os.stat_result.__base__ is tuple,
)


# GenericAlias and the PEP 695 runtime types are process-canonical.  In
# particular, syntax TypeVars and manually-created TypeVars have one class.
old_generic_alias = old_types.GenericAlias
del sys.modules["types"]
import types as new_types

manual_typevar = old_typing.TypeVar("Manual")
exec("type Alias[Syntax] = list[Syntax]")
syntax_typevar = Alias.__type_params__[0]
old_typevar_class = old_typing.TypeVar
old_alias_class = old_typing.TypeAliasType
manual_alias = old_alias_class("ManualAlias", int)

del sys.modules["typing"]
import typing as new_typing

late_typevar = old_typevar_class("Late")
late_alias = old_alias_class("LateAlias", str, type_params=(late_typevar,))
print(
    "types canonical:",
    old_types is new_types,
    old_generic_alias is new_types.GenericAlias,
)
print(
    "typing canonical:",
    old_typevar_class is new_typing.TypeVar,
    old_alias_class is new_typing.TypeAliasType,
    type(manual_typevar) is new_typing.TypeVar,
    type(syntax_typevar) is new_typing.TypeVar,
    type(manual_alias) is new_typing.TypeAliasType,
    type(late_typevar) is new_typing.TypeVar,
    type(late_alias) is new_typing.TypeAliasType,
)
print(
    "typing values:",
    repr(manual_typevar),
    repr(syntax_typevar),
    repr(late_typevar),
    repr(manual_alias),
    repr(late_alias),
    late_alias.__value__ is str,
    late_alias.__type_params__ == (late_typevar,),
)
print(
    "typing class metadata:",
    new_typing.TypeVar.__module__,
    new_typing.TypeVar.__qualname__,
    new_typing.TypeAliasType.__module__,
    new_typing.TypeAliasType.__qualname__,
)
