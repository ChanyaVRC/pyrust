# complex class-method access
print(complex.conjugate(1+2j))
print(complex.conjugate(3-4j))
print(complex.conjugate(0+0j))

# frozenset class-method access
fs = frozenset([1, 2, 3])
print(frozenset.copy(fs))
print(frozenset.union(fs, {4, 5}))
print(frozenset.intersection(fs, {2, 3, 4}))
print(frozenset.difference(fs, {2}))
print(frozenset.symmetric_difference(fs, {2, 3, 4}))
print(frozenset.issubset(frozenset([1, 2]), fs))
print(frozenset.issuperset(fs, frozenset([1])))
print(frozenset.isdisjoint(fs, {4, 5}))

# Instance methods still work (no regression)
print((1+2j).conjugate())
print(frozenset([1]).copy())
