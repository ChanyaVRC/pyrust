# Issue #2849: list.sort(key=...) with user-instance keys & mutate-during-sort parity.
#
# Verifies:
#   1. list.sort(key=...) returning user instances with __lt__ (and reflected __gt__)
#      produces the exact same results and errors as sorted() and CPython 3.12.
#   2. Stability (forward and reverse=True), key=None, empty / 1-element edge cases.
#   3. Mutate-during-sort detection across ALL list.sort paths (AllInt/AllStr key
#      fast path, general instance-key path, and key-less __lt__ path):
#      - Temporarily empties the list during sort (ob_item = NULL semantics).
#      - Raises ValueError("list modified during sort") on append/extend/pop/slice
#        mutations after restoring the sorted elements.
#      - No-op operations on the temporarily empty list (clear, extend([]),
#        reverse, l[:] = [], re-entrant l.sort()) succeed.
#      - Exceptions raised inside key= or __lt__ take precedence over mutation
#        and preserve the list's elements intact (never leaving it empty).


class K:
    def __init__(self, n):
        self.n = n

    def __lt__(self, other):
        return self.n < other.n

    def __repr__(self):
        return f"K({self.n})"


class GtOnly:
    def __init__(self, n):
        self.n = n

    def __gt__(self, other):
        return self.n > other.n

    def __repr__(self):
        return f"GtOnly({self.n})"


class Unorderable:
    def __init__(self, n):
        self.n = n


class MyList(list):
    pass


def sort_via_method(seq, **kw):
    xs = list(seq)
    ret = xs.sort(**kw)
    return ret, xs


def capture(fn):
    try:
        return ("ok", fn())
    except Exception as e:
        return (type(e).__name__, str(e))


# ── 1. Instance keys & instance elements: list.sort vs sorted ────────────────
data = [3, 1, 2]
print("repro sorted:", sorted(data, key=lambda x: K(x)))
print("repro list.sort:", sort_via_method(data, key=lambda x: K(x)))

print("K elements sorted:", sorted([K(3), K(1), K(2)]))
print("K elements list.sort:", sort_via_method([K(3), K(1), K(2)]))

print("GtOnly elements sorted:", sorted([GtOnly(3), GtOnly(1), GtOnly(2)]))
print("GtOnly elements list.sort:", sort_via_method([GtOnly(3), GtOnly(1), GtOnly(2)]))
print("GtOnly key sorted:", sorted(data, key=GtOnly))
print("GtOnly key list.sort:", sort_via_method(data, key=GtOnly))

# Reverse + stability with instance keys and instance elements
pairs = [(2, "a"), (1, "b"), (2, "c"), (1, "d"), (3, "e"), (2, "f")]
s_fwd = sorted(pairs, key=lambda p: K(p[0]))
_, m_fwd = sort_via_method(pairs, key=lambda p: K(p[0]))
s_rev = sorted(pairs, key=lambda p: K(p[0]), reverse=True)
_, m_rev = sort_via_method(pairs, key=lambda p: K(p[0]), reverse=True)
print("stability fwd:", s_fwd, s_fwd == m_fwd)
print("stability rev:", s_rev, s_rev == m_rev)

# Explicit key=None, empty list, single-element list
print("key=None:", sort_via_method([3, 1, 2], key=None))
print("empty key=42:", capture(lambda: [].sort(key=42)), capture(lambda: sorted([], key=42)))
print("len1 key=42:", capture(lambda: [1].sort(key=42)), capture(lambda: sorted([1], key=42)))

# Error parity between list.sort and sorted
print(
    "unorderable elements:",
    capture(lambda: sorted([Unorderable(2), Unorderable(1)])),
    capture(lambda: [Unorderable(2), Unorderable(1)].sort()),
)
print(
    "unorderable keys:",
    capture(lambda: sorted([2, 1], key=Unorderable)),
    capture(lambda: [2, 1].sort(key=Unorderable)),
)
print(
    "mixed incompatible keys:",
    capture(lambda: sorted([1, 2], key=lambda x: x if x == 1 else "a")),
    capture(lambda: [1, 2].sort(key=lambda x: x if x == 1 else "a")),
)

# ── 2. Mutate-during-sort in key= (AllInt fast path) ─────────────────────────
def test_key_append_int():
    l = [3, 1, 2]
    seen = []

    def k(x):
        seen.append(len(l))
        l.append(4)
        return x

    res = capture(lambda: l.sort(key=k))
    return res, seen, l


print("key append (int):", test_key_append_int())


# Mutate-during-sort in key= (AllStr fast path)
def test_key_append_str():
    l = ["c", "a", "b"]

    def k(x):
        l.append("z")
        return x

    res = capture(lambda: l.sort(key=k))
    return res, l


print("key append (str):", test_key_append_str())


# Mutate-during-sort in key= (general instance-key path)
def test_key_append_instance():
    l = [3, 1, 2]

    def k(x):
        l.append(99)
        return K(x)

    res = capture(lambda: l.sort(key=k))
    return res, l


print("key append (instance):", test_key_append_instance())


