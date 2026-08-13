# CPython 3.12 parity for issue #2938: type-argument qualifiers, the module
# name inherited by classes created through exec(), and bound super reprs.

import builtins
import types
from typing import Generic, List, Optional, TypeVar


class GenericArg:
    pass


class GenericOuter:
    class Inner:
        pass


Invariant = TypeVar("Invariant")
Covariant = TypeVar("Covariant", covariant=True)


class GenericBox(Generic[Invariant]):
    pass

print(repr(dict[str, GenericArg]))
print(repr(list[GenericOuter.Inner]))
print(repr(List[GenericOuter.Inner]), repr(Optional[GenericOuter.Inner]))
print(repr(GenericBox[GenericArg]))
print(repr(List[GenericArg]))
print(repr(Optional[GenericArg]))
print(repr(list[Invariant]), repr(list[Covariant]))
print(repr(List[Invariant]), repr(List[Covariant]))
print(repr(Optional[Invariant]), repr(Optional[Covariant]))


# A normal instance may expose TypeVar-shaped attributes. Generic alias repr
# must still dispatch that object's own repr instead of treating it as a
# TypeVar based on its mutable attribute shape.
class FakeTypeVar:
    def __init__(self):
        self.__name__ = "Fake"
        self.__infer_variance__ = False
        self.__covariant__ = True

    def __repr__(self):
        return "USER_REPR"


fake_typevar = FakeTypeVar()
print(repr(list[fake_typevar]))
print(repr(List[fake_typevar]))
print(repr(Optional[fake_typevar]))
print(repr(list[list[fake_typevar]]))


class RaisingArg:
    def __repr__(self):
        raise RuntimeError("RAISING_REPR")


try:
    repr(list[RaisingArg()])
except RuntimeError as error:
    print(type(error).__name__, error)


class NonStringArg:
    def __repr__(self):
        return 42


try:
    repr(list[NonStringArg()])
except TypeError as error:
    print(type(error).__name__, error)


class ReentrantArg:
    entered = False

    def __repr__(self):
        if self.entered:
            return "INNER"
        self.entered = True
        try:
            return f"OUTER({reentrant_alias!r})"
        finally:
            self.entered = False


reentrant_alias = list[ReentrantArg()]
print(repr(reentrant_alias))


# str() and print() must use the same interpreter-aware GenericAlias repr as
# repr(), including user repr dispatch, errors, nesting, and re-entry.
for label, alias in (
    ("fake-list", list[fake_typevar]),
    ("fake-List", List[fake_typevar]),
    ("fake-Optional", Optional[fake_typevar]),
    ("fake-nested", list[list[fake_typevar]]),
    ("raising", list[RaisingArg()]),
    ("non-string", list[NonStringArg()]),
    ("reentrant", reentrant_alias),
):
    try:
        print("str", label, str(alias))
    except (RuntimeError, TypeError) as error:
        print("str", label, type(error).__name__, error)
    print("print", label)
    try:
        print(alias)
    except (RuntimeError, TypeError) as error:
        print(type(error).__name__, error)


# GenericAlias and typing use a qualifier rule distinct from type.__repr__ for
# non-string __module__ values. A str subclass contributes its raw text; its
# user __repr__ must not run.
class ModuleText(str):
    def __repr__(self):
        raise RuntimeError("generic alias called module repr")


for label, module in (
    ("none", None),
    ("int", 42),
    ("str-subclass", ModuleText("strmod")),
    ("builtins", "builtins"),
    ("empty", ""),
):
    GenericOuter.Inner.__module__ = module
    GenericOuter.Inner.__qualname__ = "Renamed.Deep"
    print(label, repr(list[GenericOuter.Inner]))
    print(label, repr(List[GenericOuter.Inner]))
    print(label, repr(Optional[GenericOuter.Inner]))


class ModuleObject:
    def __str__(self):
        return "modx"

    def __repr__(self):
        raise RuntimeError("generic alias called module repr")


