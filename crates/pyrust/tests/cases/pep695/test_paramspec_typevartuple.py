# PEP 695 ParamSpec ([**P]) and TypeVarTuple ([*Ts]) parity tests (issue #3144).
import sys
import typing
from typing import (
    Callable,
    Generic,
    ParamSpec,
    ParamSpecArgs,
    ParamSpecKwargs,
    Protocol,
    TypeAliasType,
    TypeVar,
    TypeVarTuple,
    Unpack,
    get_args,
    get_origin,
)

# ── 1. Generic functions, async functions, classes, and type aliases ──────────

def fn_ps[**P](*args: P.args, **kwargs: P.kwargs):
    return (args, kwargs)

def fn_tvt[*Ts](*args: *Ts) -> tuple[*Ts]:
    return args

def fn_mixed[T: int, *Ts, **P](x: T, *args: *Ts) -> tuple[T, *Ts]:
    return (x, *args)

async def afn_mixed[**P, *Ts](*args: P.args, **kwargs: P.kwargs) -> tuple[*Ts]:
    return args

class ClsPS[**P]:
    def __init__(self, v=1):
        self.v = v

class ClsTVT[*Ts]:
    def __init__(self, v=2):
        self.v = v

class ClsMixed[T, *Ts, **P]:
    def get_params(self):
        return (T, Ts, P)

type AliasPS[**P] = Callable[P, int]
type AliasTVT[*Ts] = tuple[*Ts]
type AliasMixed[T, *Ts, **P] = Callable[P, tuple[T, *Ts]]

# Trailing commas in type parameter lists
def fn_trailing[**P,](): pass
class ClsTrailing[*Ts,]: pass
type AliasTrailing[T, *Ts, **P,] = tuple[T, *Ts]

for label, obj in [
    ("fn_ps", fn_ps),
    ("fn_tvt", fn_tvt),
    ("fn_mixed", fn_mixed),
    ("afn_mixed", afn_mixed),
    ("ClsPS", ClsPS),
    ("ClsTVT", ClsTVT),
    ("ClsMixed", ClsMixed),
    ("AliasPS", AliasPS),
    ("AliasTVT", AliasTVT),
    ("AliasMixed", AliasMixed),
    ("fn_trailing", fn_trailing),
    ("ClsTrailing", ClsTrailing),
    ("AliasTrailing", AliasTrailing),
]:
    tps = obj.__type_params__
    print(label, tps, [type(p).__name__ for p in tps], [repr(p) for p in tps])

# Runtime execution of generic functions
print("fn_ps call:", fn_ps(1, 2, a=3))
print("fn_tvt call:", fn_tvt("x", "y"))
print("fn_mixed call:", fn_mixed(10, "a", "b"))

# ── 2. Exact ParamSpec / TypeVarTuple attributes and object identity ──────────

T_m, Ts_m, P_m = fn_mixed.__type_params__
print(
    "fn_mixed types:",
    type(T_m) is TypeVar,
    type(Ts_m) is TypeVarTuple,
    type(P_m) is ParamSpec,
)
print(
    "P_m attrs:",
    P_m.__name__,
    P_m.__bound__,
    P_m.__covariant__,
    P_m.__contravariant__,
    P_m.__infer_variance__,
    P_m.__module__,
    repr(P_m),
)
print("Ts_m attrs:", Ts_m.__name__, Ts_m.__module__, repr(Ts_m))
print("T_m bound:", T_m.__bound__)

P_fn = fn_ps.__type_params__[0]
print("fn_ps annotations:", fn_ps.__annotations__)
print("fn_ps args origin identity:", fn_ps.__annotations__["args"].__origin__ is P_fn)
print("fn_ps kwargs origin identity:", fn_ps.__annotations__["kwargs"].__origin__ is P_fn)
print("P_fn.args == annot:", P_fn.args == fn_ps.__annotations__["args"])
print("P_fn.kwargs == annot:", P_fn.kwargs == fn_ps.__annotations__["kwargs"])
print("P_fn.args repr:", repr(P_fn.args), repr(P_fn.kwargs))

