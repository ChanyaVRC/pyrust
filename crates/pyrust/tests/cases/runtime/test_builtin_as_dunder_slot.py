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


# NOTE: a genuinely self-recursive *Python* dunder (`def __len__(self): return
# len(self)`) is a separate concern and is deliberately not exercised here.  It
# takes the `UserFunction` slot path, not the builtin path this fixture covers,
# and it still overflows the native stack in an unoptimised build — pyrust's
# recursion budget does not bound it before the real stack runs out.  Asserting it
# would make this fixture abort under the debug builds CI uses; tracked separately.


# Reaching this line is the point of the fixture: the interpreter survived.
print("still alive")
