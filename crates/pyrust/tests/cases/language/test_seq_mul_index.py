# Parity fixture: sequence repetition via __index__ (issue #1228).
#
# CPython 3.12 calls __index__ on the count operand before raising TypeError
# when one side is a built-in sequence (str/bytes/list/tuple).

class Count:
    def __index__(self):
        return 3


class Zero:
    def __index__(self):
        return 0


class Neg:
    def __index__(self):
        return -1


class BadIndex:
    def __index__(self):
        return "not_an_int"


class NoIndex:
    pass


class BigIndex:
    def __index__(self):
        return 10**100


class HasRMul:
    """Defines __rmul__ so try_dunder_binary handles it before __index__."""

    def __rmul__(self, other):
        return "rmul_result"

    def __index__(self):
        return 99  # should NOT be called when __rmul__ succeeds


# --- str ---
print("abc" * Count())
print(Count() * "abc")
print("abc" * Zero())
print("abc" * Neg())

# --- bytes ---
print(b"ab" * Count())
print(Count() * b"ab")

# --- list ---
print([1, 2] * Count())
print(Count() * [1, 2])
print([1, 2] * Zero())
print([1, 2] * Neg())

# --- tuple ---
print((1, 2) * Count())
print(Count() * (1, 2))

# --- __rmul__ takes priority over __index__ ---
print([1] * HasRMul())

# --- TypeError: count is str (no __index__) ---
try:
    [1, 2] * "abc"
except TypeError as e:
    print(e)

# --- TypeError: __index__ returns non-int ---
try:
    [1, 2] * BadIndex()
except TypeError as e:
    print(e)

# --- TypeError: no __index__ on user object ---
try:
    [1, 2] * NoIndex()
except TypeError as e:
    print(e)

# --- OverflowError: __index__ returns a BigInt (type name is original type) ---
try:
    [1, 2] * BigIndex()
except OverflowError as e:
    print(e)
