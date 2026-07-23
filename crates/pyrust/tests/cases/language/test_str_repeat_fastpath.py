# str repetition fast path: tagged `str * int` / `int * str` skips the dunder /
# numeric-slot dispatch and builds the result in a single reserved buffer.
# Must match CPython 3.12 for both operand orders, bool counts, negative/zero
# counts, multibyte text, and — critically — must NOT bypass a user __mul__ /
# __rmul__ on str/int subclasses (those are PyInstances, not tagged values).

# Both operand orders, various counts (read from a list so nothing const-folds).
texts = ["", "a", "ab", "héllo", "x" * 3]
counts = [0, 1, 2, 3, 5, -1, -4]
for t in texts:
    for n in counts:
        print(repr(t * n), repr(n * t))

# Bool counts (True==1, False==0).
print(repr("ab" * True), repr("ab" * False), repr(True * "cd"), repr(False * "cd"))

# Length sanity for a larger repeat (doubling fill).
print(len("abcdefgh" * 1000))
print(("ab" * 50) == "ab" * 50)

# str subclass with __mul__ must win (not the fast path).
class S(str):
    def __mul__(self, other):
        return "S.__mul__"
    def __rmul__(self, other):
        return "S.__rmul__"


print(S("z") * 3)
print(3 * S("z"))

# int subclass with __rmul__ / __mul__.
class I(int):
    def __rmul__(self, other):
        return "I.__rmul__"


print("z" * I(2))  # tagged str * PyInstance int -> int subclass __rmul__

# Plain str * plain str is still a TypeError (fast path only matches str*int).
try:
    "a" * "b"
except TypeError as e:
    print("TypeError:", e)

# str * huge int -> OverflowError (count exceeds index-sized range handling).
try:
    "ab" * (10**30)
except (OverflowError, MemoryError) as e:
    print(type(e).__name__)
