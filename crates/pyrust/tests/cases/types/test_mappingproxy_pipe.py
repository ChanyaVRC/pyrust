# Parity fixture for mappingproxy PEP 584 `|` and method arg-count errors.
# - mappingproxy | dict and dict | mappingproxy produce a merged dict.
# - get() / keys() / values() / items() / copy() error wording matches
#   CPython 3.12.
#
# Uses dict-backed mappingproxies (d.keys().mapping, issue #2679) so the
# content is stable across implementations; class-backed vars(C) carries
# implementation-specific dunder entries that would make output diverge.


def proxy(d):
    return d.keys().mapping


mp = proxy({"a": 1, "b": 2})

merged = mp | {"c": 3, "a": 99}
print(type(merged).__name__, sorted(merged.items()))

rmerged = {"c": 3, "a": 99} | mp
print(type(rmerged).__name__, sorted(rmerged.items()))

# Two mappingproxies.
both = mp | proxy({"c": 3})
print(type(both).__name__, sorted(both.items()))

# Right operand wins on key collisions (PEP 584 semantics).
print(sorted((proxy({"x": 1}) | {"x": 2}).items()))
print(sorted(({"x": 2} | proxy({"x": 1})).items()))

# `|` with a non-mapping right operand raises TypeError.
try:
    mp | [1, 2, 3]
except TypeError as e:
    print("TypeError:", e)

# `|` with a mappingproxy on the RIGHT (left is a non-mapping) also reports the
# proxy as `dict`, because its `__ror__` is `dict.__ror__` (CPython 3.12).
for left in [[1, 2], "s", (1,), 3.0, {1, 2}, 5]:
    try:
        left | mp
    except TypeError as e:
        print("TypeError:", e)

# Set-only operators (`&`, `-`, `^`) have no dict slot, so a mappingproxy
# operand keeps its own name there (not renamed to `dict`).
for sym, fn in [
    ("&", lambda a, b: a & b),
    ("-", lambda a, b: a - b),
    ("^", lambda a, b: a ^ b),
]:
    try:
        fn({1, 2}, mp)
    except TypeError as e:
        print("TypeError:", e)

# mappingproxy is read-only: `|=` is rejected even though `|` works.
m = mp
try:
    m |= {"z": 9}
except TypeError as e:
    print("TypeError:", e)

# Method argument-count error wording.
for call in [
    lambda: mp.get(),
    lambda: mp.get(1, 2, 3),
    lambda: mp.keys(1),
    lambda: mp.values(1),
    lambda: mp.items(1),
    lambda: mp.copy(1),
]:
    try:
        call()
        print("no error")
    except TypeError as e:
        print("TypeError:", e)

# Sanity: valid get() calls still work.
print(mp.get("a"))
print(mp.get("missing", "default"))
