# Parity fixture for str.maketrans() and str.translate().
# All edge cases exercised here correspond to CPython 3.12 behaviour.
# Non-ASCII characters are not printed directly; use ord()/repr() where needed.

# --- str.maketrans: 2-arg form (from/to strings) ---
t = str.maketrans("abc", "xyz")
print(t)                                  # {97: 120, 98: 121, 99: 122}
print("abc123".translate(t))              # xyz123
print("abcabc".translate(t))             # xyzxyz

# --- str.maketrans: 3-arg form (from/to + delete) ---
t2 = str.maketrans("aeiou", "12345", "!?")
print("hello, world!?".translate(t2))    # h2ll4, w4rld

# --- str.maketrans: 1-arg dict form ---
t3 = str.maketrans({ord("a"): "A", ord("b"): None, ord("c"): ord("C")})
print(t3)                                 # {97: 'A', 98: None, 99: 67}
print("abcabc".translate(t3))            # ACAC

# --- translate: deletion ---
print("hello".translate({ord("l"): None}))  # heo

# --- translate: one-to-many replacement ---
print("hello".translate({ord("h"): "HH"}))  # HHello

# --- translate: int value (replace with chr(n)) ---
t4 = str.maketrans("abc", "xyz")          # values are ints (ordinals)
print("abc".translate(t4))               # xyz

# --- translate: char absent from table is kept ---
print("hello".translate({ord("x"): "Y"}))  # hello (no x in 'hello')

# --- translate: empty string ---
print("".translate(str.maketrans("abc", "xyz")))  # (empty)

# --- translate: empty table ---
print("hello".translate({}))             # hello

# --- str.maketrans: 3-arg delete overrides 2-arg mapping ---
# z chars delete even if they appear in x too
t5 = str.maketrans("ab", "xy", "a")
print("abc".translate(t5))               # yc  (a deleted, b→y, c unchanged)

# --- translate via instance method form ---
t6 = "".maketrans("abc", "xyz")
print("abcdef".translate(t6))            # xyzdef

# --- hasattr ---
print(hasattr("", "translate"))          # True
print(hasattr("", "maketrans"))          # True
print(hasattr(str, "translate"))         # True
print(hasattr(str, "maketrans"))         # True

# --- error cases ---
try:
    str.maketrans()
except TypeError as e:
    print(e)

try:
    str.maketrans("a", "b", "c", "d")
except TypeError as e:
    print(e)

try:
    str.maketrans("abc")
except TypeError as e:
    print(e)

try:
    str.maketrans("abc", "xy")
except ValueError as e:
    print(e)

try:
    str.maketrans({"ab": "A"})
except ValueError as e:
    print(e)

try:
    "abc".translate({ord("a"): 1.5})
except TypeError as e:
    print(e)

try:
    "abc".translate(0, 1)
except TypeError as e:
    print(e)
