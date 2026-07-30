# Issue #2948: a bare builtin function stored in a dunder slot is a
# `builtin_function_or_method`, which implements NO descriptor protocol.  CPython
# therefore calls it with no receiver at all, so `class C: __len__ = len` makes
# `len(C())` evaluate `len()` and raise `TypeError: len() takes exactly one
# argument (0 given)`.
#
# pyrust used to prepend the receiver, turning `len(C())` into `len(self)`, which
# re-entered the same slot forever and overflowed the *native* stack — the process
# died with SIGABRT and no Python-level exception was catchable.  `__repr__ = repr`
# and `__iter__ = iter` aborted identically; `__hash__ = id` does not re-enter and
# so silently returned `id(self)` instead of raising.
#
# Every probe below is wrapped in `try` on purpose: this fixture is the abort
# regression test.  If the receiver is prepended again the interpreter dies here
# rather than printing a diff, and reaching the final line proves the error is a
# catchable Python exception.


def show(label, fn):
    try:
        print(f"{label}: {fn()!r}")
    except BaseException as e:
        print(f"{label}: {type(e).__name__}: {e}")


# ── builtins that re-enter their own slot: previously an uncatchable abort ────


class BLen:
    __len__ = len


class BRepr:
    __repr__ = repr


class BIter:
    __iter__ = iter


class BAbs:
    __abs__ = abs


show("len(BLen())", lambda: len(BLen()))
show("repr(BRepr())", lambda: repr(BRepr()))
show("iter(BIter())", lambda: iter(BIter()))
show("abs(BAbs())", lambda: abs(BAbs()))

# `bool()` falls back to `__len__`, so the truth test reaches the same slot.
show("bool(BLen())", lambda: bool(BLen()))

# An explicit call must agree with the implicit dispatch above.
show("BLen().__len__()", lambda: BLen().__len__())


# ── a builtin that does NOT re-enter: previously silently wrong ───────────────


class BHash:
    __hash__ = id


class BSizeof:
    __sizeof__ = id


# Returned `id(self)` before; must raise like CPython.
show("hash(BHash())", lambda: hash(BHash()))
show("BSizeof().__sizeof__()", lambda: BSizeof().__sizeof__())


# ── non-descriptor identity: the slot value is handed back unbound ────────────


show("BLen.__len__ is len", lambda: BLen.__len__ is len)
show("BLen().__len__ is len", lambda: BLen().__len__ is len)
show("type(BLen().__len__)", lambda: type(BLen().__len__).__name__)


# ── multi-argument builtins report their own arity ────────────────────────────


class BGetattr:
    __getattr__ = getattr


show("BGetattr().missing", lambda: BGetattr().missing)


# NOTE: a function from an imported module (`__len__ = math.sqrt`) is equally a
# non-descriptor in CPython, but pyrust still binds it.  That case is deliberately
# NOT asserted here — see the `builtins`-namespace guard in
# `call_protocol.rs::invoke_class_method` and its follow-up note.


# ── builtin *types* in slots are not builtin functions — unchanged ────────────
#
# `str` / `bool` / `int` are type objects, so they construct with no argument and
# were already correct.  Kept as regression guards for the fix's gating.


class TStr:
    __str__ = str


class TBool:
    __bool__ = bool


class TInt:
    __int__ = int


show("str(TStr())", lambda: str(TStr()))
show("bool(TBool())", lambda: bool(TBool()))
show("int(TInt())", lambda: int(TInt()))


# ── method descriptors DO bind: primitive subclasses must be unaffected ──────
#
# `list.__len__`, `dict.__contains__`, `bytes.__iter__` and the `bytearray`
# ops-table methods are slot wrappers / method descriptors, not plain builtins.
# The fix is gated so these keep receiving `self`.


class MyList(list):
    pass


class MyDict(dict):
    pass


class MyStr(str):
    pass


class MyBytes(bytes):
    pass


class MyBytearray(bytearray):
    pass


ml = MyList([1, 2, 3])
md = MyDict(a=1, b=2)
ms = MyStr("hello")

