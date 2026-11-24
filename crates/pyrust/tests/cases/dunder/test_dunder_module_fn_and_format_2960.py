"""Issue #2960: non-builtins module functions as dunders/attrs, typing sentinels,
and shared slot-router dispatch for `__format__` across `format()`, `str.format`,
f-strings, and `string.Formatter`.
"""

import math
import string
import sys
import typing


def show(label, fn):
    try:
        print(f"{label}: OK {fn()!r}")
    except BaseException as e:
        print(f"{label}: {type(e).__name__}: {e}")


# ── 1. Non-builtins module functions as regular attributes & dunders ─────────


class ModFnDunders:
    f = math.sqrt
    __len__ = math.sqrt
    __bool__ = math.sqrt
    __repr__ = math.sqrt
    __str__ = math.sqrt
    __add__ = math.sqrt
    __getitem__ = math.sqrt
    __class_getitem__ = math.sqrt


m = ModFnDunders()
show("ModFnDunders.f is math.sqrt", lambda: ModFnDunders.f is math.sqrt)
show("ModFnDunders().f is math.sqrt", lambda: m.f is math.sqrt)
show("ModFnDunders().f(9)", lambda: m.f(9))
# Warm CallMethod cache on non-descriptor module function attribute
show("warm CallMethod m.f(16)", lambda: [m.f(16) for _ in range(20)][-1])
show("len(ModFnDunders())", lambda: len(m))
show("bool(ModFnDunders())", lambda: bool(m))
show("repr(ModFnDunders())", lambda: repr(m))
show("str(ModFnDunders())", lambda: str(m))
show("ModFnDunders() + 9", lambda: m + 9)
show("ModFnDunders() + 'x'", lambda: m + "x")
show("ModFnDunders()[25]", lambda: m[25])
show("ModFnDunders[36]", lambda: ModFnDunders[36])


class SubModFn(ModFnDunders):
    pass


s = SubModFn()
show("super().f is math.sqrt", lambda: super(SubModFn, s).f is math.sqrt)
show("super().f(49)", lambda: super(SubModFn, s).f(49))
show("len(SubModFn())", lambda: len(s))
show("SubModFn() + 4", lambda: s + 4)


class MetaModFn(type):
    f = math.sqrt
    __len__ = math.sqrt
    __getitem__ = math.sqrt


class WithMetaModFn(metaclass=MetaModFn):
    pass


show("WithMetaModFn.f is math.sqrt", lambda: WithMetaModFn.f is math.sqrt)
show("WithMetaModFn.f(64)", lambda: WithMetaModFn.f(64))
show("len(WithMetaModFn)", lambda: len(WithMetaModFn))
show("WithMetaModFn[81]", lambda: WithMetaModFn[81])


# ── 2. Typing __class_getitem__ sentinels & synthetic stdlib descriptors ─────

T = typing.TypeVar("T")


class Box(typing.Generic[T]):
    pass


show("list[int]", lambda: list[int])
show("Union[int, str].__origin__", lambda: typing.Union[int, str].__origin__)
show("Union[int, str].__args__", lambda: typing.Union[int, str].__args__)
show("Optional[int].__origin__", lambda: typing.Optional[int].__origin__)
show("Optional[int].__args__", lambda: typing.Optional[int].__args__)
show("Generic[T].__origin__", lambda: typing.Generic[T].__origin__ is typing.Generic)
show("Box[int].__origin__", lambda: Box[int].__origin__ is Box)
show("Box[int].__args__", lambda: Box[int].__args__)
show("sys.version_info >= (3, 12)", lambda: sys.version_info >= (3, 12))
show("len(sys.version_info)", lambda: len(sys.version_info))
show("sys.version_info[0]", lambda: sys.version_info[0])
show("isinstance(repr(sys.version_info), str)", lambda: isinstance(repr(sys.version_info), str))


# ── 3. __format__ routed through shared slot router ──────────────────────────


def check_all_format_sites(label, obj, spec="x"):
    show(f"{label} format(obj)", lambda: format(obj))
    show(f"{label} format(obj, spec)", lambda: format(obj, spec))
    show(f"{label} str.format empty", lambda: "{}".format(obj))
    show(f"{label} str.format spec", lambda: ("{:" + spec + "}").format(obj))
    show(f"{label} fstring empty", lambda: f"{obj}")
    show(f"{label} fstring spec", lambda: f"{obj:{spec}}")
    show(
        f"{label} Formatter",
        lambda: string.Formatter().format("{:" + spec + "}", obj),
    )


