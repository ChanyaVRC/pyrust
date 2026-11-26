# Parity test (CPython 3.12.3): class patterns whose keyword (or positional)
# attribute is missing fail to match instead of raising AttributeError (#3366).

# --- Builtin types with a missing keyword attribute ---
def classify(subj):
    match subj:
        case dict(a=1):
            return "dict"
        case list(foo=1):
            return "list"
        case str(bar=1):
            return "str"
        case int(baz=1):
            return "int"
        case tuple(qux=1):
            return "tuple"
        case set(x=1):
            return "set"
        case bytes(y=1):
            return "bytes"
        case float(z=1):
            return "float"
        case _:
            return "no " + type(subj).__name__


results = [classify(s) for s in [{"a": 1}, [1, 2], "hi", 42, (1,), {1}, b"x", 3.5]]
assert results == [
    "no dict", "no list", "no str", "no int",
    "no tuple", "no set", "no bytes", "no float",
], results

# bool / frozenset / bytearray / complex / NoneType
for subj, pat in [
    (True, "bool"), (frozenset({1}), "frozenset"),
    (bytearray(b"x"), "bytearray"), (1j, "complex"), (None, "NoneType"),
]:
    match subj:
        case bool(nope=1) | frozenset(nope=1) | bytearray(nope=1) | complex(nope=1):
            assert False, pat
        case object(nope=1):
            assert False, pat
        case _:
            pass

# --- Existing attributes on builtin types still match ---
match 42:
    case int(real=r, imag=0):
        assert r == 42
    case _:
        assert False
match 1 + 2j:
    case complex(real=r, imag=i):
        assert (r, i) == (1.0, 2.0)
    case _:
        assert False
match 42:
    case int(real=41):
        assert False
    case int(real=42):
        pass
    case _:
        assert False
match 3.5:
    case float(real=x) if x == 3.5:
        pass
    case _:
        assert False

# --- Mixed: one found, one missing => no match, bindings do not leak ---
x_marker = "unset"
match 42:
    case int(real=x_marker, nope=1):
        assert False
    case int(real=42):
        pass
assert x_marker == "unset", x_marker

# --- Missing attribute vs. wildcard/capture sub-pattern still fails ---
match [1, 2]:
    case list(missing=_):
        assert False
    case list(missing=cap):
        assert False
    case _:
        pass

# --- Positional sub-patterns + keyword sub-patterns on a builtin ---
match 5:
    case int(5, real=5):
        pass
    case _:
        assert False
match 5:
    case int(5, nope=1):
        assert False
    case _:
        pass
match {"k": 1}:
    case dict({"k": 1}, nope=1):
        assert False
    case dict(_):
        pass

# --- User classes: missing keyword attribute fails the match ---
class P:
    __match_args__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y


match P(1, 2):
    case P(zzz=1):
        assert False
    case P(x=1, zzz=2):
        assert False
    case P(x=1, y=2):
        pass
    case _:
        assert False


# --- Missing attribute named by __match_args__ also fails to match ---
class Q:
    __match_args__ = ("present", "absent")
    present = 1


match Q():
    case Q(1, 2):
        assert False
    case Q(1):
        pass
    case _:
        assert False
match Q():
    case Q(_, _):
        assert False
    case _:
        pass


# --- Subclass of AttributeError raised by a property counts as missing ---
class MyAttrError(AttributeError):
    pass


class Prop:
    @property
    def boom(self):
        raise MyAttrError("nope")

    @property
    def ok(self):
        return 7


match Prop():
    case Prop(boom=1):
        assert False
    case Prop(ok=7):
        pass
    case _:
        assert False


# --- __getattr__ raising AttributeError means missing ---
class Dyn:
    def __getattr__(self, name):
        if name == "dyn":
            return 3
        raise AttributeError(name)


match Dyn():
    case Dyn(other=1):
        assert False
    case Dyn(dyn=3):
        pass
    case _:
        assert False


# --- Non-AttributeError exceptions still propagate ---
class Bad:
    __match_args__ = ("v",)

    @property
    def v(self):
        raise ValueError("kw boom")

    @property
    def w(self):
        raise KeyError("w")


try:
    match Bad():
        case Bad(w=1):
            pass
except KeyError as e:
    assert e.args == ("w",), e.args
else:
    assert False, "KeyError must propagate"

try:
    match Bad():
        case Bad(1):
            pass
except ValueError as e:
    assert str(e) == "kw boom"
else:
    assert False, "ValueError must propagate from positional lookup"

# --- A property is evaluated exactly once per match attempt ---
calls = []


class Once:
    @property
    def p(self):
        calls.append(1)
        return 9


match Once():
    case Once(p=9):
        pass
    case _:
        assert False
assert calls == [1], calls
match Once():
    case Once(p=0):
        assert False
    case _:
        pass
assert calls == [1, 1], calls


# --- isinstance check still happens first (no attribute lookup on mismatch) ---
looked = []


class Spy:
    def __getattr__(self, name):
        looked.append(name)
        raise AttributeError(name)


match Spy():
    case int(real=1):
        assert False
    case _:
        pass
assert looked == [], looked

# --- Attributes are all fetched before sub-patterns run (CPython order) ---
order = []


class Rec:
    @property
    def a(self):
        order.append("a")
        return 1

    @property
    def b(self):
        order.append("b")
        return 2


match Rec():
    case Rec(a=0, b=2):
        assert False
    case _:
        pass
assert order == ["a", "b"], order

# --- Use inside functions, loops, nested patterns and guards ---
def nested(v):
    match v:
        case [dict(a=1), list(missing=_)]:
            return "bad"
        case [dict(a=1), list(real=_)]:
            return "bad2"
        case [dict(), list()]:
            return "ok"
        case _:
            return "other"


assert nested([{"a": 1}, [1]]) == "ok", nested([{"a": 1}, [1]])
assert nested([{}, []]) == "ok"
assert nested(5) == "other"

hits = 0
for i in range(100):
    match i:
        case int(nope=1):
            assert False
        case int(real=r) if r % 2:
            hits += 1
        case _:
            pass
assert hits == 50, hits

print("ok")
