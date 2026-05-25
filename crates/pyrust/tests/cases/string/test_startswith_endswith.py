# Parity test: str.startswith() and str.endswith() with tuple arguments.
# CPython 3.12 raises TypeError if any element in the tuple is not a str,
# but short-circuits (returns True) if a matching str is found before the bad element.

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
