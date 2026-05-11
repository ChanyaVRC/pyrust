xs = [0] * 200
for i in range(200):
    xs[i] = 199 - i
for i in range(1, 200):
    key = xs[i]
    j = i - 1
    while j >= 0 and xs[j] > key:
        xs[j + 1] = xs[j]
        j -= 1
    xs[j + 1] = key
for i in range(200):
    assert xs[i] == i