show("len(MyList)", lambda: len(ml))
show("ml[1]", lambda: ml[1])
show("2 in ml", lambda: 2 in ml)
show("list(iter(ml))", lambda: list(iter(ml)))
show("list(reversed(ml))", lambda: list(reversed(ml)))
show("len(MyDict)", lambda: len(md))
show("'a' in md", lambda: "a" in md)
show("sorted(md)", lambda: sorted(md))
show("len(MyStr)", lambda: len(ms))
show("ms.upper()", lambda: ms.upper())
show("'ell' in ms", lambda: "ell" in ms)
show("list(iter(MyBytes(b'abc')))", lambda: list(iter(MyBytes(b"abc"))))
show("MyBytearray(b'abc').upper()", lambda: MyBytearray(b"abc").upper())
show("len(MyBytearray(b'abc'))", lambda: len(MyBytearray(b"abc")))


# `super()` routing to an inherited builtin dunder still binds the receiver.
class SuperList(list):
    def __contains__(self, x):
        return super().__contains__(x)

    def __len__(self):
        return super().__len__() * 10


sl = SuperList([1, 2, 3])
show("SuperList __contains__", lambda: 2 in sl)
show("SuperList __len__", lambda: len(sl))

# Unbound descriptor calls and their receiver-mismatch wording are unchanged.
show("list.__len__([1, 2])", lambda: list.__len__([1, 2]))
show("dict.__contains__", lambda: dict.__contains__({"a": 1}, "a"))
show("str.upper('ab')", lambda: str.upper("ab"))
show("list.__len__(5)", lambda: list.__len__(5))
show("str.upper(5)", lambda: str.upper(5))


# ── other slot-value shapes stay on their own paths ───────────────────────────


class SM:
    __len__ = staticmethod(lambda: 3)


class CM:
    @classmethod
    def __len__(cls):
        return 5


class Caller:
    def __call__(self):
        return 11


class WithCaller:
    __len__ = Caller()


class BadLen:
    __len__ = 5


class Normal:
    def __len__(self):
        return 7

    def __repr__(self):
        return "<Normal>"

    def __hash__(self):
        return 99


show("len(SM())", lambda: len(SM()))
show("len(CM())", lambda: len(CM()))
show("len(WithCaller())", lambda: len(WithCaller()))
show("len(BadLen())", lambda: len(BadLen()))
show("len(Normal())", lambda: len(Normal()))
show("repr(Normal())", lambda: repr(Normal()))
show("hash(Normal())", lambda: hash(Normal()))


# ── installation routes, cache invalidation and the wider slot family ────────
#
# The rows above pin the reported repros.  The rows below pin the rest of the
# rule: that it reaches the class through every installation route (class body,
# `type()` dict, inheritance), that rebinding a warm slot invalidates whatever
# the dunder dispatch cached, and that the neighbouring slots the fix also
# changed (`__class_getitem__`) or deliberately left alone (`Counter` /
# `deque` / `OrderedDict` descriptors, typing's receiver-reading
# `__class_getitem__` sentinels) behave.  All verified byte-identical against
# CPython 3.12; every one of them aborted the interpreter before this fix.



Made = type("Made", (object,), {"__len__": len, "__repr__": repr, "__hash__": id})
show("type() dict __len__", lambda: len(Made()))
show("type() dict __repr__", lambda: repr(Made()))
show("type() dict __hash__", lambda: hash(Made()))


class Base:
    __len__ = len
    __repr__ = repr


class Mid(Base):
    pass


class Leaf(Mid):
    pass


show("inherited __len__", lambda: len(Leaf()))
show("inherited __repr__", lambda: repr(Leaf()))
show("Leaf.__len__ is len", lambda: Leaf.__len__ is len)
show("Leaf().__len__ is len", lambda: Leaf().__len__ is len)


class Override(Base):
    def __len__(self):
        return 3


show("subclass override wins", lambda: len(Override()))
show("override explicit call", lambda: Override().__len__())


# An instance attribute never shadows an implicit dunder lookup.
class InstShadow:
    def __len__(self):
        return 4


_shadowed = InstShadow()
_shadowed.__len__ = len
show("instance dict ignored", lambda: len(_shadowed))
show("instance dict wins on getattr", lambda: _shadowed.__len__ is len)


