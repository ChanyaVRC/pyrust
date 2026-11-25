"""Comprehensive Python 3.11/3.12 stdlib parity suite (issue #3231).

Exercises Python 3.11/3.12 public APIs and edge cases across:
  math, itertools, datetime, enum, typing, functools, string, operator.
Output is diffed line-for-line against CPython 3.12 by parity_compare.
"""

import collections.abc
import datetime
import enum
import functools
import io
import itertools
import math
import operator
import string
import sys
import types
import typing


def show(label, fn):
    try:
        print(f"OK  {label}: {fn()!r}")
    except Exception as exc:
        print(f"ERR {label}: {type(exc).__name__}: {exc}")


def show_exc_type(label, fn):
    try:
        print(f"OK  {label}: {fn()!r}")
    except Exception as exc:
        print(f"ERR {label}: {type(exc).__name__}")


# =============================================================================
# 1. math
# =============================================================================
print("=== math ===")
show("sumprod ints", lambda: math.sumprod([1, 2, 3], [4, 5, 6]))
show("sumprod compensated", lambda: math.sumprod([1e16, 1.0, -1e16], [1.0, 1.0, 1.0]))
show("sumprod bools", lambda: (type(math.sumprod([True, False], [True, True])).__name__, math.sumprod([True, False], [True, True])))
show("sumprod empty", lambda: math.sumprod([], []))
show("sumprod gen", lambda: math.sumprod((i for i in range(5)), (i * 2 for i in range(5))))
show("sumprod bigints", lambda: math.sumprod([10**30, 1], [1, 7]))
show("sumprod mismatch", lambda: math.sumprod([1, 2], [1]))

show("cbrt", lambda: (math.cbrt(27), math.cbrt(-8), math.cbrt(0.0), math.copysign(1.0, math.cbrt(-0.0)), math.cbrt(math.inf), math.cbrt(-math.inf), math.isnan(math.cbrt(math.nan))))
show("exp2", lambda: (math.exp2(0), math.exp2(10), math.exp2(-2), math.exp2(-1075)))
show("exp2 overflow", lambda: math.exp2(1024))

show("nextafter default", lambda: math.nextafter(1.0, 2.0))
show("nextafter steps=None", lambda: math.nextafter(1.0, 2.0, steps=None))
show("nextafter steps=0", lambda: math.nextafter(1.0, 2.0, steps=0))
show("nextafter steps=0 negzero", lambda: math.copysign(1.0, math.nextafter(-0.0, 0.0, steps=0)))
show("nextafter steps=0 nan_y", lambda: math.nextafter(1.0, math.nan, steps=0))
show("nextafter steps=2", lambda: math.nextafter(1.0, 2.0, steps=2))
show("nextafter cross_zero_down_1", lambda: (math.nextafter(5e-324, -1.0, steps=1), math.copysign(1.0, math.nextafter(5e-324, -1.0, steps=1))))
show("nextafter cross_zero_up_1", lambda: (math.nextafter(-5e-324, 1.0, steps=1), math.copysign(1.0, math.nextafter(-5e-324, 1.0, steps=1))))
show("nextafter cross_zero_down_2", lambda: math.nextafter(5e-324, -1.0, steps=2))
show("nextafter cross_zero_up_2", lambda: math.nextafter(-5e-324, 1.0, steps=2))
show("nextafter steps=-1", lambda: math.nextafter(1.0, 2.0, steps=-1))

show("ulp", lambda: (math.ulp(0.0), math.ulp(1.0), math.ulp(-1.0), math.ulp(math.inf), math.isnan(math.ulp(math.nan))))
show("dist", lambda: (math.dist([0, 0], [3, 4]), math.dist([1, 2, 3], [4, 6, 3])))
show("hypot", lambda: (math.hypot(), math.hypot(3, 4), math.hypot(2, 3, 6)))
show("isqrt", lambda: (math.isqrt(0), math.isqrt(15), math.isqrt(16), math.isqrt(10**40)))
show("comb", lambda: (math.comb(10, 3), math.comb(5, 10), math.comb(0, 0)))
show("perm", lambda: (math.perm(5), math.perm(5, None), math.perm(5, 2), math.perm(5, 10)))
show("lcm", lambda: (math.lcm(), math.lcm(12), math.lcm(12, 18, 30), math.lcm(0, 5)))
show("gcd", lambda: (math.gcd(), math.gcd(-12), math.gcd(12, 18, 30)))
show("prod", lambda: (math.prod([2, 3, 4]), math.prod([], start=5), math.prod([2, 3], start=10)))
show("remainder", lambda: (math.remainder(5, 2), math.remainder(3.5, 2), math.remainder(2.5, 2)))
show("fsum", lambda: math.fsum([1e16, 1.0, -1e16, 0.5]))


