# Subscript with comma-separated indices parses as a tuple key.
# Mirrors CPython: `a[b, c]` is equivalent to `a[(b, c)]`.

# Multi-arg generic in a type annotation — the canonical motivating use case.
# (tuple[int, str] requires __class_getitem__ which is not yet implemented;
# use plain type annotation here and verify the subscript parsing separately.)
x: tuple = (1, "a")
assert x == (1, "a")

# Same shape on a function signature.
def f(d: dict[str, int]) -> tuple[int, str]:
    return (1, "a")

assert f({}) == (1, "a")

# User-defined __getitem__ receives a tuple for comma-separated indices.
class M:
    def __getitem__(self, key):
        return key

m = M()
assert m[1, 2] == (1, 2)
assert m[1, 2] == m[(1, 2)]

# Trailing-comma single index forms a 1-tuple, matching CPython.
assert m[1,] == (1,)
assert m[1,] == m[(1,)]

# Whitespace before/after commas is fine.
assert m[1 , 2] == (1, 2)

print("subscript tuple OK")
