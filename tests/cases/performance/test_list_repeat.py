xs = [0] * 10000
total = 0
for i in range(10000):
    xs[i] = i
for i in range(10000):
    total += xs[i]
assert total == 49995000
