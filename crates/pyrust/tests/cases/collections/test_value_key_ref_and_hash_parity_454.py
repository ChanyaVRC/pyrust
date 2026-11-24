"""CPython 3.12.3 parity tests for issue #454:
Zero-alloc ValueKeyRef probes, compact PyKey layout, cross-type equality,
NaN identity, frozenset/tuple keys, and single-__hash__ guarantees.
"""

from collections import Counter, OrderedDict, defaultdict
from types import MappingProxyType


# ── 1. Single-__hash__ guarantee for PyInstance across dict/set/frozenset ────

class HashCounter:
    def __init__(self, v):
        self.v = v
        self.hashes = 0
        self.eqs = 0

    def __hash__(self):
        self.hashes += 1
        return hash(self.v)

    def __eq__(self, other):
        self.eqs += 1
        if isinstance(other, HashCounter):
            return self.v == other.v
        return self.v == other


def reset_and_check(obj, expected_hashes, label):
    assert obj.hashes == expected_hashes, f"{label}: got {obj.hashes}, expected {expected_hashes}"
    obj.hashes = 0


k = HashCounter(42)
d = {}

d[k] = "first"
reset_and_check(k, 1, "dict[obj] = v (insert)")

d[k] = "second"
reset_and_check(k, 1, "dict[obj] = v (update)")

assert d[k] == "second"
reset_and_check(k, 1, "dict[obj] (hit)")

assert k in d
reset_and_check(k, 1, "obj in dict (hit)")

assert d.get(k) == "second"
reset_and_check(k, 1, "dict.get(obj) (hit)")

assert d.setdefault(k, "unused") == "second"
reset_and_check(k, 1, "dict.setdefault(obj) (hit)")

assert d.pop(k) == "second"
reset_and_check(k, 1, "dict.pop(obj) (hit)")

assert k not in d
reset_and_check(k, 1, "obj in dict (miss)")

assert d.get(k, "def") == "def"
reset_and_check(k, 1, "dict.get(obj) (miss)")

assert d.setdefault(k, "created") == "created"
reset_and_check(k, 1, "dict.setdefault(obj) (miss)")

del d[k]
reset_and_check(k, 1, "del dict[obj]")

# Set and frozenset single-__hash__ guarantees
s = set()
s.add(k)
reset_and_check(k, 1, "set.add(obj) (insert)")

s.add(k)
reset_and_check(k, 1, "set.add(obj) (duplicate)")

assert k in s
reset_and_check(k, 1, "obj in set (hit)")

s.discard(k)
reset_and_check(k, 1, "set.discard(obj) (hit)")

assert k not in s
reset_and_check(k, 1, "obj in set (miss)")

s.add(k)
k.hashes = 0
s.remove(k)
reset_and_check(k, 1, "set.remove(obj)")

s.add(k)
k.hashes = 0
fs = frozenset(s)
assert k in fs
reset_and_check(k, 1, "obj in frozenset (hit)")

k_miss = HashCounter(999)
assert k_miss not in fs
reset_and_check(k_miss, 1, "obj in frozenset (miss)")

# MappingProxy single-__hash__ guarantee
backing = {k: "val"}
k.hashes = 0
mp = MappingProxyType(backing)
assert mp[k] == "val"
reset_and_check(k, 1, "mappingproxy[obj]")
assert mp.get(k) == "val"
reset_and_check(k, 1, "mappingproxy.get(obj)")
assert k in mp
reset_and_check(k, 1, "obj in mappingproxy")

# OrderedDict / Counter / defaultdict single-__hash__ on direct ops
od = OrderedDict()
od[k] = 1
od["other"] = 2
k.hashes = 0
od.move_to_end(k)
reset_and_check(k, 1, "OrderedDict.move_to_end(obj)")

c = Counter()
c[k] = 3
reset_and_check(k, 1, "Counter[obj] = v")
assert c[k] == 3
reset_and_check(k, 1, "Counter[obj]")
assert k in c
reset_and_check(k, 1, "obj in Counter")
assert c.get(k) == 3
reset_and_check(k, 1, "Counter.get(obj)")

dd = defaultdict(int)
dd[k] = 7
reset_and_check(k, 1, "defaultdict[obj] = v")
assert dd[k] == 7
reset_and_check(k, 1, "defaultdict[obj] (hit)")
assert k in dd
reset_and_check(k, 1, "obj in defaultdict")
assert dd.get(k) == 7
reset_and_check(k, 1, "defaultdict.get(obj)")