# =============================================================================
# 2. itertools
# =============================================================================
print("=== itertools ===")
show("batched pos", lambda: list(itertools.batched("ABCDEFG", 3)))
show("batched exact", lambda: list(itertools.batched(range(6), 2)))
show("batched empty", lambda: list(itertools.batched([], 3)))
show("batched kw", lambda: list(itertools.batched(iterable="ABCDE", n=2)))
show("batched n=0", lambda: list(itertools.batched("ABC", 0)))
show("batched n=-2", lambda: list(itertools.batched("ABC", -2)))
show("batched n=float", lambda: list(itertools.batched("ABC", 1.5)))

show("pairwise", lambda: (list(itertools.pairwise([])), list(itertools.pairwise([1])), list(itertools.pairwise("ABCD"))))
t1, t2 = itertools.tee(range(4), 2)
show("tee", lambda: (list(t1), list(t2)))
show("groupby", lambda: [(k, list(g)) for k, g in itertools.groupby("AAABBBCCA")])
show("starmap", lambda: list(itertools.starmap(pow, [(2, 3), (3, 2)])))
show("accumulate kw", lambda: (
    list(itertools.accumulate([1, 2, 3, 4], func=operator.mul)),
    list(itertools.accumulate(iterable=[1, 2, 3], initial=10)),
    list(itertools.accumulate([1, 2, 3], initial=None)),
))
show("zip_longest", lambda: list(itertools.zip_longest("AB", "1234", fillvalue="-")))
show("compress kw", lambda: list(itertools.compress(data="ABCD", selectors=[1, 0, 1, 0])))
show("filterfalse", lambda: list(itertools.filterfalse(lambda x: x % 2, range(6))))
show("dropwhile", lambda: list(itertools.dropwhile(lambda x: x < 3, [1, 2, 3, 1, 4])))
show("takewhile", lambda: list(itertools.takewhile(lambda x: x < 3, [1, 2, 3, 1, 4])))
show("islice", lambda: list(itertools.islice(range(10), 1, 8, 2)))
show("chain.from_iterable", lambda: list(itertools.chain.from_iterable([[1, 2], [3], [4, 5]])))
show("product", lambda: list(itertools.product("AB", repeat=2)))
show("permutations kw", lambda: list(itertools.permutations(iterable="ABC", r=2)))
show("combinations kw", lambda: list(itertools.combinations(iterable="ABC", r=2)))
show("cwr kw", lambda: list(itertools.combinations_with_replacement(iterable="AB", r=2)))


# =============================================================================
# 3. datetime
# =============================================================================
print("=== datetime ===")
show("UTC alias", lambda: datetime.UTC is datetime.timezone.utc)
show("date.fromisoformat", lambda: (
    datetime.date.fromisoformat("2024-01-15"),
    datetime.date.fromisoformat("20240115"),
    datetime.date.fromisoformat("2024-W03-1"),
    datetime.date.fromisoformat("2024W031"),
    datetime.date.fromisoformat("2024-W03"),
    datetime.date.fromisoformat("2024W03"),
))
show("time.fromisoformat", lambda: (
    datetime.time.fromisoformat("12"),
    datetime.time.fromisoformat("12:30"),
    datetime.time.fromisoformat("123045"),
    datetime.time.fromisoformat("T12:30:45Z"),
    datetime.time.fromisoformat("12:30:45,5"),
    datetime.time.fromisoformat("12:30:45.123456789"),
    datetime.time.fromisoformat("12:30+05"),
    datetime.time.fromisoformat("12:30-0530"),
))
show("datetime.fromisoformat", lambda: (
    datetime.datetime.fromisoformat("2024-01-15"),
    datetime.datetime.fromisoformat("20240115T123045Z"),
    datetime.datetime.fromisoformat("2024-W03-1T12:30:45+02:00"),
    datetime.datetime.fromisoformat("2024-01-15T12"),
    datetime.datetime.fromisoformat("2024-01-15 12:30:45,25+05"),
    datetime.datetime.fromisoformat("2024-01-15T12:30:45+05:30:15"),
))
show("datetime lowercase z rejected", lambda: datetime.datetime.fromisoformat("2024-01-15T12:30:45z"))
show("time lowercase z rejected", lambda: datetime.time.fromisoformat("12:30:45z"))
ic_d = datetime.date(2024, 1, 15).isocalendar()
ic_dt = datetime.datetime(2024, 1, 15, 10, 20).isocalendar()
show("isocalendar", lambda: (
    ic_d, ic_d.year, ic_d.week, ic_d.weekday,
    ic_dt == ic_d,
    datetime.date.fromisocalendar(2024, 3, 1),
    datetime.datetime.fromisocalendar(2024, 3, 1),
))