Ts_fn = fn_tvt.__type_params__[0]
print("fn_tvt annotations:", fn_tvt.__annotations__)
print("fn_tvt args is Unpack[Ts]:", fn_tvt.__annotations__["args"] == Unpack[Ts_fn])
print("fn_tvt return args:", fn_tvt.__annotations__["return"].__args__)
print("fn_tvt return params:", fn_tvt.__annotations__["return"].__parameters__)
print("fn_tvt param identity:", fn_tvt.__annotations__["return"].__parameters__[0] is Ts_fn)

# Method closure captures class type parameters with exact identity
cm = ClsMixed()
c_T, c_Ts, c_P = cm.get_params()
print(
    "ClsMixed identity:",
    (c_T, c_Ts, c_P) == ClsMixed.__type_params__,
    c_T is ClsMixed.__type_params__[0],
    c_Ts is ClsMixed.__type_params__[1],
    c_P is ClsMixed.__type_params__[2],
)

# ── 3. Unpack and TypeVarTuple iteration / unpacking ──────────────────────────

u_ts = Unpack[Ts_fn]
print("Unpack repr:", repr(u_ts))
print("Unpack origin/args:", get_origin(u_ts) is Unpack, get_args(u_ts) == (Ts_fn,))
print("Unpack params:", u_ts.__parameters__ == (Ts_fn,))
print("Unpack eq/hash:", u_ts == Unpack[Ts_fn], hash(u_ts) == hash(Unpack[Ts_fn]))
print("list(Ts):", list(Ts_fn), list(Ts_fn)[0] == u_ts)
print("tuple[int, *Ts, str]:", repr(tuple[int, *Ts_fn, str]))
print("tuple[int, *Ts, str] params:", tuple[int, *Ts_fn, str].__parameters__)

# Starred *args annotation with custom iterable and error cases
class SingleAnnot:
    def __iter__(self):
        return iter(["custom_annot"])

sa = SingleAnnot()
def fn_custom_star(*args: *sa):
    pass
print("custom star annot:", fn_custom_star.__annotations__)

for bad_iter in [(1, 2), ()]:
    try:
        def _bad(*args: *bad_iter):
            pass
    except ValueError as e:
        print("bad star annot ValueError:", e)

# PEP 563 future annotations with *Ts
fut_ns = {}
exec(
    "from __future__ import annotations\n"
    "def g[*Ts](*args: *Ts) -> tuple[int, *Ts]: pass\n"
    "def h[*Ts]() -> tuple[*Ts]: pass\n",
    fut_ns,
)
print("future g:", fut_ns["g"].__annotations__)
print("future h:", fut_ns["h"].__annotations__)

# ── 4. TypeAliasType value, subscript, and __getitem__ ────────────────────────

print("AliasPS value:", AliasPS.__value__)
print("AliasPS[int, str]:", repr(AliasPS[int, str]), AliasPS[int, str].__args__)
print("AliasPS[[int, str]]:", repr(AliasPS[[int, str]]), AliasPS[[int, str]].__args__)
print("AliasPS.__getitem__:", repr(AliasPS.__getitem__((int, str))))

print("AliasTVT value:", AliasTVT.__value__)
print("AliasTVT[int, str]:", repr(AliasTVT[int, str]), AliasTVT[int, str].__args__)
print("AliasTVT[()]:", repr(AliasTVT[()]), AliasTVT[()].__args__)

print("AliasMixed value:", AliasMixed.__value__)
print("AliasMixed[int, str, [bool]]:", repr(AliasMixed[int, str, [bool]]))

# Manual TypeAliasType with ParamSpec & TypeVarTuple
P_man = ParamSpec("P_man")
Ts_man = TypeVarTuple("Ts_man")
ManualAlias = TypeAliasType("ManualAlias", Callable[P_man, tuple[*Ts_man]], type_params=(P_man, Ts_man))
print("ManualAlias:", ManualAlias, ManualAlias.__type_params__, ManualAlias.__value__)

