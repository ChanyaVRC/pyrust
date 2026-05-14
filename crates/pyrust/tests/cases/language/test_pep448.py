# PEP 448 — additional unpacking generalisations
# `*expr` in list / tuple / set literals; `**expr` in dict literals.

# ── Dict splat ──────────────────────────────────────────────────────────
a = {"a": 1}
b = {"b": 2}
assert {**a, **b} == {"a": 1, "b": 2}
assert {**a, "c": 3} == {"a": 1, "c": 3}
# Later keys override earlier ones — both for splats and explicit pairs.
assert {**a, **b, "a": 99} == {"a": 99, "b": 2}
# Nested literal splat
assert {**a, **{"d": 4}} == {"a": 1, "d": 4}
# Explicit pair after splat overriding
assert {**{"k": 1, "v": 2}, "k": 99} == {"k": 99, "v": 2}
# Last-key-wins across multiple splats
assert {**{"a": 1}, **{"a": 2}, **{"a": 3}} == {"a": 3}
# Empty splat is a no-op
assert {**{}, **{}, "x": 1} == {"x": 1}
# Single splat alone
assert {**{"k": 7}} == {"k": 7}

# ── List splat ──────────────────────────────────────────────────────────
assert [1, 2, *[3, 4], 5] == [1, 2, 3, 4, 5]
assert [*[]] == []
assert [*[1], *[2, 3], 4, *[5]] == [1, 2, 3, 4, 5]
# Splat from string
assert [*"abc"] == ["a", "b", "c"]
# Splat from a tuple into a list
assert [*(1, 2, 3)] == [1, 2, 3]
# Splat from range
assert [*range(3), *range(3, 6)] == [0, 1, 2, 3, 4, 5]

# ── Tuple splat ─────────────────────────────────────────────────────────
assert (*[1, 2], *[3, 4], 5) == (1, 2, 3, 4, 5)
# Single splat with trailing comma (still a tuple)
assert (*[1, 2, 3],) == (1, 2, 3)
# Empty splat tuple
assert (*[],) == ()
# Splat from a tuple into a tuple
assert (*((1, 2)), 3) == (1, 2, 3)

# ── Set splat (dedupes) ─────────────────────────────────────────────────
assert {*[1, 2], *[3, 4]} == {1, 2, 3, 4}
# Set splat with duplicates — deduped
assert {*[1, 1, 2, 2], *[3, 3]} == {1, 2, 3}
# Mixed splat + literal element in a set
assert {0, *[1, 2], 3, *[4, 5]} == {0, 1, 2, 3, 4, 5}

# ── TypeError on non-iterable / non-mapping ─────────────────────────────
try:
    _ = [*5]
except TypeError:
    pass
else:
    raise AssertionError("expected TypeError for [*5]")

try:
    _ = {**5}
except TypeError:
    pass
else:
    raise AssertionError("expected TypeError for {**5}")

try:
    _ = {*5}
except TypeError:
    pass
else:
    raise AssertionError("expected TypeError for {*5}")

try:
    _ = (*5,)
except TypeError:
    pass
else:
    raise AssertionError("expected TypeError for (*5,)")

# ── Idiomatic dict-merge: base + override ───────────────────────────────
base = {"host": "localhost", "port": 80}
override = {"port": 8080, "debug": True}
merged = {**base, **override}
assert merged == {"host": "localhost", "port": 8080, "debug": True}

# ── Idiomatic list-concat ───────────────────────────────────────────────
xs = [1, 2, 3]
ys = [4, 5, 6]
assert [*xs, *ys] == xs + ys

print("ok")
