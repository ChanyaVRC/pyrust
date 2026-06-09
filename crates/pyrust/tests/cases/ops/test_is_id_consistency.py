# `is` must agree with `id()`: `a is b` <=> `id(a) == id(b)` (#2287).
# Aliased strings were the bug — `is` returned False while `id` was equal.


# An aliased string is the same object: `is` True and consistent with `id`.
a = "hello world this is long"
b = a
print(a is b)
print(id(a) == id(b))
print((a is b) == (id(a) == id(b)))

# A reference taken from a longer expression still aliases.
sub = a[0:5]
sub2 = sub
print(sub is sub2)
print((sub is sub2) == (id(sub) == id(sub2)))

# Whatever interning CPython does for short/computed literals, pyrust only
# guarantees `is`/`id` consistency — assert the invariant, not the value.
c1 = "ab"
c2 = "a" + "b"
print((c1 is c2) == (id(c1) == id(c2)))

empty1 = ""
empty2 = ""
print((empty1 is empty2) == (id(empty1) == id(empty2)))

# Other types must be unaffected.
print([] is [])
lst = []
print(lst is lst)
print(1 is 1)
print(None is None)
print(True is True)
t = (1, 2, 3)
u = t
print(u is t)
print((u is t) == (id(u) == id(t)))
d = {"k": 1}
e = d
print(e is d)
print((e is d) == (id(e) == id(d)))
