"""
One representation-substitutability boundary for builtin subclasses (#2386).

A builtin-subclass instance (`class BA(bytearray)`, `class LI(list)`, …) is a
`PyInstance` carrying its base value in a `__builtin_data__` attr.  Every
consumer (method dispatch, iteration, repr/str, operators, contains, slicing,
len, hash, conversions, format) must treat such an instance as its inherited
base — *unless* the subclass overrides the relevant dunder, in which case the
user method wins (CPython subclass-slot semantics).

This consolidates the per-site unwrap shims behind `effective_builtin_receiver`
and is the root-cause fix for the #2324 class (bytearray subclasses broken,
generic reprs, `BA+BA` TypeErrors, `BA == bytearray` False, unhashable keying).

NOTE (carved out — tracked as #2387): builtin dunders are not exposed as
*attributes* on subclasses, so `LI([1]).__iter__()` and `hasattr(bytes,
'__mod__')` diverge from CPython.  That is an attribute-exposure gap orthogonal
to the substitutability routing; not exercised here.
"""


class BA(bytearray):
    pass


class BY(bytes):
    pass


class LI(list):
    pass


class DI(dict):
    pass


class ST(str):
    pass


class IN(int):
    pass


class SE(set):
    pass


class TU(tuple):
    pass


def show(label, fn):
    try:
        print(label, "=", repr(fn()))
    except Exception as e:  # noqa: BLE001
        print(label, "!", type(e).__name__, str(e))


# --- method dispatch, incl. the bytearray ops-table family (#2324) ---
show("ba.upper", lambda: BA(b"abc").upper())
show("ba.find", lambda: BA(b"abc").find(b"b"))
show("ba.replace", lambda: BA(b"abc").replace(b"a", b"X"))
show("ba.hex", lambda: BA(b"ab").hex())
show("ba.split", lambda: BA(b"a,b").split(b","))
show("by.hex", lambda: BY(b"xy").hex())
show("by.split", lambda: BY(b"a-b").split(b"-"))
show("li.copy-type", lambda: type(LI([1, 2]).copy()).__name__)
show("st.upper", lambda: ST("ab").upper())
show("n.bit_length", lambda: IN(5).bit_length())
# return-type semantics: methods on subclass receivers return the BASE type
show("ba.upper-type", lambda: type(BA(b"ab").upper()).__name__)
show("st.upper-type", lambda: type(ST("ab").upper()).__name__)
show("li.copy-base", lambda: type(LI([1]).copy()).__name__)

# --- iteration ---
show("iter(ba)", lambda: list(BA(b"ab")))
show("iter(by)", lambda: list(BY(b"xy")))
show("iter(li)", lambda: list(LI([1, 2])))
show("iter(di)", lambda: sorted(DI(a=1, b=2)))
show("iter(se)", lambda: sorted(SE({1, 2})))
show("iter(tu)", lambda: list(TU((3, 4))))
show("reversed(ba)", lambda: list(reversed(BA(b"abc"))))
show("sum-gen(ba)", lambda: sum(x for x in BA(b"\x01\x02\x03")))

# --- repr / str ---
show("repr(ba)", lambda: repr(BA(b"ab")))
show("repr(ba-empty)", lambda: repr(BA()))
show("str(ba)", lambda: str(BA(b"ab")))
show("fstr(ba)", lambda: f"{BA(b'ab')}")
show("fstr-r(ba)", lambda: f"{BA(b'ab')!r}")
show("repr(by)", lambda: repr(BY(b"xy")))
show("repr(li)", lambda: repr(LI([1, 2])))
show("repr(di)", lambda: repr(DI(a=1)))
show("repr(st)", lambda: repr(ST("ab")))
show("repr(n)", lambda: repr(IN(5)))
show("fstr(se)", lambda: f"{SE({1})}")
show("str(se)", lambda: str(SE({1})))

# --- operators (eval_binary fallback) ---
show("ba+ba", lambda: BA(b"a") + BA(b"b"))
show("ba+bytes", lambda: BA(b"a") + b"b")
show("bytes+ba", lambda: b"a" + BA(b"b"))
show("ba*2", lambda: BA(b"ab") * 2)
show("by*2", lambda: BY(b"xy") * 2)
show("li+li", lambda: LI([1]) + LI([2]))
show("li*2", lambda: LI([1, 2]) * 2)
show("st+st", lambda: ST("a") + ST("b"))
show("n+1", lambda: IN(5) + 1)
show("tu+tu", lambda: TU((1,)) + TU((2,)))
show("ba+bytes-type", lambda: type(BA(b"a") + b"cd").__name__)
show("ba==bytearray", lambda: BA(b"ab") == bytearray(b"ab"))
show("ba==bytes", lambda: BA(b"ab") == b"ab")
show("ba<ba", lambda: BA(b"ab") < BA(b"ac"))
show("ba>=ba", lambda: BA(b"ab") >= BA(b"ab"))
show("by==b", lambda: BY(b"xy") == b"xy")
show("n==5", lambda: IN(5) == 5)
show("sorted-ba", lambda: sorted([BA(b"c"), BA(b"a"), BA(b"b")]))