# ── 5. Generic class subscripting and typing.Generic interaction ──────────────

print("ClsPS issubclass Generic:", issubclass(ClsPS, Generic), ClsPS.__bases__)
print("ClsTVT issubclass Generic:", issubclass(ClsTVT, Generic), ClsTVT.__bases__)
print("ClsMixed issubclass Generic:", issubclass(ClsMixed, Generic), ClsMixed.__bases__)

print("ClsPS[int, str]:", repr(ClsPS[int, str]), ClsPS[int, str].__args__)
print("ClsPS[[int, str]]:", repr(ClsPS[[int, str]]), ClsPS[[int, str]].__args__)
print("ClsPS[int]:", repr(ClsPS[int]), ClsPS[int].__args__)
print("ClsPS[[]]:", repr(ClsPS[[]]), ClsPS[[]].__args__)
print("ClsPS[...]:", repr(ClsPS[...]), ClsPS[...].__args__)
print("ClsPS[P_man]:", repr(ClsPS[P_man]), ClsPS[P_man].__args__)
print("ClsPS[int, str]() inst:", ClsPS[int, str](99).v)
try:
    ClsPS[()]
except TypeError as e:
    print("ClsPS[()] TypeError:", e)

print("ClsTVT[int]:", repr(ClsTVT[int]), ClsTVT[int].__args__)
print("ClsTVT[int, str]:", repr(ClsTVT[int, str]), ClsTVT[int, str].__args__)
print("ClsTVT[()]:", repr(ClsTVT[()]), ClsTVT[()].__args__)
print("ClsTVT[int, str]() inst:", ClsTVT[int, str](88).v)

print("ClsMixed[int, str, bool, [float]]:", repr(ClsMixed[int, str, bool, [float]]), ClsMixed[int, str, bool, [float]].__args__)
print("ClsMixed[int, ...]:", repr(ClsMixed[int, ...]), ClsMixed[int, ...].__args__)

# Generic[...] with ParamSpec and TypeVarTuple
print("Generic[P_man]:", repr(Generic[P_man]), Generic[P_man].__args__)
print("Generic[*Ts_man]:", repr(Generic[*Ts_man]), Generic[*Ts_man].__args__)
T_man = TypeVar("T_man")
print("Generic[T_man, *Ts_man, P_man]:", repr(Generic[T_man, *Ts_man, P_man]))

# PEP 695 class inheriting from plain base, Protocol, and conflicting Generic
class BasePlain:
    pass

class SubPlain[T, **P](BasePlain):
    pass

print("SubPlain bases:", SubPlain.__bases__, issubclass(SubPlain, BasePlain), issubclass(SubPlain, Generic))

class MyProto[*Ts](Protocol):
    pass

print("MyProto bases:", MyProto.__bases__, issubclass(MyProto, Protocol), issubclass(MyProto, Generic))

try:
    class BadGen1[**P](Generic[P]):
        pass
except TypeError as e:
    print("BadGen1 TypeError:", e)

try:
    class BadGen2[*Ts](Generic):
        pass
except TypeError as e:
    print("BadGen2 TypeError:", e)

# ── 6. ParamSpec manual construction, union, non-subclassable, read-only ──────

for kw in [{}, {"covariant": True}, {"contravariant": True}, {"infer_variance": True}, {"bound": int}]:
    p = ParamSpec("Q", **kw)
    print("manual ParamSpec:", repr(p), p.__bound__, p.__covariant__, p.__contravariant__, p.__infer_variance__, p.__module__)

print("P_man | int:", repr(P_man | int))
print("int | P_man:", repr(int | P_man))

for bad_call in [
    lambda: ParamSpec("Q", covariant=True, contravariant=True),
    lambda: ParamSpec("Q", covariant=True, infer_variance=True),
    lambda: ParamSpec(123),
    lambda: ParamSpec("Q", int),
    lambda: TypeVarTuple(123),
    lambda: TypeVarTuple("Ts", int),
]:
    try:
        bad_call()
    except (TypeError, ValueError) as e:
        print("manual err:", type(e).__name__, e)

