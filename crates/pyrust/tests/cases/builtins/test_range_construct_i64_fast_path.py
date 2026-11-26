# range() construction: the all-i64 fast path (#3376) must stay observably
# identical to the general __index__/BigInt path — values, bool args, i64
# boundaries, BigInt fallback, custom __index__ and the CPython error wording.

# --- plain int / bool shapes ---
print(range(5), list(range(5)))
print(range(2, 7), list(range(2, 7)))
print(range(0, 10, 3), list(range(0, 10, 3)))
print(range(10, 0, -3), list(range(10, 0, -3)))
print(range(-3), list(range(-3)), len(range(-3)))
print(range(True), list(range(True)))
print(range(False), list(range(False)))
print(range(False, True), list(range(False, True)))
print(range(True, 5, True), list(range(True, 5, True)))
print(range(0, 6, True), list(range(0, 6, True)))

# --- attributes are plain ints, also for bool args ---
r = range(True, 4, True)
print(r.start, r.stop, r.step)
print(type(r.start).__name__, type(r.stop).__name__, type(r.step).__name__)

# --- i64 boundaries stay on the i64 representation and keep their semantics ---
M = 2**63 - 1
N = -(2**63)
print(range(M), len(range(M - 3, M)))
print(list(range(M - 3, M)))
print(list(range(N, N + 3)))
print(list(range(N + 3, N, -1)))
try:
    print(len(range(N, M)))
except OverflowError as e:
    print("OverflowError", e)
print(range(N, M, M))
print(M - 1 in range(M), M in range(M), N in range(N, 0))

# --- values just past i64 fall back to the BigInt range ---
print(range(2**63), len(range(2**63 - 3, 2**63)))
print(range(N - 1, N + 2), list(range(N - 1, N + 2)))
print(range(0, 10, 2**64), list(range(0, 10, 2**64)))
print(list(range(10, 0, -(2**64))))
print(range(1, 2**63).step, range(2**70, 2**70 + 3))

# --- mixed fast / slow argument shapes ---
print(list(range(0, 3, 2**65)), list(range(2**65, 2**65 + 2)))
print(list(range(True, 2**65, 2**64)) == [1, 1 + 2**64])


# --- __index__ objects, including int subclasses ---
class Idx:
    def __init__(self, n):
        self.n = n

    def __index__(self):
        return self.n


class MyInt(int):
    pass


class WeirdInt(int):
    def __index__(self):
        return 3


print(list(range(Idx(4))))
print(list(range(Idx(1), Idx(6), Idx(2))))
print(list(range(1, Idx(5))))
print(list(range(Idx(2), 5)))
print(list(range(MyInt(4))))
print(list(range(MyInt(1), MyInt(7), MyInt(3))))
print(list(range(WeirdInt(10))))
print(list(range(0, 10, MyInt(4))))
print(range(Idx(10**20)))


# --- __index__ raising / returning garbage propagates ---
class Boom:
    def __index__(self):
        raise RuntimeError("boom")


class BadIdx:
    def __index__(self):
        return "x"


for bad in (Boom, BadIdx):
    try:
        range(bad())
    except Exception as e:
        print(type(e).__name__, e)

# --- error types and messages ---
for args in [
    (0,),
    (0, 5, 0),
    (True, 5, False),
    (0, 5, 2**64 * 0),
]:
    try:
        print(range(*args))
    except Exception as e:
        print(type(e).__name__, e)

for call in (
    lambda: range(1.5),
    lambda: range(0, 2.5),
    lambda: range(0, 5, 1.0),
    lambda: range("3"),
    lambda: range(None),
    lambda: range(1, None),
    lambda: range(1, 2, [3]),
    lambda: range(),
    lambda: range(1, 2, 3, 4),
    lambda: range(1, 2, 3, 4, 5),
    lambda: range(stop=3),
    lambda: range(1, step=2),
    lambda: range(0, 5, 0),
    lambda: range(0, 5, 2**70 * 0),
    lambda: range(0, 5, False),
    lambda: range(2**70, 5, 0),
):
    try:
        print(call())
    except Exception as e:
        print(type(e).__name__, e)

# --- equality / hash / iteration unaffected ---
print(range(5) == range(0, 5, 1), hash(range(5)) == hash(range(0, 5, 1)))
print(range(True) == range(1), range(0, 0) == range(5, 5))
total = 0
for _ in range(3):
    for j in range(2):
        total += j + 1
print(total)
for _ in range(1):
    print("once")
for _ in range(0):
    print("never")