# Append + pop (length returns to 0, but list was still mutated!)
def test_key_append_pop():
    l = [3, 1, 2]

    def k(x):
        l.append(99)
        l.pop()
        return x

    res = capture(lambda: l.sort(key=k))
    return res, l


print("key append+pop:", test_key_append_pop())


# Extend non-empty and slice-assign + clear
def test_key_extend_nonempty():
    l = [3, 1, 2]

    def k(x):
        l.extend([9])
        return x

    res = capture(lambda: l.sort(key=k))
    return res, l


def test_key_slice_clear():
    l = [3, 1, 2]

    def k(x):
        l[:] = [1]
        l.clear()
        return x

    res = capture(lambda: l.sort(key=k))
    return res, l


print("key extend([9]):", test_key_extend_nonempty())
print("key slice+clear:", test_key_slice_clear())


# Single-element list with mutating key=
def test_len1_key_append():
    l = [3]

    def k(x):
        l.append(99)
        return x

    res = capture(lambda: l.sort(key=k))
    return res, l


print("len1 key append:", test_len1_key_append())


# list subclass mutating during sort
def test_subclass_key_append():
    ml = MyList([3, 1, 2])

    def k(x):
        ml.append(99)
        return x

    res = capture(lambda: ml.sort(key=k))
    return res, ml


print("subclass key append:", test_subclass_key_append())


# Non-allocating operations on the temporarily empty list succeed (CPython parity)
def test_key_noops():
    out = []
    for label, mut in [
        ("clear", lambda l: l.clear()),
        ("extend_empty", lambda l: l.extend([])),
        ("reverse", lambda l: l.reverse()),
        ("empty_slice", lambda l: l.__setitem__(slice(None), [])),
        ("reentrant_sort", lambda l: l.sort()),
    ]:
        l = [3, 1, 2]

        def k(x):
            mut(l)
            return x

        res = capture(lambda: l.sort(key=k))
        out.append((label, res, l))
    return out


print("key no-ops:", test_key_noops())


# Index assignment inside key= sees empty list -> IndexError, original list intact
def test_key_setitem():
    l = [3, 1, 2]

    def k(x):
        l[0] = 99
        return x

    res = capture(lambda: l.sort(key=k))
    return res, l


print("key setitem:", test_key_setitem())


# Exception in key= leaves original list intact (even if earlier key mutated l)
def test_key_raises():
    l = [3, 1, 2]

    def k(x):
        if x == 1:
            raise RuntimeError("boom")
        return x

    res = capture(lambda: l.sort(key=k))
    return res, l


def test_key_mutates_then_raises():
    l = [3, 1, 2]

    def k(x):
        if x == 3:
            l.append(99)
            return x
        raise RuntimeError("second")

    res = capture(lambda: l.sort(key=k))
    return res, l


print("key raises:", test_key_raises())
print("key mutates then raises:", test_key_mutates_then_raises())


# reverse= __bool__ runs BEFORE stealing the list (CPython parity)
def test_reverse_bool_mutates():
    l = [3, 1, 2]

    class MutBool:
        def __bool__(self):
            l.append(0)
            return True

    l.sort(reverse=MutBool())
    return l


print("reverse bool mutates:", test_reverse_bool_mutates())


# ── 3. Mutate-during-sort in __lt__ (direct and via key=) ────────────────────
def test_lt_observe_empty():
    l = []
    seen = []

    class C:
        def __init__(self, v):
            self.v = v

        def __repr__(self):
            return f"C({self.v})"

        def __lt__(self, other):
            seen.append(len(l))
            return self.v < other.v

    l.extend([C(3), C(1), C(2)])
    l.sort()
    return len(seen) > 0, all(n == 0 for n in seen), l


print("lt observe empty:", test_lt_observe_empty())


def test_lt_append():
    l = []
    seen = []

    class C:
        def __init__(self, v):
            self.v = v

        def __repr__(self):
            return f"C({self.v})"

        def __lt__(self, other):
            seen.append(len(l))
            l.append(C(99))
            return self.v < other.v

    l.extend([C(3), C(1), C(2)])
    res = capture(lambda: l.sort())
    return res, seen[0] == 0 and seen[-1] >= 1, l


print("lt append:", test_lt_append())


def test_key_lt_append():
    l = [3, 1, 2]

    class MutKey:
        def __init__(self, v):
            self.v = v

        def __lt__(self, other):
            l.append(99)
            return self.v < other.v

    res = capture(lambda: l.sort(key=MutKey))
    return res, l


print("key.__lt__ append:", test_key_lt_append())


def test_lt_raises_preserves_elements():
    class Boom:
        def __init__(self, v):
            self.v = v

        def __lt__(self, other):
            if self.v == 1 or other.v == 1:
                raise RuntimeError("lt boom")
            return self.v < other.v

    l = [Boom(3), Boom(1), Boom(2)]
    res = capture(lambda: l.sort())
    return res, len(l), sorted(x.v for x in l)


print("lt raises preserves:", test_lt_raises_preserves_elements())
