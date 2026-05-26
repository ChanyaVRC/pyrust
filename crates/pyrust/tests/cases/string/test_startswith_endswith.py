# Parity test: str.startswith() and str.endswith() type validation.
# CPython 3.12 raises TypeError if the first arg is not a str or tuple of str,
# and if any element in the tuple is not a str.

# Happy path: all-str tuple
print("hello".startswith(("he", "lo")))      # True
print("hello".startswith(("lo", "he")))      # True
print("hello".startswith(("xyz", "abc")))    # False
print("hello".endswith(("lo", "world")))     # True
print("hello".endswith(("world", "lo")))     # True
print("hello".endswith(("xyz", "abc")))      # False

# Happy path: single str argument (no tuple)
print("hello".startswith("he"))              # True
print("hello".startswith("lo"))              # False
print("hello".endswith("lo"))                # True
print("hello".endswith("he"))                # False

# TypeError: non-str/tuple first arg
try:
    "hello".startswith(42)
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    "hello".endswith(None)
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    "hello".startswith(b"he")
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    "hello".endswith(b"lo")
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# TypeError: no first argument
try:
    "hello".startswith()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    "hello".endswith()
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# TypeError: non-str element reached before any match
try:
    "hello".startswith((5, "he"))
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    "hello".endswith((42, "lo"))
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    "hello".startswith((None,))
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    "hello".endswith(("xyz", None))
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# Short-circuit: matching str comes before the non-str element -> no TypeError
print("hello".startswith(("he", 5)))         # True (short-circuits before 5)
print("hello".endswith(("lo", 42)))          # True (short-circuits before 42)

# Empty tuple
print("hello".startswith(()))                # False
print("hello".endswith(()))                  # False

# Inverted window (start > end) with non-str element: TypeError still raised.
# CPython validates element types even when the slice range is inverted.
try:
    "hello".startswith((5, "he"), 10, 1)
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    "hello".endswith((5, "lo"), 10, 1)
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# Inverted window with all-str tuple: False (no TypeError, no match possible)
print("hello".startswith(("he",), 10, 1))   # False
print("hello".endswith(("lo",), 10, 1))     # False
