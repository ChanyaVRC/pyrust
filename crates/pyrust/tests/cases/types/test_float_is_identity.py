# `is` identity for floats: aliased floats are the same object (#2527).
# NaN-boxed floats are bit-copied on clone, so an aliased binding shares the
# same value and `is` must hold — matching CPython object identity.

# Aliased non-NaN float.
a = 1.5
b = a
print(b is a)        # True
print(a is not b)    # False

# Aliased NaN: same binding is identical even though `==` is False.
nan = float("nan")
print(nan is nan)    # True
x = nan
print(x is nan)      # True
print(nan == nan)    # False (NaN never equals itself)

# Aliased inf.
inf = float("inf")
print(inf is inf)    # True

# Aliased -0.0.
z = -0.0
print(z is z)        # True

# Distinct float values stay non-identical.
print((1.5) is (2.5))  # False

# 0.0 and -0.0 have different bit patterns -> distinct identity.
p = 0.0
n = -0.0
print(p is n)        # False
print(p == n)        # True (0.0 == -0.0 numerically)

# Caveat (value-boxing tradeoff): two separately-constructed NaN objects are
# bit-identical here, so we intentionally do NOT assert the CPython
# distinct-NaN-objects case (CPython: False) — it cannot be matched in a value
# representation and is out of scope for this fixture.
