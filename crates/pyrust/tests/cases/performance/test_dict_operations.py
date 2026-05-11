d = {}
for i in range(1000):
    d[i] = i * i
total = 0
for i in range(1000):
    total += d[i]
assert total == 332833500
