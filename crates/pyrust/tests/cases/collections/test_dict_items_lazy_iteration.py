# Iterating a dict `.items()` view builds each (k, v) tuple lazily per step
# (issue #2830) instead of materialising all N tuples up front. This must be
# observationally identical to CPython: same tuples, same size-mutation guard,
# same OrderedDict wording.
from collections import OrderedDict

d = {"a": 1, "b": 2, "c": 3}

# basic iteration + unpack
print(list(d.items()))
print([(k, v) for k, v in d.items()])
print({v: k for k, v in d.items()})
total = 0
for k, v in d.items():
    total += v
print(total)

# each yielded element is a fresh 2-tuple
for t in d.items():
    print(t, type(t).__name__, len(t), t[0], t[1])

# empty and single
print(list({}.items()))
for _k, _v in {}.items():
    print("unreachable")
for k, v in {"x": 9}.items():
    print(k, v)

# a view is re-iterable and each pass yields equal tuples
view = d.items()
print(list(view), list(view))

# mixed value types
dm = {1: "s", 2: [1, 2], 3: None, 4: (5, 6)}
for k, v in dm.items():
    print(k, v)

# tuple equality / membership against the view
print(("a", 1) in d.items())
print(("a", 99) in d.items())

# ── size-mutation guard during items iteration ──────────────────────────────
# insert
d2 = {1: 1, 2: 2, 3: 3}
try:
    for k, v in d2.items():
        if k == 1:
            d2[99] = 99
except RuntimeError as e:
    print("insert:", e)

# delete
d2 = {1: 1, 2: 2, 3: 3}
try:
    for k, v in d2.items():
        if k == 1:
            del d2[2]
except RuntimeError as e:
    print("del:", e)

# clear
d2 = {1: 1, 2: 2, 3: 3}
try:
    for k, v in d2.items():
        d2.clear()
except RuntimeError as e:
    print("clear:", e)

# value-only mutation preserves size → allowed
d2 = {1: 1, 2: 2, 3: 3}
seen = []
for k, v in d2.items():
    d2[k] = v * 10
    seen.append((k, v))
print("valuemut:", seen, d2)

# ── OrderedDict items view: same guard, OrderedDict wording ──────────────────
od = OrderedDict([(1, 1), (2, 2), (3, 3)])
print(list(od.items()))
try:
    for k, v in od.items():
        if k == 1:
            od[99] = 99
except RuntimeError as e:
    print("od insert:", e)

od = OrderedDict([(1, 1), (2, 2), (3, 3)])
try:
    for k, v in od.items():
        od.clear()
except RuntimeError as e:
    print("od clear:", e)

# ── unpack-target shapes (the 2-target case is fused; others must still work) ─
dd = {"a": 1, "b": 2}
# parenthesized 2-target (fused, identical to `k, v`)
for (k, v) in dd.items():
    print("paren", k, v)
# wrong arity -> ValueError (not fused)
try:
    for x, y, z in dd.items():
        print(x, y, z)
except ValueError as e:
    print("3targets:", e)
try:
    for (only,) in dd.items():
        print(only)
except ValueError as e:
    print("1target:", e)
# extended `*` unpack -> UnpackEx, not fused
for k, *rest in dd.items():
    print("star", k, rest)
for *init, last in dd.items():
    print("star2", init, last)
# nested targets: keys are themselves 2-tuples
dn = {(1, 2): "x", (3, 4): "y"}
for (a, b), c in dn.items():
    print("nested", a, b, c)
# loop variables survive after the loop
for k, v in dd.items():
    pass
print("last", k, v)
# unpack inside a comprehension
print({v: k for k, v in dd.items()})
