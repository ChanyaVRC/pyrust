# Parity fixture for MappingProxyType over non-dict mappings.
# Issue #2936: `types.MappingProxyType(OrderedDict())` raised
# "TypeError: mappingproxy() argument must be a mapping, not OrderedDict".
# CPython 3.12 accepts any dict subclass (OrderedDict / defaultdict / Counter /
# user subclasses) as well as another mappingproxy, and the resulting proxy is a
# live read-only view whose repr and copy() delegate to the proxied object.
from collections import Counter, OrderedDict, defaultdict
from types import MappingProxyType


class MyDict(dict):
    pass


od = OrderedDict([("a", 1), ("b", 2)])
dd = defaultdict(int, {"a": 1, "b": 2})
ct = Counter({"a": 1, "b": 2})
md = MyDict(a=1, b=2)
plain = {"a": 1, "b": 2}

print("--- construction ---")
for label, mapping in [
    ("OrderedDict", od),
    ("defaultdict", dd),
    ("Counter", ct),
    ("MyDict", md),
    ("dict", plain),
]:
    proxy = MappingProxyType(mapping)
    print(label, type(proxy).__name__, len(proxy))

print("--- read surface (OrderedDict) ---")
p = MappingProxyType(od)
print(len(p))
print(p["a"], p["b"])
print(p.get("a"), p.get("zz"), p.get("zz", -1))
print("a" in p, "zz" in p)
print(list(p))
print(list(p.keys()), list(p.values()), list(p.items()))
print(repr(p))
print(p == {"a": 1, "b": 2}, p == {"a": 1}, p == od, od == p, p != od, p != {"a": 1})
print(p == MappingProxyType(od), p == MappingProxyType(dd))
print(list(reversed(p)))
try:
    print(p["zz"])
except KeyError as e:
    print("KeyError:", e)

print("--- copy delegates to the proxied type ---")
for label, mapping in [
    ("OrderedDict", od),
    ("defaultdict", dd),
    ("Counter", ct),
    ("MyDict", md),
    ("dict", plain),
]:
    copied = MappingProxyType(mapping).copy()
    print(label, type(copied).__name__, sorted(copied.items()))

print("--- copy is detached ---")
c = MappingProxyType(od).copy()
c["zz"] = 99
print("zz" in p, sorted(od.items()))

print("--- repr delegates to the proxied object ---")
print(repr(MappingProxyType(dd)))
print(repr(MappingProxyType(ct)))
print(repr(MappingProxyType(md)))
print(repr(MappingProxyType(plain)))

print("--- str/print/f-string show the proxied object (repr does not) ---")
empty_spec = ""
print(
    str(p),
    "|",
    f"{p}",
    "|",
    f"{p:}",
    "|",
    f"{p:{empty_spec}}",
    "|",
    "{}".format(p),
    "|",
    format(p, empty_spec),
)
print(p)
print(str(MappingProxyType(ct)), "|", str(MappingProxyType(md)))

print("--- PEP 584 | keeps the proxied type ---")
print(MappingProxyType(od) | {"c": 3})
print({"c": 3} | MappingProxyType(od))
print(MappingProxyType(md) | {"c": 3})
try:
    q = MappingProxyType(od)
    q |= {"c": 3}
except TypeError as e:
    print("|=:", "not supported" in str(e) or "unsupported" in str(e))

print("--- live view ---")
live_src = OrderedDict([("a", 1), ("b", 2)])
live = MappingProxyType(live_src)
live_src["c"] = 3
print(len(live), list(live), live["c"])
live_src.move_to_end("a")
print(list(live), repr(live))
live_src.popitem(last=False)
print(list(live), len(live))
del live_src["c"]
print(list(live), live == {"a": 1})

print("--- read-only ---")
for op in ("setitem", "delitem", "update", "pop", "clear"):
    try:
        if op == "setitem":
            p["zz"] = 1
        elif op == "delitem":
            del p["a"]
        elif op == "update":
            p.update({"zz": 1})
        elif op == "pop":
            p.pop("a")
        else:
            p.clear()
        print(op, "no error")
    except (TypeError, AttributeError) as e:
        print(op, type(e).__name__)

print("--- proxy of a proxy ---")
pp = MappingProxyType(p)
print(len(pp), pp["a"], repr(pp), type(pp.copy()).__name__, sorted(pp.copy().items()))

# CPython forwards `==` / `!=` / `|` to the proxied object *recursively*: when
# that object is itself a mappingproxy the forwarded call forwards again, so a
# nested proxy compares and merges as the innermost mapping.  A single-hop
# resolution would leave the inner proxy as the operand and silently fall back
# to plain-dict semantics (equality False, `|` losing the proxied type).
ppp = MappingProxyType(pp)
print(pp == od, ppp == od, od == pp, pp != od, pp == p, p == pp)
print(pp | {"c": 3}, {"c": 3} | pp, ppp | {"c": 3})
nested_counter = MappingProxyType(MappingProxyType(ct))
merged = nested_counter | Counter({"a": 5})
print(merged, type(merged).__name__)
nested_plain = MappingProxyType(MappingProxyType(plain))
print(nested_plain == plain, nested_plain | {"z": 9})

print("--- empty mappings ---")
for label, mapping in [
    ("OrderedDict", OrderedDict()),
    ("Counter", Counter()),
    ("defaultdict", defaultdict(list)),
]:
    e = MappingProxyType(mapping)
    print(label, len(e), bool(e), list(e), repr(e))

print("--- an initially empty subclass is still a live view ---")
fresh = MyDict()
fresh_proxy = MappingProxyType(fresh)
print(len(fresh_proxy), list(fresh_proxy))
fresh["k"] = "v"
print(len(fresh_proxy), fresh_proxy["k"], list(fresh_proxy.items()), fresh_proxy == fresh)

print("--- non-mappings are rejected ---")
for bad in ([], (), set(), frozenset(), 5, None, object(), list, MyDict):
    try:
        MappingProxyType(bad)
        print(type(bad).__name__, "accepted")
    except TypeError as e:
        print(type(bad).__name__, "TypeError:", e)

print("--- arity errors ---")
try:
    MappingProxyType()
except TypeError as e:
    print("no args:", e)
try:
    MappingProxyType({}, {})
except TypeError as e:
    print("two args:", e)
