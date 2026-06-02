# Parity fixture for sum() numeric accumulation (#1975 / #2050).
#
# CPython 3.12 uses Neumaier (Kahan-Babuska) compensated summation for the
# float fast path, an i64-ish int fast path that drops to generic addition on
# overflow / big ints, and a generic __add__ fallback.  These cases pin the
# bit-exact behaviour against CPython 3.12.

# --- Neumaier compensated float sums (#1975) ---
print(repr(sum([0.1] * 10)))             # 1.0
print(repr(sum([0.1, 0.2, 0.3])))        # 0.6
print(repr(sum([1e16, 1.0, -1e16])))     # 1.0
print(repr(sum([1.0, 1e100, 1.0, -1e100])))  # 2.0
print(repr(sum([0.1] * 1000)))           # 100.0

# Infinities keep their sign (compensation term is dropped when non-finite).
print(repr(sum([float("inf"), 1.0])))    # inf
print(repr(sum([1.0, float("inf")])))    # inf
print(repr(sum([float("inf"), float("-inf")])))  # nan
print(repr(sum([1e308, 1e308])))         # inf

# --- int / bigint exactness ---
print(sum(range(10)))                    # 45
print(sum([2**62, 2**62]))               # 2**63  (i64 overflow -> generic)
print(sum([10**30, 1, 2]))               # exact big int
print(sum([1, 2**100, 3]))               # int fast path then big int
print(sum([-(2**63), -1]))               # negative overflow

# --- mixed int + float, transition order matters ---
print(repr(sum([1, 2, 0.5])))            # 3.5
print(repr(sum([1, 1e16, 1.0, -1e16])))  # 1.0
print(repr(sum([1, 0.1] * 5)))           # 5.500000000000001
print(repr(sum([2**70, -1e100, 1e100], -3.5)))  # 0.0  (big int -> generic)

# --- start handling ---
print(sum([1, 2, 3], 10))                # 16
print(repr(sum([1.5, 2], 0)))            # 3.5
print(repr(sum([1e16, 1.0, -1e16], 1.0)))  # 2.0  (float start)
print(repr(sum([], 5)))                  # 5
print(repr(sum([], 5.0)))                # 5.0
print(repr(sum([], True)))               # True  (start returned unchanged)
print(repr(sum([1, 2], True)))           # 4     (bool start -> generic)

# --- bool elements ---
print(sum([True, True, False, 1]))       # 3

# --- generator input (lazy iteration) ---
print(sum(x for x in range(1000)))       # 499500
print(repr(sum(x * 0.5 for x in range(10))))  # 22.5

# --- non-numeric: list / tuple concat and user __add__ ---
print(sum([[1], [2], [3]], []))          # [1, 2, 3]
print(sum([(1,), (2,)], ()))             # (1, 2)


class V:
    def __init__(self, n):
        self.n = n

    def __add__(self, other):
        return V(self.n + (other.n if isinstance(other, V) else other))

    def __radd__(self, other):
        return V((other.n if isinstance(other, V) else other) + self.n)

    def __repr__(self):
        return "V(%d)" % self.n


print(sum([V(1), V(2), V(3)]))           # V(6)  (start 0 via __radd__)
print(sum([V(1), V(2)], V(10)))          # V(13)

# --- error cases ---
for code in [
    'sum(["a", "b"])',
    'sum([1, 2], "x")',
    'sum([], bytearray())',
    'sum([b"a"])',
    'sum([bytearray(b"a")])',
    'sum([1, "x"])',
    'sum(5)',
]:
    try:
        eval(code)
        print(code, "-> NO ERROR")
    except TypeError as e:
        print(code, "->", e)
