# Lazy list iteration
lst = list(range(10))
total = 0
for x in lst:
    total += x
print(total)  # 45

# Tuple unpacking
pairs = [(0, 0), (1, 2), (2, 4), (3, 6), (4, 8)]
s = 0
for a, b in pairs:
    s += a + b
print(s)  # 0+3+6+9+12 = 30

# Nested tuple unpack
triples = [(1, 2, 3), (4, 5, 6)]
total = 0
for a, b, c in triples:
    total += a + b + c
print(total)  # 6 + 15 = 21
