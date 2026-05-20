# Parity fixture for:
#   #847 — eval_slice catch-all should dispatch __getitem__ for BuiltinObject targets
#   #849 — slice_index_from_value should handle BigInt and the __index__ protocol

# --- __index__ protocol as slice bounds on built-in sequences (issue #849) ---
#
# CPython calls __index__ on each bound when a built-in sequence (list, str,
# tuple, bytes) processes a slice.

class Index:
    def __init__(self, n):
        self._n = n

    def __index__(self):
        return self._n


a = [10, 20, 30, 40, 50]

# __index__ start bound
print(a[Index(2):])               # [30, 40, 50]
# __index__ stop bound
print(a[:Index(3)])               # [10, 20, 30]
# __index__ step bound
print(a[::Index(2)])              # [10, 30, 50]
# all three bounds via __index__
print(a[Index(1):Index(4):Index(2)])  # [20, 40]

# __index__ bounds work for str, tuple, bytes too
s = "abcde"
print(s[Index(1):Index(4)])       # bcd
print(s[::Index(2)])              # ace

t = (0, 1, 2, 3, 4)
print(t[:Index(3)])               # (0, 1, 2)
print(t[Index(2)::Index(2)])      # (2, 4)

b = b"hello"
print(b[Index(1):Index(4)])       # b'ell'

# --- __index__ returning non-int raises TypeError ---

class BadIndex:
    def __index__(self):
        return "not an int"


try:
    _ = [1, 2, 3][BadIndex():]
except TypeError as e:
    print(type(e).__name__)       # TypeError

# --- BigInt literal bounds: clamped to sys.maxsize / -sys.maxsize-1 ---
# Integer literals large enough to overflow i64 are BigInt in pyrust.
# CPython clamps them to sys.maxsize and -sys.maxsize-1 on 64-bit platforms.

a2 = [0, 1, 2, 3]
print(a2[:9999999999999999999999999999])    # [0, 1, 2, 3]
print(a2[9999999999999999999999999999:])    # []
print(a2[:-9999999999999999999999999999])   # []
print(a2[-9999999999999999999999999999:])   # [0, 1, 2, 3]

# --- UserClass.__getitem__ receives raw bounds, not resolved integers ---
#
# CPython does NOT call __index__ before building the slice object passed
# to a user-defined __getitem__.  Resolution via __index__ happens downstream,
# when the user code passes the slice to a built-in sequence.

class Inspector:
    """Records the raw slice object received by __getitem__."""
    def __getitem__(self, idx):
        if isinstance(idx, slice):
            return (type(idx.start).__name__, type(idx.stop).__name__, idx.step)
        return idx


ins = Inspector()
# Bounds are Index objects — not resolved integers
result = ins[Index(1):Index(3)]
print(result[0])    # Index
print(result[1])    # Index
print(result[2])    # None

# --- UserClass with __getitem__ that delegates to a list ---
#
# The user's __getitem__ receives the slice with raw Index bounds, then
# passes it to list.__getitem__, which calls __index__ on each bound.

class Seq:
    def __init__(self, data):
        self.data = data

    def __getitem__(self, index):
        return self.data[index]


seq = Seq([0, 1, 2, 3, 4, 5])
print(seq[Index(2):])             # [2, 3, 4, 5]
print(seq[:Index(4)])             # [0, 1, 2, 3]
print(seq[Index(1)::Index(2)])    # [1, 3, 5]

# --- obj[start::step] syntax (varied step values, issue #847 scope) ---
class Counter:
    def __getitem__(self, idx):
        if isinstance(idx, slice):
            return (idx.start, idx.stop, idx.step)
        return idx


ctr = Counter()
print(ctr[::2])     # (None, None, 2)
print(ctr[1::3])    # (1, None, 3)
print(ctr[2:10:])   # (2, 10, None)
