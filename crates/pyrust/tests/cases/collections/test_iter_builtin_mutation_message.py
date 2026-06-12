# Parity fixture for issue #2400.
#
# The manual `iter()` / `next()` form (the `iter()` *builtin* path, distinct
# from the `for`-loop `GetIter` path covered by stdlib/test_collections_mutation_iter.py)
# must report the same size-mutation RuntimeError as CPython 3.12:
#   - dict and any plain dict subclass -> "dictionary changed size during iteration"
#   - OrderedDict and any OrderedDict subclass -> "OrderedDict mutated during iteration"
# This holds for both key-add and key-delete mutations.  Mutation is applied at
# a deterministic point (after exactly one `next()`).

from collections import OrderedDict


class DSub(dict):
    pass


class ODSub(OrderedDict):
    pass


def run(label, make, mutate):
    c = make()
    it = iter(c)
    try:
        next(it)
        mutate(c)
        next(it)
        print(label, "NO ERROR")
    except RuntimeError as e:
        print(label, "->", e)


def add(c):
    c['z'] = 9


def remove(c):
    del c['a']


makers = [
    ("dict", lambda: dict(a=1, b=2, c=3)),
    ("OrderedDict", lambda: OrderedDict(a=1, b=2, c=3)),
    ("dictSub", lambda: DSub(a=1, b=2, c=3)),
    ("ODSub", lambda: ODSub(a=1, b=2, c=3)),
]

for name, make in makers:
    run(f"{name} add", make, add)
    run(f"{name} del", make, remove)


# ── Value-only mutation (size unchanged) is allowed on the iter() path ──
def value_only(label, make):
    c = make()
    it = iter(c)
    visited = []
    try:
        for _ in range(len(c)):
            k = next(it)
            c[k] = 99
            visited.append(k)
        print(label, "value-mut OK", len(visited))
    except RuntimeError as e:
        print(label, "UNEXPECTED ->", e)


value_only("dict value", lambda: dict(a=1, b=2, c=3))
value_only("OrderedDict value", lambda: OrderedDict(a=1, b=2, c=3))
value_only("ODSub value", lambda: ODSub(a=1, b=2, c=3))


# ── Normal, unmutated iter()/next() drains cleanly ─────────────────────
def drain(label, make):
    it = iter(make())
    out = []
    while True:
        try:
            out.append(next(it))
        except StopIteration:
            break
    print(label, "drained", out)


drain("dict", lambda: dict(a=1, b=2, c=3))
drain("OrderedDict", lambda: OrderedDict(a=1, b=2, c=3))
drain("ODSub", lambda: ODSub(a=1, b=2, c=3))


# ── A user-defined __iter__ on a dict subclass still wins (not the backing
#    guard path). ──────────────────────────────────────────────────────
class CustomIter(OrderedDict):
    def __iter__(self):
        return iter(["custom"])


print("custom __iter__:", list(iter(CustomIter(a=1, b=2))))