class RaisingModule:
    def __str__(self):
        raise RuntimeError("MODULE_STR")


class NonStringModule:
    def __str__(self):
        return 42


for label, module in (
    ("object", ModuleObject()),
    ("raising", RaisingModule()),
    ("non-string", NonStringModule()),
):
    GenericOuter.Inner.__module__ = module
    aliases = (
        ("list", list[GenericOuter.Inner]),
        ("List", List[GenericOuter.Inner]),
        ("Optional", Optional[GenericOuter.Inner]),
    )
    for alias_name, alias in aliases:
        try:
            print(label, alias_name, repr(alias))
        except (RuntimeError, TypeError) as error:
            print(label, alias_name, type(error).__name__, error)


# Explicit types.GenericAlias accepts a user class as its origin. Its module
# qualifier follows the same Python-visible str boundary as a class argument.
class Origin:
    pass


origin_default_module = Origin.__module__
for label, has_module, module in (
    ("absent", False, None),
    ("default", True, origin_default_module),
    ("str", True, "originmod"),
    ("str-subclass", True, ModuleText("strmod")),
    ("none", True, None),
    ("int", True, 42),
    ("object", True, ModuleObject()),
    ("raising", True, RaisingModule()),
    ("non-string", True, NonStringModule()),
    ("builtins", True, "builtins"),
    ("empty", True, ""),
):
    if has_module:
        Origin.__module__ = module
        origin = Origin
    else:
        # Canonical builtins have no raw __module__ entry. A Python-created
        # class cannot delete its mandatory __module__ slot.
        origin = list
    alias = types.GenericAlias(origin, int)
    try:
        print("origin", label, repr(alias))
    except (RuntimeError, TypeError) as error:
        print("origin", label, type(error).__name__, error)
Origin.__module__ = origin_default_module


def exec_class_module(namespace):
    exec("class Created: pass", namespace)
    return namespace["Created"].__module__


# With no globals __name__, LOAD_NAME falls through to the configured
# __builtins__ provider.  An explicit globals value remains authoritative.
print(exec_class_module({}))
print(exec_class_module({"__name__": "explicit_module"}))
dict_provider = {"__name__": "dict_provider"}
# CPython implements class statements through __build_class__; PyRust lowers
# them directly and does not expose that helper as a builtins attribute yet.
if hasattr(builtins, "__build_class__"):
    dict_provider["__build_class__"] = builtins.__build_class__
print(
    exec_class_module(
        {
            "__builtins__": dict_provider,
        }
    )
)

dict_provider_without_name = {}
if hasattr(builtins, "__build_class__"):
    dict_provider_without_name["__build_class__"] = builtins.__build_class__
try:
    exec_class_module({"__builtins__": dict_provider_without_name})
except NameError as error:
    print(type(error).__name__, error)

original_builtins_name = builtins.__name__
builtins.__name__ = "module_provider"
try:
    print(exec_class_module({"__builtins__": builtins}))
    print(
        exec_class_module(
            {"__builtins__": builtins, "__name__": "explicit_over_module"}
        )
    )
finally:
    builtins.__name__ = original_builtins_name

del builtins.__name__
try:
    exec_class_module({"__builtins__": builtins})
except NameError as error:
    print(type(error).__name__, error)
finally:
    builtins.__name__ = original_builtins_name


class SuperBase:
    pass


class SuperChild(SuperBase):
    def __repr__(self):
        raise RuntimeError("bound super repr called user repr")

    def instance_proxy_repr(self):
        return repr(super())

    @classmethod
    def class_proxy_repr(cls):
        return repr(super())


print(SuperChild().instance_proxy_repr())
print(SuperChild.class_proxy_repr())
print(repr(super(SuperChild)))


class SuperOuter:
    class Nested(SuperBase):
        def proxy_repr(self):
            return repr(super())


print(SuperOuter.Nested().proxy_repr())
