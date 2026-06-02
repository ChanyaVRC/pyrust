# Parity fixture for issue #2067: comprehension `for` targets accept the full
# grammar a `for` statement does -- parenthesized sub-targets, nested tuple
# unpacking, star targets, and single-element parenthesized targets.

data = [(1, (2, 3)), (4, (5, 6))]

# nested tuple unpacking in the target
print([a + b + c for a, (b, c) in data])

# parenthesized flat target
print([x for (x, y) in [(1, 2), (3, 4)]])

# star target
print([a for a, *b in [(1, 2, 3), (4, 5)]])
print([b for a, *b in [(1, 2, 3), (4, 5)]])

# single-element parenthesized target
print([a for (a,) in [(1,)]])

# deeply nested target
print([a for a, (b, (c, d)) in [(1, (2, (3, 4)))]])

# flat bare-name targets still work
print([a + b for a, b in [(1, 2), (3, 4)]])

# set / dict / genexpr forms with unpacking targets
print(sorted({a for (a, b) in [(1, 2), (3, 4)]}))
print({k: v for (k, v) in [(1, 10), (2, 20)]})
print(list(a for a, (b, c) in data))
