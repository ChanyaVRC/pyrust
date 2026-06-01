# Set comprehensions lowered via the dedicated SetAdd opcode (issue #1861).
# Covers dedup, conditions, multiple for-clauses, nesting, generators,
# walrus targets, and user __hash__/__eq__ dedup.

# Basic dedup over a modulo
print("mod3", sorted({x % 3 for x in range(10)}))

# Multiple for-clauses with a condition
print("pairs", sorted({(x, y) for x in range(3) for y in range(3) if x != y}))

# Empty set comprehension is still a set, not a dict
empty = {x for x in range(0)}
print("empty", empty, type(empty).__name__)

# All elements collapse to one
print("single", {42 for _ in range(5)})

# Comprehension over a generator expression
print("gen", sorted({i * i for i in (j for j in range(6))}))

# Nested condition across two clauses
print("multi-cond", sorted({a + b for a in range(3) if a > 0 for b in range(3) if b < a}))

# Walrus inside the element expression leaks to the enclosing scope
print("walrus", sorted({(n := i) + 1 for i in range(4)}))
print("walrus-leak", n)

# Set comprehension nested inside another comprehension
print("nested", sorted(len({y for y in range(x)}) for x in range(4)))

# Elements that are themselves frozensets
print("frozen", sorted({frozenset({x, x + 1}) for x in range(3)}, key=lambda f: sorted(f)))


# User-defined __hash__/__eq__ drives dedup
class C:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return self.v % 2

    def __eq__(self, other):
        return isinstance(other, C) and self.v % 2 == other.v % 2

    def __repr__(self):
        return f"C({self.v})"


s = {C(i) for i in range(6)}
print("user-dedup-len", len(s))
print("user-dedup-keys", sorted(c.v % 2 for c in s))

# None dedup
print("none-dedup", len({None for _ in range(3)}))