# =============================================================================
# 4. enum
# =============================================================================
print("=== enum ===")
show("enum MROs", lambda: (
    issubclass(enum.IntEnum, enum.ReprEnum),
    issubclass(enum.StrEnum, enum.ReprEnum),
    issubclass(enum.IntFlag, enum.ReprEnum),
    issubclass(enum.IntFlag, enum.Flag),
    issubclass(enum.FlagBoundary, enum.StrEnum),
    issubclass(enum.EnumCheck, enum.StrEnum),
))


class HttpMethod(enum.StrEnum):
    GET = enum.auto()
    POST = enum.auto()
    DELETE = "delete"


show("StrEnum", lambda: (
    list(HttpMethod),
    str(HttpMethod.GET),
    repr(HttpMethod.GET),
    f"{HttpMethod.POST}",
    HttpMethod.GET == "get",
    isinstance(HttpMethod.GET, str),
))
def _bad_str_enum():
    class BadStr(enum.StrEnum):
        A = 1


show_exc_type("StrEnum non-str", _bad_str_enum)


class FloatEnum(float, enum.ReprEnum):
    HALF = 0.5
    ONE = 1.0


show("ReprEnum custom", lambda: (str(FloatEnum.HALF), repr(FloatEnum.HALF), f"{FloatEnum.HALF:.2f}", FloatEnum.HALF + 0.5))

show("FlagBoundary", lambda: (
    list(enum.FlagBoundary),
    enum.STRICT is enum.FlagBoundary.STRICT,
    enum.CONFORM is enum.FlagBoundary.CONFORM,
    enum.EJECT is enum.FlagBoundary.EJECT,
    enum.KEEP is enum.FlagBoundary.KEEP,
))


class FStrict(enum.Flag, boundary=enum.STRICT):
    A = 1
    B = 2


class FConform(enum.Flag, boundary=enum.CONFORM):
    A = 1
    B = 2


class FEject(enum.Flag, boundary=enum.EJECT):
    A = 1
    B = 2


class FKeep(enum.Flag, boundary=enum.KEEP):
    A = 1
    B = 2


show("Flag STRICT", lambda: (FStrict(3), ~FStrict.A, list(FStrict.A | FStrict.B), len(FStrict.A | FStrict.B), FStrict(0) in FStrict.A))
show_exc_type("Flag STRICT bad", lambda: FStrict(5))
show_exc_type("Flag STRICT neg bad", lambda: FStrict(-5))
show("Flag STRICT neg ok", lambda: FStrict(-1))
show("Flag CONFORM", lambda: (FConform(5), FConform(4), FConform(-5)))
show("Flag EJECT", lambda: (FEject(5), type(FEject(5)).__name__, FEject(-5), FEject(-1)))
show("Flag KEEP", lambda: (FKeep(5), repr(FKeep(5)), FKeep(4), repr(FKeep(4)), FKeep(-5)))


class Perm(enum.IntFlag):
    R = 4
    W = 2
    X = 1


show("IntFlag", lambda: (repr(Perm(5)), repr(Perm(9)), str(Perm(9)), len(Perm.R | Perm.X), list(Perm.R | Perm.X), ~Perm.W))
show("show_flag_values", lambda: (enum.show_flag_values(0), enum.show_flag_values(5), enum.show_flag_values(7)))
show("show_flag_values neg", lambda: enum.show_flag_values(-1))


