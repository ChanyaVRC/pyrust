# Repeated entry into a loop over a small dict (#2890).  A container under the
# eager key-order capture size pays its whole iteration setup once per loop
# entry, so this case is dominated by that setup rather than by the walk.
d = {i: i * 2 for i in range(32)}
total = 0
for _ in range(1_000_000):
    for k in d:
        total += k
