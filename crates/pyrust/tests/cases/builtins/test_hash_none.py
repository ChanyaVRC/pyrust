"""Parity fixture for issue #906: PyKey::None must hash via py_hash_none()
so that a user-defined object whose __hash__ returns hash(None) and whose
__eq__ matches None can be found in dicts and sets that contain None, and
vice versa."""


class NoneAlike:
    """Object whose hash and equality match None."""

    def __hash__(self):
        return hash(None)

    def __eq__(self, other):
        return other is None or isinstance(other, NoneAlike)


obj = NoneAlike()

# Case 1: store under None, look up with the alias object.
d1 = {None: "found_none"}
print("Case 1 - store None, lookup obj:", d1.get(obj, "MISSING"))

# Case 2: store under the alias object, look up with None.
d2 = {obj: "found_obj"}
print("Case 2 - store obj, lookup None:", d2.get(None, "MISSING"))

# Case 3: set membership — obj in {None}.
s = {None}
print("Case 3 - obj in set {None}:", obj in s)

# Case 4: set membership — None in {obj}.
s2 = {obj}
print("Case 4 - None in set {obj}:", None in s2)

# Case 5: set deduplication — {None, obj} must collapse to one element.
dedup1 = {None, obj}
print("Case 5 - len({None, obj}):", len(dedup1))

# Case 6: set deduplication in the other insertion order.
dedup2 = {obj, None}
print("Case 6 - len({obj, None}):", len(dedup2))

# Regression: native None lookups still work correctly.
d_native = {None: 42}
print("Regression - d[None]:", d_native[None])
print("Regression - None in {None}:", None in {None})

# Issue #902: hash(None) must not equal 0 or False's hash (which is 0),
# and must not equal the Python hash error sentinel -1.
print("hash(None) != hash(0):", hash(None) != hash(0))
print("hash(None) != hash(False):", hash(None) != hash(False))
print("hash(None) != -1:", hash(None) != -1)