@enum.verify(enum.UNIQUE, enum.CONTINUOUS)
class GoodEnum(enum.Enum):
    ONE = 1
    TWO = 2
    THREE = 3


show("verify ok", lambda: list(GoodEnum))


def _bad_unique():
    @enum.verify(enum.UNIQUE)
    class E(enum.Enum):
        A = 1
        B = 1


def _bad_continuous():
    @enum.verify(enum.CONTINUOUS)
    class E(enum.Enum):
        A = 1
        B = 3


def _bad_named_flags():
    @enum.verify(enum.NAMED_FLAGS)
    class F(enum.Flag):
        A = 1
        B = 4
        C = 3


show("verify UNIQUE fail", _bad_unique)
show("verify CONTINUOUS fail", _bad_continuous)
show("verify NAMED_FLAGS fail", _bad_named_flags)


def _bad_unique_dec():
    @enum.unique
    class E(enum.Enum):
        A = 1
        B = 1


show("unique fail", _bad_unique_dec)


@enum.global_enum
class GlobalStatus(enum.Enum):
    GS_OK = 1
    GS_ERR = 2


@enum.global_enum
class GlobalBits(enum.Flag):
    GB_A = 1
    GB_B = 2


show("global_enum", lambda: (
    repr(GlobalStatus.GS_OK),
    str(GlobalStatus.GS_OK),
    GS_OK is GlobalStatus.GS_OK,
    repr(GlobalBits.GB_A),
    repr(GlobalBits.GB_A | GlobalBits.GB_B),
    str(GlobalBits.GB_A),
    GB_B is GlobalBits.GB_B,
))


class RichEnum(enum.Enum):
    X = 10
    value = 20
    NM = enum.nonmember(99)
    FN = enum.member(lambda a: a + 1)

    @enum.property
    def doubled(self):
        return self.value * 2


show("member/nonmember/property", lambda: (
    [m.name for m in RichEnum],
    RichEnum.NM,
    RichEnum.FN.value(41),
    RichEnum.X.doubled,
    RichEnum.value,
    RichEnum.value.value,
))
show("enum.property on cls", lambda: RichEnum.doubled)


class AutoCustom(enum.Enum):
    def _generate_next_value_(name, start, count, last_values):
        return f"item_{name}_{count}"

    ALPHA = enum.auto()
    BETA = enum.auto()


show("custom _generate_next_value_", lambda: [(m.name, m.value) for m in AutoCustom])

DynEnum = enum.Enum("DynEnum", ["FIRST", "SECOND"])
show("functional Enum", lambda: [(m.name, m.value) for m in DynEnum])


# =============================================================================
# 5. typing
# =============================================================================
print("=== typing ===")


class BaseCls:
    def greet(self) -> str:
        return "hi"


class SubCls(BaseCls):
    @typing.override
    def greet(self) -> str:
        return "hello"


show("override", lambda: (getattr(SubCls.greet, "__override__", False), SubCls().greet()))

show("non-subscriptable reprs", lambda: (
    repr(typing.Self),
    repr(typing.Never),
    repr(typing.NoReturn),
    repr(typing.LiteralString),
))
show_exc_type("Self[int]", lambda: typing.Self[int])
show_exc_type("Never[int]", lambda: typing.Never[int])
show_exc_type("LiteralString[int]", lambda: typing.LiteralString[int])

P = typing.ParamSpec("P")
show("Required/NotRequired/TypeGuard/Concatenate", lambda: (
    repr(typing.Required[int]),
    typing.get_origin(typing.Required[int]) is typing.Required,
    typing.get_args(typing.Required[int]),
    repr(typing.NotRequired[str]),
    typing.get_origin(typing.NotRequired[str]) is typing.NotRequired,
    typing.get_args(typing.NotRequired[str]),
    repr(typing.TypeGuard[int]),
    typing.get_origin(typing.TypeGuard[int]) is typing.TypeGuard,
    typing.get_args(typing.TypeGuard[int]),
    repr(typing.Concatenate[int, P]),
    typing.get_origin(typing.Concatenate[int, P]) is typing.Concatenate,
    typing.get_args(typing.Concatenate[int, P]),
))

show("assert_type", lambda: typing.assert_type(123, int))
show("assert_never", lambda: typing.assert_never("oops"))
_buf = io.StringIO()
_old_err = sys.stderr
sys.stderr = _buf
_rev = typing.reveal_type(42)
sys.stderr = _old_err
show("reveal_type", lambda: (_rev, _buf.getvalue().strip()))


