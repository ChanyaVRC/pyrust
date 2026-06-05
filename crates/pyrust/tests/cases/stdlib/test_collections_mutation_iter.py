# Parity fixture for issue #2201.
#
# The dict subclasses Counter / defaultdict / OrderedDict must raise
# RuntimeError when the mapping changes size during iteration, matching
# CPython 3.12. OrderedDict (and its subclasses) use their own message
# ("OrderedDict mutated during iteration"); every other dict subclass uses
# dict's wording ("dictionary changed size during iteration"). Value-only
# mutations that preserve the key count are allowed.

from collections import Counter, defaultdict, OrderedDict


def size_change(name, make, mutate):
    c = make()
    try:
        for k in c:
            mutate(c)
        print(name, "NO ERROR")
    except RuntimeError as e:
        print(name, "->", e)


size_change("Counter", lambda: Counter(a=1, b=2), lambda c: c.__setitem__('z', 9))
size_change("defaultdict", lambda: defaultdict(int, x=1, y=2), lambda c: c.__setitem__('w', 1))
size_change("OrderedDict", lambda: OrderedDict(a=1, b=2), lambda c: c.__setitem__('z', 1))


# A plain dict subclass uses the dict message.
class D(dict):
    pass


size_change("PlainDictSub", lambda: D(a=1, b=2), lambda c: c.__setitem__('z', 9))


# A subclass of OrderedDict inherits the OrderedDict message.
class MyOD(OrderedDict):
    pass


size_change("OrderedDictSub", lambda: MyOD(a=1, b=2), lambda c: c.__setitem__('z', 1))


# ── Value-only mutation is allowed (size unchanged) ────────────────────
def value_only(name, make):
    c = make()
    visited = []
    try:
        for k in c:
            c[k] = 99
            visited.append(k)
        print(name, "value-mut OK", len(visited))
    except RuntimeError as e:
        print(name, "UNEXPECTED ->", e)


value_only("Counter", lambda: Counter(a=1, b=2))
value_only("defaultdict", lambda: defaultdict(int, x=1, y=2))
value_only("OrderedDict", lambda: OrderedDict(a=1, b=2))


# ── Normal iteration is unregressed ────────────────────────────────────
print("Counter iter:", sorted(Counter(a=1, b=2)))
print("defaultdict iter:", sorted(defaultdict(int, x=1, y=2)))
print("OrderedDict iter:", list(OrderedDict(a=1, b=2)))

# Re-iteration after a completed loop still works.
c = Counter(a=1, b=2)
print("first:", sorted(c))
print("second:", sorted(c))