for cls in (ParamSpec, TypeVarTuple, ParamSpecArgs, ParamSpecKwargs):
    try:
        class _Sub(cls):
            pass
    except TypeError as e:
        print("non-subclassable:", cls.__name__, e)

for attr in ("__name__", "__bound__", "__covariant__", "__contravariant__", "__infer_variance__", "args", "kwargs"):
    try:
        setattr(P_m, attr, "x")
    except AttributeError as e:
        print("RO ParamSpec:", attr, e)

try:
    setattr(Ts_m, "__name__", "x")
except AttributeError as e:
    print("RO TypeVarTuple:", e)

try:
    setattr(P_m.args, "__origin__", P_man)
except AttributeError as e:
    print("RO ParamSpecArgs:", e)

# ── 7. Scope isolation (no leakage / no clobbering) ───────────────────────────

OUTER_P = "outer_p"
OUTER_TS = "outer_ts"

def scoped[**OUTER_P, *OUTER_TS]():
    return (OUTER_P.__name__, OUTER_TS.__name__)

print("scoped call:", scoped(), OUTER_P, OUTER_TS)

def no_leak[**LP, *LTS]():
    pass

for missing in ("LP", "LTS"):
    try:
        eval(missing)
    except NameError:
        print("no leak:", missing)

# ── 8. Canonical class identity across typing reload ──────────────────────────

old_ps, old_tvt, old_psa, old_pskw = ParamSpec, TypeVarTuple, ParamSpecArgs, ParamSpecKwargs
del sys.modules["typing"]
import typing as typing2

print(
    "canonical reload:",
    old_ps is typing2.ParamSpec,
    old_tvt is typing2.TypeVarTuple,
    old_psa is typing2.ParamSpecArgs,
    old_pskw is typing2.ParamSpecKwargs,
    type(P_m) is typing2.ParamSpec,
    type(Ts_m) is typing2.TypeVarTuple,
    type(P_m.args) is typing2.ParamSpecArgs,
    type(P_m.kwargs) is typing2.ParamSpecKwargs,
)

# ── 9. SyntaxError cases ──────────────────────────────────────────────────────

def check_syntax_err(src, check_msg=True):
    try:
        compile(src, "<test>", "exec")
        print("UNEXPECTED OK:", src)
    except SyntaxError as e:
        if check_msg:
            print("SyntaxError:", src, "->", e.msg)
        else:
            print("SyntaxError:", src)

# Bounds/constraints on ParamSpec / TypeVarTuple (exact message parity)
for src in [
    "def f[**P: int](): pass",
    "def f[**P: (int, str)](): pass",
    "def f[**P: ()](): pass",
    "def f[*Ts: int](): pass",
    "def f[*Ts: (int, str)](): pass",
    "def f[*Ts: ()](): pass",
    "async def f[**P: int](): pass",
    "async def f[*Ts: int](): pass",
    "class C[**P: int]: pass",
    "class C[*Ts: (int, str)]: pass",
    "type A[**P: int] = int",
    "type A[*Ts: int] = int",
    "type A[*Ts: (int, str)] = int",
    # Duplicate type parameter names across kinds
    "def f[T, *T](): pass",
    "def f[T, **T](): pass",
    "def f[*Ts, **Ts](): pass",
    "class C[*Ts, *Ts]: pass",
    "type A[**P, **P] = int",
]:
    check_syntax_err(src, check_msg=True)

# Malformed type parameter lists & invalid starred annotations (exact parser error phrasing may vary)
for src in [
    "def f[](): pass",
    "class C[]: pass",
    "type A[] = int",
    "def f[*](): pass",
    "def f[**](): pass",
    "def f[***P](): pass",
    "def f[, T](): pass",
    "def f[T, , U](): pass",
    "def f[*Ts](x: *Ts): pass",
    "def f[*Ts](**kwargs: *Ts): pass",
    "def f[*Ts]() -> *Ts: pass",
]:
    check_syntax_err(src, check_msg=False)

print("pep695 paramspec & typevartuple ok")
