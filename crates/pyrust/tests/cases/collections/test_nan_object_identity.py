# Two *distinct* NaN objects are distinct container keys and distinct sequence
# elements (#2911).
#
# CPython compares container elements with `is` before `==`
# (PyObject_RichCompareBool), and NaN is the only primitive where that is
# observable, since `nan == nan` is False.  So a NaN finds *itself* but never a
# different NaN object.  pyrust used to collapse every NaN to one value, making
# two distinct `float("nan")` results indistinguishable.
#
# The same-object half of the rule is pinned by builtins/test_nan_identity.py.
#
# `hash(nan)` is deliberately NOT asserted: CPython hashes NaN by object id
# (3.10+) so its values are address-derived, while pyrust keeps the stable
# sys.hash_info.nan value.  Only the observable container behaviour is pinned.

a = float("nan")
b = float("nan")

# dict: two entries, insertion-ordered, each keeping its own key object.
d = {a: 1, b: 2}
print(len(d), list(d.values()))
print(list(d)[0] is a, list(d)[1] is b)
print(d[a], d[b])
print(len({a: 1, b: 2, a: 3}), list({a: 1, b: 2, a: 3}.values()))
print(len({a: 1, 1: 2}))

d2 = {a: 1}
d2[b] = 2
print(len(d2), d2[a], d2[b])

# dict membership / get discriminate the two objects.
print(a in d2, b in d2)
print({a: 1}.get(a), {a: 1}.get(b))

# set / frozenset: two distinct elements.
print(len({a, b}), len({a, a}), len({a, b, a, b}))
print(b in {a}, a in {a})
print(len(frozenset([a, b])), a in frozenset([a]), b in frozenset([a]))

# set algebra keeps them apart.
print(len({a, b} | {a}), len({a, b} - {a}), len({a, b} & {a}))
print({a, b} == {a, b}, {a, b} == {b, a}, {a} == {a}, {a} == {b})

# sequence equality: identity-then-equality, element by element.
print([a] == [a], [a] == [b], [a] != [b])
print((a,) == (a,), (a,) == (b,))
print([1, a] == [1, a], [1, a] == [1, b])
print([[a]] == [[a]], [[a]] == [[b]])

# membership / index / count on list and tuple.
print(a in [a], b in [a])
print(a in (a,), b in (a,))
print([a].index(a), (a,).index(a))
print([a].count(a), [a, a].count(a), [a, b].count(a), [a, b].count(b))
print((a, b).count(a))

# dict equality compares values by identity-then-equality too.
print({a: 1} == {a: 1}, {a: 1} == {b: 1})
print({1: a} == {1: a}, {1: a} == {1: b})

# bare equality is untouched: NaN is still not equal to anything, itself
# included.
print(a == a, a == b, a != a, a != b)

# `is` agrees with the container behaviour.
print(a is a, a is b, float("nan") is float("nan"))
c = a
print(c is a, len({a: 1, c: 2}), [a] == [c])

# Operations CPython defines as returning self keep the identity.
print((+a) is a, float(a) is a, a.conjugate() is a, a.real is a)
print(abs(a) is a, (a * 1) is a)

# A NaN survives a round-trip out of, and back into, its container.
d3 = {a: "x", b: "y"}
keys = list(d3)
print(keys[0] is a, keys[1] is b)
print(d3[keys[0]], d3[keys[1]])
print([k in d3 for k in keys])
s3 = {a, b}
print(sorted(d3[k] for k in s3))

# Removal targets exactly one object.
L = [1, a, b, 2]
L.remove(a)
print(len(L), L[1] is b)
s4 = {a, b}
s4.discard(a)
print(len(s4), b in s4, a in s4)
del d3[a]
print(len(d3), a in d3, b in d3)

# Distinct NaNs inside a complex are distinct keys as well.
za = complex(float("nan"), 1.0)
zb = complex(float("nan"), 1.0)
print(len({za: 1, zb: 2}), za in {za}, zb in {za})
print([complex(float("nan"), 0.0)] == [complex(float("nan"), 0.0)])

# Non-NaN floats still collapse by value — 0.0 / -0.0 / False must NOT split.
print(len({0.0: "a", -0.0: "b"}), list({0.0: "a", -0.0: "b"}))
print(len({0: 1, 0.0: 2, False: 3, -0.0: 4}))
print(len({1.5: 1, 1.5: 2}), len({1.5, 1.5}), 1.5 in {1.5})
inf = float("inf")
print(len({inf: 1, float("inf"): 2}), float("inf") in {inf})
print(len({float("inf"), float("inf")}))

# Ordering builtins are unaffected by the identity rule.
print(sorted([3, a, 1]), min([3, a, 1]), max([3, a, 1]), min([a, 3, 1]))
