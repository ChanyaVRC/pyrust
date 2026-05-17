# Parity fixture for issue #502: hash() of a tuple containing a PyInstance
# with a custom __hash__ must dispatch __hash__ rather than raising TypeError.
#
# Note: the exact hash values produced by pyrust's tuple mixing formula differ
# from CPython's (tracked separately in issue #522).  This fixture checks
# determinism and correct __hash__ dispatch, not exact integer values.

class C:
    def __hash__(self): return 42
    def __eq__(self, other): return isinstance(other, C)

c = C()

# Plain PyInstance hash still works.
print(hash(c))

# Tuple containing a custom-hash instance: determinism.
h1 = hash((c,))
h2 = hash((c,))
print(h1 == h2)

# Mixed primitive + custom-hash instance: determinism.
print(hash((1, c, "x")) == hash((1, c, "x")))

# Tuple as dict key round-trip.
d = {(c,): "val"}
print(d[(c,)])

# Pure primitive tuple still works (regression guard).
print(hash((1, 2, 3)) == hash((1, 2, 3)))

# Nested tuple containing a custom-hash instance.
print(hash(((c,),)) == hash(((c,),)))

# __hash__ = None inside a tuple raises TypeError.
class Unhashable:
    __hash__ = None

try:
    hash((Unhashable(),))
    print("ERROR: expected TypeError")
except TypeError as e:
    print(type(e).__name__)