@typing.dataclass_transform(eq_default=False, kw_only_default=True)
class ModelMeta:
    pass


show("dataclass_transform", lambda: ModelMeta.__dataclass_transform__)

typing.clear_overloads()


@typing.overload
def parse_val(x: int) -> int: ...


@typing.overload
def parse_val(x: str) -> str: ...


def parse_val(x):
    return x


_ov_count_1 = len(typing.get_overloads(parse_val))
typing.clear_overloads()
_ov_count_2 = len(typing.get_overloads(parse_val))
show("overloads", lambda: (_ov_count_1, _ov_count_2, parse_val(9)))


class UserTD(typing.TypedDict):
    id: int
    nickname: typing.NotRequired[str]
    tag: typing.Annotated[typing.NotRequired[str], "meta"]


class PartialTD(typing.TypedDict, total=False):
    id: typing.Required[int]
    bio: str


FuncTD = typing.TypedDict("FuncTD", {"a": int, "b": typing.NotRequired[str]})
show("TypedDict + is_typeddict", lambda: (
    typing.is_typeddict(UserTD),
    typing.is_typeddict(PartialTD),
    typing.is_typeddict(FuncTD),
    typing.is_typeddict(dict),
    sorted(UserTD.__required_keys__),
    sorted(UserTD.__optional_keys__),
    sorted(PartialTD.__required_keys__),
    sorted(PartialTD.__optional_keys__),
    sorted(FuncTD.__required_keys__),
    sorted(FuncTD.__optional_keys__),
))


def hinted_fn(a: "LocalAlias", b: typing.Annotated[int, "m"]) -> None:
    pass


show("get_type_hints", lambda: (
    typing.get_type_hints(hinted_fn, localns={"LocalAlias": float}),
    typing.get_type_hints(hinted_fn, localns={"LocalAlias": float}, include_extras=True),
    typing.get_type_hints(UserTD),
    typing.get_type_hints(UserTD, include_extras=True),
))

show("get_origin/get_args Callable & Literal & Annotated", lambda: (
    typing.get_origin(typing.Callable[[int, str], bool]) is collections.abc.Callable,
    typing.get_args(typing.Callable[[int, str], bool]),
    typing.get_origin(typing.Literal[1, "a"]) is typing.Literal,
    typing.get_args(typing.Literal[1, "a"]),
    typing.get_origin(typing.Annotated[int, "x", "y"]) is typing.Annotated,
    typing.get_args(typing.Annotated[int, "x", "y"]),
    typing.get_origin(int | str) is types.UnionType,
    typing.get_args(int | str),
))

UserId = typing.NewType("UserId", int)
show("NewType", lambda: (UserId(42), UserId.__name__, UserId.__supertype__, repr(UserId), typing.get_args(UserId | str)))


@typing.runtime_checkable
class Greeter(typing.Protocol):
    def greet(self) -> str: ...


show("Protocol runtime_checkable", lambda: (isinstance(SubCls(), Greeter), isinstance(123, Greeter)))


# =============================================================================
# 6. functools
# =============================================================================
print("=== functools ===")
show("WRAPPER constants", lambda: (functools.WRAPPER_ASSIGNMENTS, functools.WRAPPER_UPDATES))


@functools.singledispatch
def describe(x):
    return f"other:{x}"


@describe.register
def _(x: int | str):
    return f"int_or_str:{x}"


@describe.register(typing.Union[float, complex])
def _(x):
    return f"num:{x}"


@describe.register
def _(x: None):
    return "none"


show("singledispatch 3.11+ unions", lambda: (
    describe(1),
    describe("a"),
    describe(2.5),
    describe(1 + 2j),
    describe(None),
    describe([1]),
    isinstance(describe.registry, types.MappingProxyType),
))


class FormatterBox:
    def __init__(self, prefix):
        self.prefix = prefix

    @functools.singledispatchmethod
    def fmt(self, arg):
        return f"{self.prefix}:default:{arg}"

    @fmt.register
    def _(self, arg: int | bool):
        return f"{self.prefix}:int_or_bool:{arg}"

    @fmt.register(str)
    def _(self, arg):
        return f"{self.prefix}:str:{arg}"

    @functools.singledispatchmethod
    @classmethod
    def from_val(cls, arg):
        return ("default", cls.__name__, arg)

    @from_val.register
    @classmethod
    def _(cls, arg: int | str):
        return ("int_or_str", cls.__name__, arg)


