# Issue #2490: dir(float)/dir(complex) instances must include their slot
# dunders (__add__, __trunc__, __neg__, …) and instance methods, matching
# hasattr and CPython 3.12.  Previously the Float/Complex dir arm returned only
# the universal object dunders, so '__add__' in dir(1.7) was False even though
# hasattr(1.7, '__add__') was True.


# The slot dunders called out in the issue now appear in dir(1.7).
for name in ("__add__", "__sub__", "__mul__", "__truediv__", "__floordiv__",
             "__mod__", "__pow__", "__divmod__", "__neg__", "__pos__",
             "__abs__", "__round__", "__trunc__", "__floor__", "__ceil__",
             "__bool__"):
    print(name, name in dir(1.7))

# Reflected operators are present too.
for name in ("__radd__", "__rsub__", "__rmul__", "__rtruediv__",
             "__rfloordiv__", "__rmod__", "__rpow__", "__rdivmod__"):
    print(name, name in dir(1.7))

# Instance methods / properties present in dir(1.7).
for name in ("conjugate", "hex", "is_integer", "as_integer_ratio",
             "fromhex", "real", "imag"):
    print(name, name in dir(1.7))

# complex slot dunders + conjugate + real/imag in dir(1j).
for name in ("__add__", "__sub__", "__mul__", "__truediv__", "__pow__",
             "__neg__", "__pos__", "__abs__", "__bool__", "conjugate",
             "real", "imag"):
    print(name, name in dir(1j))

# complex does NOT advertise float-only names.
for name in ("__trunc__", "__floordiv__", "__mod__", "hex", "is_integer"):
    print(name, name in dir(1j))

# dir() is consistent with hasattr for every name it reports (the invariant
# the bug is about): nothing in dir(1.7)/dir(1j) is unresolvable.
print(all(hasattr(1.7, n) for n in dir(1.7)))
print(all(hasattr(1j, n) for n in dir(1j)))

# Enumeration is value-agnostic across boundary floats.
print(dir(0.0) == dir(1.7))
print(dir(-0.0) == dir(1.7))
print(dir(float("nan")) == dir(1.7))
print(dir(float("inf")) == dir(1.7))

# dir() result is sorted and deduplicated.
d = dir(1.7)
print(d == sorted(d))
print(len(d) == len(set(d)))
