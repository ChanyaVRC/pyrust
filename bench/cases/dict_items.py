# Tight loop over `dict.items()`.  Each iteration produces a fresh 2-tuple
# `(key, value)`; this benchmark targets the small-tuple inline-storage
# optimisation tracked in #281.
d = {i: i * 2 for i in range(1_000_000)}
total = 0
for k, v in d.items():
    total += k + v