# ── 2. Cross-type numeric unification & first-key-wins via ValueKeyRef ───────

d_num = {}
d_num[True] = "bool"
assert d_num[1] == "bool"
assert d_num[1.0] == "bool"
assert d_num[1 + 0j] == "bool"
assert 1 in d_num and 1.0 in d_num and (1 + 0j) in d_num
assert d_num.get(1.0) == "bool"
assert d_num.setdefault(1 + 0j, "nope") == "bool"
# Existing-key update keeps original key `True`
d_num[1] = "updated_by_int"
d_num[1.0] = "updated_by_float"
d_num[1 + 0j] = "updated_by_complex"
keys = list(d_num.keys())
assert len(keys) == 1 and type(keys[0]) is bool and keys[0] is True
assert d_num[True] == "updated_by_complex"
assert d_num.pop(1.0) == "updated_by_complex"
assert len(d_num) == 0

# Zero unification: 0, False, 0.0, -0.0, 0+0j, -0.0-0.0j
d_zero = {-0.0: "neg_zero"}
for z in (0, False, 0.0, -0.0, 0 + 0j, complex(-0.0, -0.0)):
    assert z in d_zero
    assert d_zero[z] == "neg_zero"
d_zero[0] = "updated"
assert len(d_zero) == 1
assert d_zero[False] == "updated"

# BigInt <-> Float <-> Complex unification
big = 10**20
d_big = {big: "bigint"}
assert 1e20 in d_big
assert (1e20 + 0j) in d_big
assert d_big[1e20] == "bigint"
assert d_big[1e20 + 0j] == "bigint"
d_big[1e20] = "via_float"
assert len(d_big) == 1 and type(next(iter(d_big))) is int
assert d_big[big] == "via_float"
del d_big[1e20 + 0j]
assert len(d_big) == 0


# ── 3. Tuple, nested tuple, bytes, frozenset, Ellipsis, None probes ──────────

d_tup = {
    (1, "alpha", (True, 0.0, b"xyz")): "t1",
    (): "empty",
    (10**20, 1 + 0j): "tbig",
    frozenset({1, 2, 3}): "fs1",
    None: "none_val",
    ...: "ellipsis_val",
    b"bytes_key": "bytes_val",
}

assert d_tup[(1.0, "alpha", (1, -0.0, b"xyz"))] == "t1"
assert (1 + 0j, "alpha", (1.0, False, b"xyz")) in d_tup
d_tup[(True, "alpha", (1.0, 0, b"xyz"))] = "t1_updated"
assert d_tup[(1, "alpha", (True, 0.0, b"xyz"))] == "t1_updated"
assert len(d_tup) == 7

assert d_tup[()] == "empty"
assert d_tup[(1e20, 1)] == "tbig"
assert d_tup[frozenset({3, 2, 1})] == "fs1"
assert frozenset({1.0, 2, 3}) in d_tup
d_tup[frozenset({2, 3, 1.0})] = "fs1_updated"
assert d_tup[frozenset({1, 2, 3})] == "fs1_updated"
assert d_tup[None] == "none_val"
assert d_tup[...] == "ellipsis_val"
assert d_tup[b"bytes_key"] == "bytes_val"

# Set & frozenset probes with tuples and frozensets
s_comp = {(1, "a"), frozenset({10, 20}), b"hi", ..., None, 10**20}
assert (1.0, "a") in s_comp
assert (True, "a") in s_comp
assert frozenset({20.0, 10}) in s_comp
assert 1e20 in s_comp
s_comp.add((1.0, "a"))
assert len(s_comp) == 6
s_comp.discard((True, "a"))
assert len(s_comp) == 5
assert (1, "a") not in s_comp

fs_comp = frozenset(s_comp)
assert frozenset({10, 20}) in fs_comp
assert 1e20 in fs_comp
assert b"hi" in fs_comp
assert ... in fs_comp
assert None in fs_comp


# ── 4. NaN identity semantics ────────────────────────────────────────────────

nan1 = float("nan")
nan2 = float("nan")
d_nan = {nan1: "first", nan2: "second"}
assert len(d_nan) == 2
assert d_nan[nan1] == "first"
assert d_nan[nan2] == "second"
assert nan1 in d_nan and nan2 in d_nan
assert float("nan") not in d_nan
d_nan[nan1] = "first_updated"
assert len(d_nan) == 2 and d_nan[nan1] == "first_updated"

