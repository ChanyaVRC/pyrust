d = {}
for i in range(1000):
    d[i] = i * 2
total = 0
for k in d:
    total += k
assert total == 499500

xs = [0] * 1000
for i in range(1000):
    xs[i] = i
s = 0
for x in xs:
    s += x
assert s == 499500
