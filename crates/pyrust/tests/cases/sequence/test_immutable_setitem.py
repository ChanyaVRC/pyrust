# Parity fixture for issue #932:
# Subscript assignment on immutable types must raise TypeError, not RuntimeError,
# with the message matching CPython 3.12 exactly.

# --- tuple: indexed assignment ---
t = (1, 2, 3)
try:
    t[0] = 99
except TypeError as e:
    print("tuple setitem:", e)

# --- str: indexed assignment ---
s = "hello"
try:
    s[0] = "x"
except TypeError as e:
    print("str setitem:", e)

# --- bytes: indexed assignment ---
b = b"hello"
try:
    b[0] = 99
except TypeError as e:
    print("bytes setitem:", e)

# --- frozenset: indexed assignment ---
fs = frozenset([1, 2])
try:
    fs[0] = 1
except TypeError as e:
    print("frozenset setitem:", e)

# --- range: indexed assignment ---
r = range(5)
try:
    r[0] = 1
except TypeError as e:
    print("range setitem:", e)

# --- tuple: slice assignment ---
t2 = (1, 2, 3)
try:
    t2[0:2] = [99]
except TypeError as e:
    print("tuple slice setitem:", e)

# --- str: slice assignment ---
s2 = "hello"
try:
    s2[0:2] = "xy"
except TypeError as e:
    print("str slice setitem:", e)

# --- tuple: indexed deletion ---
t3 = (1, 2, 3)
try:
    del t3[0]
except TypeError as e:
    print("tuple delitem:", e)

# --- tuple: slice deletion ---
t4 = (1, 2, 3)
try:
    del t4[0:1]
except TypeError as e:
    print("tuple slice delitem:", e)

# --- error is catchable as TypeError (not RuntimeError) ---
caught_as_type_error = False
try:
    (1, 2)[0] = 9
except TypeError:
    caught_as_type_error = True
print("catchable as TypeError:", caught_as_type_error)