fb = FormatterBox("P")
show("singledispatchmethod", lambda: (
    fb.fmt(5),
    fb.fmt(True),
    fb.fmt("hi"),
    fb.fmt([1]),
    FormatterBox.from_val(7),
    FormatterBox.from_val("x"),
    FormatterBox.from_val(3.5),
))


class Cell:
    def __init__(self):
        self._alive = False

    def set_state(self, state, *, tag="std"):
        self._alive = state
        return (state, tag)

    set_alive = functools.partialmethod(set_state, True)
    set_dead = functools.partialmethod(set_state, False, tag="off")
    set_dead_custom = functools.partialmethod(set_dead, tag="override")


c = Cell()
show("partialmethod", lambda: (
    c.set_alive(),
    c.set_dead(),
    c.set_dead_custom(),
    Cell.set_alive(c, tag="cls"),
    c.set_dead.func.__name__,
))


class CachedBox:
    def __init__(self, v):
        self.v = v
        self.calls = 0

    @functools.cached_property
    def computed(self):
        """Computed doc."""
        self.calls += 1
        return self.v * 10


cb = CachedBox(3)
v1 = cb.computed
v2 = cb.computed
del cb.computed
v3 = cb.computed
show("cached_property", lambda: (v1, v2, v3, cb.calls, CachedBox.computed.__doc__))


@functools.lru_cache(maxsize=8, typed=True)
def add1(x):
    return x + 1


@functools.cache
def mul2(x):
    return x * 2


show("cache_parameters", lambda: (add1(1), add1.cache_parameters(), mul2(3), mul2.cache_parameters()))


def orig(x):
    """orig doc"""
    return x


@functools.wraps(orig, assigned=("__name__",), updated=())
def wrap_custom(x):
    """wrap doc"""
    return orig(x)


show("wraps assigned/updated", lambda: (wrap_custom.__name__, wrap_custom.__doc__, wrap_custom.__wrapped__ is orig))


# =============================================================================
# 7. string
# =============================================================================
print("=== string ===")
show("Template valid", lambda: (
    string.Template("$a ${b} $$ $a").is_valid(),
    string.Template("$a ${b} $$ $a").get_identifiers(),
))
show("Template invalid skips in get_identifiers", lambda: (
    string.Template("$a $1 ${bad!} $b $").is_valid(),
    string.Template("$a $1 ${bad!} $b $").get_identifiers(),
))


class PercentTemplate(string.Template):
    delimiter = "%%"


show("Template multi-char delimiter", lambda: (
    PercentTemplate("%%a %%%% %%{b} %%1").is_valid(),
    PercentTemplate("%%a %%%% %%{b} %%1").get_identifiers(),
    PercentTemplate("%%a %%%% %%{b}").is_valid(),
))

fmt = string.Formatter()
show("Formatter", lambda: (
    fmt.format("{0.real:.1f} {k[1]!r}", 3 + 4j, k=["a", "b"]),
    list(fmt.parse("hi {x!s:>5}!")),
))


# =============================================================================
# 8. operator
# =============================================================================
print("=== operator ===")
show("operator.call pos/kw", lambda: (
    operator.call(pow, 2, 8),
    operator.call(lambda **kw: sorted(kw.items()), obj=1, other=2),
    operator.call(dict, a=1, b=2),
))
show("operator.itemgetter edge", lambda: (
    operator.itemgetter(slice(1, 3))([10, 20, 30, 40]),
    operator.itemgetter(2, 0)("abc"),
    repr(operator.itemgetter("k")),
    operator.itemgetter(1, 2).__reduce__()[1],
))
show("operator.attrgetter edge", lambda: (
    operator.attrgetter("real", "imag")(3 + 4j),
    repr(operator.attrgetter("a.b")),
    operator.attrgetter("a", "b").__reduce__()[1],
))
show("operator.methodcaller edge", lambda: (
    operator.methodcaller("split", ",", maxsplit=1)("a,b,c"),
    repr(operator.methodcaller("replace", "a", "b")),
))
