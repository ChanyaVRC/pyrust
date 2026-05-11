# Comprehensions: list, dict, set; with condition; nested; multiple for-clauses

# Basic list comprehension
result = [x * 2 for x in range(5)]
print("list-basic", result)

# List comprehension with condition
evens = [x for x in range(10) if x % 2 == 0]
print("list-cond", evens)

# List comprehension with expression
squares = [x * x for x in range(6)]
print("list-squares", squares)

# Nested list comprehension (two for-clauses)
pairs = [x + y for x in range(3) for y in range(3)]
print("list-nested", pairs)

# Nested with condition on outer and inner
filtered = [x * y for x in range(1, 4) for y in range(1, 4) if x != y]
print("list-nested-cond", filtered)

# Dict comprehension
d = {k: v for k, v in [("a", 1), ("b", 2), ("c", 3)]}
print("dict-basic", d)

# Dict comprehension with condition
d2 = {k: v for k, v in [("a", 1), ("b", 2), ("c", 3)] if v > 1}
print("dict-cond", d2)

# Dict comprehension building squares
sq_dict = {x: x * x for x in range(5)}
print("dict-squares", sq_dict)

# Set comprehension
s = {x * 2 for x in range(5)}
print("set-basic", sorted(s))

# Set comprehension with condition (deduplication)
s2 = {x % 3 for x in range(9)}
print("set-dedup", sorted(s2))

# Set comprehension with condition filter
s3 = {x for x in range(10) if x % 2 == 0}
print("set-cond", sorted(s3))

# Comprehension over string
chars = [c for c in "hello" if c != "l"]
print("list-str", chars)

# Comprehension result used in another expression
total = sum([x * x for x in range(5)])
print("list-sum", total)
