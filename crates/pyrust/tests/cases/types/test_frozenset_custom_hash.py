class C:
    def __hash__(self): return 42
    def __eq__(self, other): return isinstance(other, C)

c = C()
fs = frozenset([c])
print(c in fs)      # True
print(C() in fs)    # True  (same hash and eq)
print(1 in fs)      # False

# frozenset of mixed types
fs2 = frozenset([1, c, "hello"])
print(c in fs2)     # True
print(2 in fs2)     # False
