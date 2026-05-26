# Parity fixture for issue #1210: for-loop parenthesized tuple targets

# Basic parenthesized tuple target
for (x, y) in [(1, 2), (3, 4)]:
    print(x, y)

# Nested parenthesized tuple target
for (x, (y, z)) in [(1, (2, 3)), (4, (5, 6))]:
    print(x, y, z)

# No regression: bare comma tuple target
for x, y in [(7, 8), (9, 10)]:
    print(x, y)

# No regression: single variable
for x in [11, 12]:
    print(x)

# Single-element parenthesized tuple (trailing comma)
for (x,) in [(13,), (14,)]:
    print(x)

# Mixed: parenthesized sub-target with bare names
for x, (y, z) in [(1, (2, 3)), (4, (5, 6))]:
    print(x, y, z)

# Parenthesized target followed by bare name
for (x, y), z in [((1, 2), 3), ((4, 5), 6)]:
    print(x, y, z)

# Starred inside parenthesized target
for (x, *y) in [(1, 2, 3), (4, 5, 6)]:
    print(x, y)

# for-else with parenthesized target
for (a, b) in [(100, 200)]:
    print(a, b)
else:
    print("else branch")
