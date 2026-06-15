# Sequence operations short-circuit on identity before calling __eq__,
# mirroring CPython's PyObject_RichCompareBool.  NaN is the only primitive
# where this is observable (`x == x` is False), so a NaN searching for
# *itself* is found even though plain equality misses it.
#
# Caveat: pyrust's floats are NaN-boxed values, not heap objects, so two
# *distinct* `float('nan')` calls can't be told apart and would compare equal
# under our bit-pattern approximation — so this fixture deliberately only
# asserts the same-NaN-object cases that a value representation can match
# (it never asserts `float('nan') in [float('nan')]`, which CPython reports
# as False).

n = float("nan")

# membership (`in`)
print(n in [n])
print(n in (n,))
print(n in [1, n, 2])
print(n in (1, n, 2))

# list / tuple .index
print([n].index(n))
print([1, n, 2].index(n))
print((n,).index(n))

# list / tuple .count
print([n].count(n))
print([n, n, n].count(n))
print((n, n).count(n))

# list / tuple ==
print([n] == [n])
print((n,) == (n,))
print([1, n] == [1, n])
print([[n]] == [[n]])

# list.remove finds the NaN by identity
L = [1, n, 2]
L.remove(n)
print(L)

# bare equality is unaffected: n == n is still False
print(n == n)
print(n != n)

# integers (and other primitives) are unaffected by the identity rule
print(1 in [1])
print([1].index(1))
print([1, 1, 1].count(1))
print([1, 2] == [1, 2])

# user object whose __eq__ returns False is still found by identity
class W:
    def __eq__(self, other):
        return False

w = W()
print(w in [w])
print([w].index(w))
print([w].count(w))
print([w] == [w])