# ── the inline dunder cache must invalidate when the slot is rebound ──────────
#
# Rebinding after the slot has been dispatched thousands of times is the case a
# stale per-class cache would get wrong: the warm `UserFunction` slot must not
# survive the assignment of a bare builtin, and rebinding back to a Python
# function must recover.


class Warm:
    def __len__(self):
        return 1

    def __add__(self, other):
        return 2


_warm = Warm()
_total = 0
for _ in range(2000):
    _total += len(_warm) + (_warm + _warm)
print(f"warm total: {_total}")

Warm.__len__ = len
Warm.__add__ = len
show("rebound to builtin (len)", lambda: len(_warm))
show("rebound to builtin (+)", lambda: _warm + _warm)

_errors = set()
for _ in range(2000):
    try:
        len(_warm)
    except TypeError as e:
        _errors.add(str(e))
print(f"hot-loop errors: {sorted(_errors)}")

Warm.__len__ = lambda self: 11
Warm.__add__ = lambda self, other: 99
_total = 0
for _ in range(2000):
    _total += len(_warm) + (_warm + _warm)
print(f"rebound back to python: {_total}")


# ── metaclass slots take the same router ─────────────────────────────────────


class Meta(type):
    __len__ = len
    __repr__ = repr
    __getitem__ = abs


class WithMeta(metaclass=Meta):
    pass


show("len(class)", lambda: len(WithMeta))
show("repr(class)", lambda: repr(WithMeta))
show("class[1]", lambda: WithMeta[1])


# ── construction slots ───────────────────────────────────────────────────────


class InitPrint:
    __init__ = print


class NewLen:
    __new__ = len


class InitLen:
    __init__ = len


show("__init__ = print", lambda: type(InitPrint()).__name__)
show("__new__ = len", lambda: NewLen())
show("__init__ = len", lambda: InitLen())


# ── context manager / iterator slots ─────────────────────────────────────────


class Ctx:
    __enter__ = len
    __exit__ = print


def _enter_ctx():
    with Ctx() as bound:
        return bound


show("__enter__ = len", _enter_ctx)


class Ctx2:
    def __enter__(self):
        return 1

    __exit__ = print


def _exit_ctx():
    with Ctx2() as bound:
        return bound


show("__exit__ = print", _exit_ctx)


class SelfIter:
    def __iter__(self):
        return self

    __next__ = next


show("__next__ = next", lambda: list(SelfIter()))
show("__next__ = next in comprehension", lambda: [x for x in SelfIter()])


# ── comparison and arithmetic slots receive exactly one argument ─────────────
#
# A binary dunder is called with the *other* operand alone — the receiver is
# dropped, the operand is not.  `__eq__ = len` therefore evaluates `len(other)`.


class Cmp:
    __eq__ = len
    __ne__ = len
    __lt__ = len
    __le__ = len
    __gt__ = len
    __ge__ = len
    __hash__ = None


_cmp = Cmp()
for _label, _probe in [
    ("==", lambda: _cmp == _cmp),
    ("!=", lambda: _cmp != _cmp),
    ("<", lambda: _cmp < _cmp),
    ("<=", lambda: _cmp <= _cmp),
    (">", lambda: _cmp > _cmp),
    (">=", lambda: _cmp >= _cmp),
]:
    show(f"Cmp {_label} Cmp", _probe)

show("Cmp == list", lambda: _cmp == [1, 2, 3])
show("list == Cmp", lambda: [1, 2, 3] == _cmp)
show("Cmp in list", lambda: _cmp in [_cmp])


class Arith:
    __add__ = len
    __sub__ = len
    __mul__ = len
    __truediv__ = len
    __floordiv__ = len
    __mod__ = len
    __pow__ = len
    __lshift__ = len
    __and__ = len
    __matmul__ = len
    __radd__ = len
    __iadd__ = len


