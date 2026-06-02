# Parity fixture for issue #2031: comprehensions accept any number of chained
# `if` filters per `for` clause (CPython `comp_if: 'if' or_test (comp_if)?`).
# The filters AND together with short-circuit semantics.
#
# Covers list / set / dict comprehensions and generator expressions, plus a
# single-`if` and no-`if` control case so we catch regressions.

# list comprehension, two and three chained `if`s
print([x for x in range(20) if x % 2 == 0 if x % 3 == 0])
print([x for x in range(10) if x > 2 if x < 8 if x != 5])

# set comprehension (sort for stable output across set hash order)
print(sorted({x for x in range(10) if x % 2 if x > 3}))

# dict comprehension
print({x: x for x in range(10) if x % 2 if x > 3})

# generator expression
print(list(x for x in range(10) if x > 2 if x < 8))

# multiple `if`s on each of multiple `for` clauses (filters reference the
# loop variable bound by their own clause only)
print([x * y for x in range(3) for y in range(3) if x if y])
print([(x, y) for x in range(4) if x > 0 if x < 3 for y in range(4) if y > 0 if y < 3])

# single-`if` and no-`if` still work
print([x for x in range(5) if x > 1])
print([x for x in range(3)])
