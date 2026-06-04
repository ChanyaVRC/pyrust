# range() with arbitrary-precision (BigInt) bounds — issue #2118.
# CPython's range is arbitrary-precision; pyrust used to raise OverflowError for
# any bound outside i64.  These exercise construction / repr / len / contains /
# index / count / getitem / slice / iteration / attributes for big-bound ranges.

# --- Construction + repr ---
print(range(0, 10**19, 2))
print(range(10**20))
print(range(10**19, 10**19 + 5))
print(repr(range(0, 10**19, 2)))

# --- len (and the length-overflow OverflowError) ---
print(len(range(0, 10**19, 3)))
print(len(range(0, 2**63 - 1)))
try:
    len(range(10**20))
except OverflowError as e:
    print("OverflowError:", e)

# --- attributes (start/stop/step are plain ints) ---
r = range(0, 10**19, 2)
print(r.start, r.stop, r.step)
print(type(range(10**20).step).__name__)
print(type(range(10**20).stop).__name__)

# --- membership ---
# NOTE: only *integer* membership is O(1) in CPython.  A non-member *float*
# (e.g. `5.5 in range(10**20)`) makes CPython fall back to a linear scan that
# would never finish, so such cases are intentionally omitted here.
print(10**18 in r)
print(10**18 + 1 in r)
print(10**20 - 1 in range(10**20))
print(-(10**19) + 2 in range(0, -(10**19), -2))

# --- index / count ---
print(r.index(10**18))
print(range(10**20).count(10**19 * 2))
try:
    r.index(10**18 + 1)
except ValueError as e:
    print("ValueError:", e)

# --- getitem (positive, negative, out-of-range) ---
print(r[100])
print(range(10**19, 10**19 + 5)[0])
print(range(0, -(10**19), -2)[5])
print(range(10**20)[-1])
try:
    range(10**20)[10**21]
except IndexError as e:
    print("IndexError:", e)

# --- slicing (resolved in arbitrary precision; length may exceed i64) ---
print(r[10:13])
print(range(10**20)[:5])
print(range(10**20)[10**18:10**18 + 3])
print(range(10**20)[-3:])
print(range(10**20)[::10**19])
print(range(0, 10**20, 2)[10**5:10**5 + 4])
print(range(10**20)[::-1][:3])

# --- iteration (lazy; never materialises a huge range) ---
print(list(range(10**19, 10**19 + 5)))
print([x for x in range(10**19, 10**19 - 6, -2)])
print(list(reversed(range(10**19, 10**19 + 4))))
print(sum(range(10**19, 10**19 + 5)))
print(list(map(lambda x: x * 2, range(10**19, 10**19 + 3))))
print([p for p in zip(range(10**19, 10**19 + 3), range(3))])

# --- lazy iter() over an out-of-i64-length range does not materialise ---
it = iter(range(10**20))
print(next(it), next(it), next(it))
print(type(it).__name__)  # CPython: longrange_iterator
e = enumerate(range(10**20), 10**30)
print(next(e), next(e))

# --- for-loop counter crossing the i64 boundary ---
out = []
for i in range(2**63 - 2, 2**63 + 2):
    out.append(i)
print(out)

# --- truthiness ---
print(bool(range(10**20, 10**20)))
print(bool(range(0, 10**20)))

# --- equality + hashability ---
print(range(10**20) == range(10**20))
print(hash(range(10**20)) == hash(range(10**20)))

# --- small (i64) ranges are unaffected ---
print(range(5), list(range(5)), len(range(0, 10, 2)), range(0, 10, 2)[3])