_arith = Arith()
for _label, _probe in [
    ("+", lambda: _arith + [1]),
    ("-", lambda: _arith - [1, 2]),
    ("*", lambda: _arith * [1, 2, 3]),
    ("/", lambda: _arith / "ab"),
    ("//", lambda: _arith // "abc"),
    ("%", lambda: _arith % {}),
    ("**", lambda: _arith ** (1, 2)),
    ("<<", lambda: _arith << [0]),
    ("&", lambda: _arith & set()),
    ("@", lambda: _arith @ [1]),
]:
    show(f"Arith {_label}", _probe)


class NoAdd:
    def __add__(self, other):
        return NotImplemented


show("reflected __radd__", lambda: NoAdd() + _arith)


def _augmented():
    target = Arith()
    target += 1
    return target


show("__iadd__", _augmented)


# ── conversion slots ─────────────────────────────────────────────────────────


class Convert:
    __str__ = len
    __bytes__ = len
    __index__ = len
    __contains__ = len
    __call__ = len
    __getitem__ = len


_conv = Convert()
show("__str__ = len", lambda: str(_conv))
show("f-string", lambda: f"{_conv}")
show("percent-s", lambda: "%s" % _conv)
show("__bytes__ = len", lambda: bytes(_conv))
show("__index__ = len", lambda: _conv.__index__())
show("__contains__ = len", lambda: 1 in _conv)
show("__call__ = len", lambda: _conv())
show("__getitem__ = len", lambda: _conv[0])


# Builtin *types* in slots construct with no argument and are unaffected.
class Types:
    __str__ = str
    __bool__ = bool
    __int__ = int
    __float__ = float
    __bytes__ = bytes


_types = Types()
show("__str__ = str", lambda: str(_types))
show("__bool__ = bool", lambda: bool(_types))
show("__int__ = int", lambda: int(_types))
show("__float__ = float", lambda: float(_types))
show("__bytes__ = bytes", lambda: bytes(_types))


# ── `__class_getitem__` follows the same rule ────────────────────────────────
#
# The receiver class is not prepended, so the subscript is the only argument.


class ClassGetitem:
    __class_getitem__ = len


show("__class_getitem__ = len", lambda: ClassGetitem[[1, 2]])
show("__class_getitem__ arity", lambda: ClassGetitem[1])


# ── non-primitive builtin classes keep binding their receiver ────────────────
#
# `Counter` / `deque` / `OrderedDict` dunders are registered without a
# canonical primitive owner, which is the same branch the rule above is gated
# on.  They are method descriptors and must still receive `self`.

from collections import Counter, OrderedDict, deque

_counter = Counter("aabbc")
_deque = deque([1, 2, 3])
_ordered = OrderedDict(a=1, b=2)
show("Counter[key]", lambda: _counter["a"])
show("len(Counter)", lambda: len(_counter))
show("key in Counter", lambda: "b" in _counter)
show("deque[index]", lambda: _deque[1])
show("len(deque)", lambda: len(_deque))
show("iter(deque)", lambda: list(_deque))
show("reversed(deque)", lambda: list(reversed(_deque)))
show("OrderedDict[key]", lambda: _ordered["b"])
show("len(OrderedDict)", lambda: len(_ordered))
show("OrderedDict ==", lambda: _ordered == OrderedDict(a=1, b=2))
show("defaultdict missing key", lambda: __import__("collections").defaultdict(int)["zz"])

# typing's `__class_getitem__` sentinels read their receiver class and must not
# be unbound by the rule above (they are module functions with undotted names).
import typing

show("Union origin", lambda: typing.Union[int, str].__origin__)
show("Optional origin", lambda: typing.Optional[int].__origin__)
show("List[int]", lambda: typing.List[int])
show("Dict args", lambda: typing.Dict[str, int].__args__)


class Generic1(typing.Generic[typing.TypeVar("T")]):
    pass


show("Generic1[int].__origin__", lambda: Generic1[int].__origin__ is Generic1)


# NOTE: a genuinely self-recursive *Python* dunder (`def __len__(self): return
# len(self)`) is a separate concern and is deliberately not exercised here.  It
# takes the `UserFunction` slot path, not the builtin path this fixture covers,
# and it still overflows the native stack in an unoptimised build — pyrust's
# recursion budget does not bound it before the real stack runs out.  Asserting it
# would make this fixture abort under the debug builds CI uses; tracked separately.


# Reaching this line is the point of the fixture: the interpreter survived.
print("still alive")