# Tuple containing NaN
d_tnan = {(nan1, 1): "tn1", (nan2, 1): "tn2"}
assert len(d_tnan) == 2
assert d_tnan[(nan1, 1.0)] == "tn1"
assert d_tnan[(nan2, True)] == "tn2"
assert (float("nan"), 1) not in d_tnan

# Complex with NaN
cnan1 = complex(nan1, 0.0)
assert cnan1 not in d_nan
d_cnan = {cnan1: "cn1"}
assert d_cnan[cnan1] == "cn1"
assert nan1 not in d_cnan


# ── 5. Mixed primitive + dynamic PyInstance keys (probe & eq dispatch) ───────

class MatchesInt:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return hash(self.n)

    def __eq__(self, other):
        if isinstance(other, MatchesInt):
            return self.n == other.n
        return self.n == other


m1 = MatchesInt(10)
d_mix = {m1: "from_obj"}
assert 10 in d_mix
assert d_mix[10] == "from_obj"
assert d_mix.get(10.0) == "from_obj"
d_mix[10] = "updated_via_int"
assert len(d_mix) == 1
assert d_mix[m1] == "updated_via_int"
assert isinstance(next(iter(d_mix)), MatchesInt)
del d_mix[10]
assert len(d_mix) == 0

# Nested dynamic object inside tuple and frozenset
m2 = MatchesInt(7)
d_nested = {(m2, "k"): "nested_tuple", frozenset({m2}): "nested_fs"}
assert (7, "k") in d_nested
assert d_nested[(7, "k")] == "nested_tuple"
assert (MatchesInt(7), "k") in d_nested
assert d_nested[(MatchesInt(7), "k")] == "nested_tuple"
assert frozenset({MatchesInt(7)}) in d_nested
assert d_nested[frozenset({MatchesInt(7)})] == "nested_fs"
assert frozenset({7}) in d_nested
assert d_nested[frozenset({7})] == "nested_fs"

# None vs custom Object with hash(None)
class MatchesNone:
    def __hash__(self):
        return hash(None)

    def __eq__(self, other):
        return other is None or isinstance(other, MatchesNone)


mn = MatchesNone()
s_none = {mn}
assert None in s_none
fs_none = frozenset({mn})
assert None in fs_none
d_none = {mn: "obj_none"}
assert None in d_none and d_none[None] == "obj_none"


# ── 6. Live iterator value-only update + Counter tallying with tuples ────────

d_iter = {"a": 1, "b": 2, (1, 2): 3}
seen = []
for k_it in d_iter:
    d_iter[k_it] = d_iter[k_it] * 10
    seen.append((k_it, d_iter[k_it]))
assert seen == [("a", 10), ("b", 20), ((1, 2), 30)]

c_tup = Counter([(1, "x"), (1.0, "x"), (True, "x"), (2, "y"), (2.0, "y")])
assert c_tup[(1, "x")] == 3
assert c_tup[(2, "y")] == 2
assert len(c_tup) == 2


# ── 7. Unhashable keys still raise TypeError everywhere ──────────────────────

for bad in ([1], {1: 2}, {1}, (1, [2]), (1, (2, {3}))):
    for op in (
        lambda b=bad: d_tup[b],
        lambda b=bad: b in d_tup,
        lambda b=bad: d_tup.get(b),
        lambda b=bad: d_tup.pop(b, None),
        lambda b=bad: d_tup.setdefault(b, 0),
        lambda b=bad: c_tup[b],
        lambda b=bad: b in c_tup,
        lambda b=bad: dd[b],
    ):
        try:
            op()
            raise AssertionError(f"expected TypeError for unhashable {bad!r}")
        except TypeError:
            pass

# Note: CPython allows mutable `set` in set/frozenset membership/discard/remove
# via temporary frozenset coercion; non-set unhashables must still raise TypeError.
for bad in ([1], {1: 2}, (1, [2]), (1, (2, {3}))):
    for op in (
        lambda b=bad: b in s_comp,
        lambda b=bad: s_comp.discard(b),
        lambda b=bad: s_comp.remove(b),
        lambda b=bad: b in fs_comp,
    ):
        try:
            op()
            raise AssertionError(f"expected TypeError for unhashable {bad!r}")
        except TypeError:
            pass

print("OK: test_value_key_ref_and_hash_parity_454")
