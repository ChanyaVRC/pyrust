# Dict equality compares values via CPython's `PyObject_RichCompareBool`, which
# checks `a is b` (identity) before falling back to `__eq__`.  A NaN value stored
# in a dict must therefore compare equal to *itself* even though `nan != nan`
# makes bare `==` False.  See issue #2545.

# Self-comparison: the same dict object on both sides.  The value is the same
# object, so the identity short-circuit makes `d == d` True.
d = {1: float("nan")}
print(d == d)  # True

# Same for a NaN-bearing complex value.
d2 = {1: complex(float("nan"), 0)}
print(d2 == d2)  # True

# Two dicts that share the *same* NaN value object (the interned `nan` name)
# compare equal because the values are identical objects.
nan = float("nan")
print({1: nan} == {1: nan})  # True
print({"a": nan, "b": 1} == {"a": nan, "b": 1})  # True

# Imaginary-part NaN, same object on both sides.
cnan = complex(0, float("nan"))
print({0: cnan} == {0: cnan})  # True

# Ordinary (non-NaN) float values are unaffected: value equality still holds and
# unequal values still compare False.
print({1: 2.0} == {1: 2.0})  # True
print({1: 2.0} == {1: 3.0})  # False
print({1: 2.0, 2: 4.0} == {1: 2.0, 2: 4.0})  # True

# Differing keys still compare False even when values are NaN-identical.
print({1: nan} == {2: nan})  # False
