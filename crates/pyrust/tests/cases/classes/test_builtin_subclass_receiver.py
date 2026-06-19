# Issue #2386: a single representation-substitutability boundary
# (effective_builtin_receiver) normalises a builtin-subclass PyInstance to its
# backing value at each consumer entry point. This fixture exercises the
# consumer paths routed through that helper: numeric/container coercion in
# binary ops, iteration (iter_values / collect_iterable), dict-merge entry
# extraction, set-op item extraction, membership (`in`), printf-style `%`
# formatting on str/bytes subclasses, complex-operand detection, and the
# not-iterable error which must name the actual subclass, not the backing base.


class MyList(list):
    pass


class MyTuple(tuple):
    pass


class MyDict(dict):
    pass


class MySet(set):
    pass


class MyFrozen(frozenset):
    pass


class MyStr(str):
    pass


class MyInt(int):
    pass


class MyFloat(float):
    pass


class MyComplex(complex):
    pass


class MyBytes(bytes):
    pass


# --- binary-op coercion: result follows the base type ---
print(MyList([1, 2]) + [3])
print(MyTuple((1, 2)) + (3,))
print(MyInt(40) + 2)
print(MyFloat(1.5) * 2)
print(MyStr("ab") + "c")
print(MyBytes(b"ab") + b"c")
print(MyComplex(1, 2) + 1j)

# --- iteration ---
print(list(MyList([3, 4, 5])))
print(sorted(MySet({3, 1, 2})))
print(sorted(MyFrozen({9, 7})))
print([k for k in MyDict({"a": 1, "b": 2})])
print(list(MyTuple((6, 7))))

# --- dict merge (entry extraction) ---
merged = {"z": 0}
merged |= MyDict({"a": 1})
print(merged)
print({**MyDict({"x": 9})})

# --- set ops (item extraction) ---
print(sorted(MySet({1, 2}) | {3}))
print(sorted(MyFrozen({1, 2}) & {2, 3}))

# --- membership ---
print(2 in MyList([1, 2, 3]))
print("k" in MyDict({"k": 1}))
print(5 in MySet({4, 5, 6}))

# --- printf-style formatting ---
print(MyStr("hi %s, %d") % ("a", 7))
print(MyBytes(b"v=%d") % 3)

# --- complex-operand detection ---
print(MyComplex(2, 0) * MyInt(3))

# --- not-iterable error names the subclass, not the backing base ---
try:
    iter(MyInt(5))
except TypeError as e:
    print(e)
try:
    for _ in MyFloat(1.0):
        pass
except TypeError as e:
    print(e)
