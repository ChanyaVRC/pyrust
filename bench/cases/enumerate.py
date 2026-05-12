xs = [i for i in range(5_000_000)]
total = 0
for k, v in enumerate(xs):
    total += k + v