class FmtBuiltinFormat:
    __format__ = format


check_all_format_sites("__format__=format", FmtBuiltinFormat())
# Also verify literal-spec FormatValueSpec opcode cache path
_bf = FmtBuiltinFormat()
show("literal f'{_bf:x}'", lambda: f"{_bf:x}")
show("literal f'{_bf}'", lambda: f"{_bf}")
show("warm literal f'{_bf:abc}'", lambda: [f"{_bf:abc}" for _ in range(10)][-1])


class FmtMathSqrt:
    __format__ = math.sqrt


check_all_format_sites("__format__=math.sqrt", FmtMathSqrt())


class FmtStrUpper:
    __format__ = str.upper


check_all_format_sites("__format__=str.upper", FmtStrUpper())


class FmtStatic:
    __format__ = staticmethod(lambda spec: f"S:{spec}")


check_all_format_sites("__format__=staticmethod", FmtStatic())


class FmtClass:
    @classmethod
    def __format__(cls, spec):
        return f"C:{cls.__name__}:{spec}"


check_all_format_sites("__format__=classmethod", FmtClass())


class FmtProp:
    @property
    def __format__(self):
        return lambda spec: f"P:{spec}"


check_all_format_sites("__format__=property", FmtProp())


class FmtPropNonCallable:
    @property
    def __format__(self):
        return 123


check_all_format_sites("__format__=property->int", FmtPropNonCallable())


class CustomFmtDesc:
    def __get__(self, obj, objtype=None):
        return lambda spec: f"D:{type(obj).__name__}:{objtype.__name__}:{spec}"


class FmtCustomDesc:
    __format__ = CustomFmtDesc()


check_all_format_sites("__format__=custom_desc", FmtCustomDesc())


class CallableObj:
    def __call__(self, spec):
        return f"CALL:{spec}"


class FmtCallableObj:
    __format__ = CallableObj()


check_all_format_sites("__format__=callable_obj", FmtCallableObj())


class FmtNotCallable:
    __format__ = 99


check_all_format_sites("__format__=int", FmtNotCallable())


class FmtReturnsNonStr:
    def __format__(self, spec):
        return 42


check_all_format_sites("__format__->int", FmtReturnsNonStr())


class FmtInstShadow:
    def __format__(self, spec):
        return f"CLS:{spec}"


_shadow = FmtInstShadow()
_shadow.__format__ = lambda spec: "INST"
check_all_format_sites("instance __format__ ignored", _shadow)


class BareClass:
    def __str__(self):
        return "BARE"


check_all_format_sites("missing __format__ fallback", BareClass())


class IntSubFormatBuiltin(int):
    __format__ = format


check_all_format_sites("int sub __format__=format", IntSubFormatBuiltin(5))


class IntSubOwnFormat(int):
    __format__ = int.__format__


check_all_format_sites("int sub __format__=int.__format__", IntSubOwnFormat(255))


class IntSubObjectFormat(int):
    __format__ = object.__format__


check_all_format_sites("int sub __format__=object.__format__", IntSubObjectFormat(5))


class IntSubStrFormat(int):
    __format__ = str.__format__


check_all_format_sites("int sub __format__=str.__format__", IntSubStrFormat(5))


class IntSubPlain(int):
    pass


check_all_format_sites("int sub plain", IntSubPlain(255))


class StrSubOverride(str):
    def __format__(self, spec):
        return f"STRSUB({str(self)}:{spec})"


check_all_format_sites("str sub override", StrSubOverride("hi"))


class MetaFmtFormat(type):
    __format__ = format


class WithMetaFmtFormat(metaclass=MetaFmtFormat):
    pass


check_all_format_sites("metaclass __format__=format", WithMetaFmtFormat)


class MetaFmtProp(type):
    @property
    def __format__(cls):
        return lambda spec: f"MP:{cls.__name__}:{spec}"


class WithMetaFmtProp(metaclass=MetaFmtProp):
    pass


check_all_format_sites("metaclass __format__=property", WithMetaFmtProp)