# --- contains / index / slice / len / bool ---
show("98 in ba", lambda: 98 in BA(b"ab"))
show("ba[0]", lambda: BA(b"ab")[0])
show("ba[0:1]", lambda: BA(b"ab")[0:1])
show("ba[::-1]", lambda: BA(b"abc")[::-1])
show("li[0:1]-type", lambda: type(LI([1, 2])[0:1]).__name__)
show("len(ba)", lambda: len(BA(b"abc")))
show("len(li)", lambda: len(LI([1, 2])))
show("bool(ba)", lambda: bool(BA()))

# --- hash / unhashable keying ---
show("hash(by)==", lambda: hash(BY(b"xy")) == hash(b"xy"))
show("hash(st)==", lambda: hash(ST("ab")) == hash("ab"))
show("hash(n)==", lambda: hash(IN(5)) == hash(5))
show("hash(ba)", lambda: hash(BA(b"a")))
show("set{ba}", lambda: {BA(b"a")})
show("dict{ba}", lambda: {BA(b"a"): 1})
show("by-as-key", lambda: {BY(b"xy"): 1}[b"xy"])
show("st-as-key", lambda: {ST("ab"): 1}["ab"])

# --- conversions ---
show("bytes(ba)", lambda: bytes(BA(b"ab")))
show("bytes(ba)-type", lambda: type(bytes(BA(b"ab"))).__name__)
show("list(li)", lambda: list(LI([1, 2])))
show("int(n)-type", lambda: type(int(IN(5))).__name__)
show("str(st)-type", lambda: type(str(ST("ab"))).__name__)

# --- format / percent ---
show("st%", lambda: ST("%d") % 3)
show("st.format", lambda: ST("{}").format(1))
# PR #2380: a markup-free str.format on a subclass preserves subclass identity.
show("st.format-id", lambda: type(ST("hi").format()).__name__)

# --- isinstance / mro / type ---
show("isinstance", lambda: (isinstance(BA(b""), bytearray), isinstance(BY(b""), bytes), isinstance(IN(0), int)))
show("mro", lambda: [c.__name__ for c in BA.__mro__])
show("type-name", lambda: (type(BA(b"")).__name__, type(BY(b"")).__name__))

# --- join consumers ---
show("join-st", lambda: ",".join([ST("a"), ST("b")]))
show("b-join", lambda: b"-".join([BY(b"x"), BY(b"y")]))
show("ba-join", lambda: bytes(BA(b",").join([b"a", b"b"])))


# ==================== OVERRIDE-WINS BATTERY ====================
# A user dunder/method override must win over the inherited builtin behaviour.
class IterOver(list):
    def __iter__(self):
        return iter([x * 10 for x in [1, 2, 3]])


class ReprOver(bytearray):
    def __repr__(self):
        return "CUSTOM_REPR"


class StrOver(bytearray):
    def __str__(self):
        return "CUSTOM_STR"


class EqOver(list):
    def __eq__(self, other):
        return "EQ_CALLED"

    __hash__ = list.__hash__


class AddOver(list):
    def __add__(self, other):
        return "ADD_CALLED"


class GetItemOver(list):
    def __getitem__(self, i):
        return "GI_CALLED"


class ContainsOver(list):
    def __contains__(self, x):
        return True


class LenOver(list):
    def __len__(self):
        return 999


class MethodOver(bytearray):
    def upper(self):
        return "UP_CALLED"


show("ov-iter", lambda: list(IterOver([1, 2, 3])))
show("ov-repr", lambda: repr(ReprOver(b"ab")))
show("ov-str", lambda: str(StrOver(b"ab")))
show("ov-fstr", lambda: f"{StrOver(b'ab')}")
show("ov-eq", lambda: EqOver([1]) == [1])
show("ov-add", lambda: AddOver([1]) + [2])
show("ov-getitem", lambda: GetItemOver([1, 2])[0])
show("ov-contains", lambda: 99 in ContainsOver([1, 2]))
show("ov-len", lambda: len(LenOver([1, 2])))
show("ov-method", lambda: MethodOver(b"ab").upper())
