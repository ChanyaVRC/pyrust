# Issue #2760: __getnewargs__ on tuple / str / bytes / int / float / bool /
# complex (the pickle-protocol hook).  All return a 1-tuple containing self,
# EXCEPT complex (a 2-tuple of real/imag floats) and bool (an int 0/1, not the
# bool).  Verifies the happy path, hasattr/dir visibility, the no-argument
# guard, and the unbound (type-qualified) form including the receiver guard.

# --- bound happy path ---
print((1, 2).__getnewargs__())
print(().__getnewargs__())
print("hello".__getnewargs__())
print("".__getnewargs__())
print(b"hi".__getnewargs__())
print(b"".__getnewargs__())
print((42).__getnewargs__())
print((10 ** 30).__getnewargs__())
print((3.14).__getnewargs__())
print((-0.0).__getnewargs__())
print(True.__getnewargs__())
print(False.__getnewargs__())
print((1 + 2j).__getnewargs__())
print((0j).__getnewargs__())

# --- hasattr / dir visibility (and bytearray must NOT have it) ---
for v in [(1, 2), "x", b"x", 5, 1.5, True, 1j]:
    print(type(v).__name__, hasattr(v, "__getnewargs__"),
          "__getnewargs__" in dir(v))
print("bytearray", hasattr(bytearray(b"x"), "__getnewargs__"))

# --- no-argument guard (CPython: "<type>.__getnewargs__() takes no
# arguments (1 given)"; bool reports as int) ---
for v, label in [((1, 2), "tuple"), ("x", "str"), (b"x", "bytes"),
                 (5, "int"), (1.5, "float"), (True, "bool"), (1j, "complex")]:
    try:
        v.__getnewargs__(99)
    except TypeError as e:
        print(label, "->", e)

# --- no-keyword-argument guard (bound form) ---
for v, label in [((1, 2), "tuple"), ("x", "str"), (b"x", "bytes"),
                 (5, "int"), (1.5, "float"), (True, "bool"), (1j, "complex")]:
    try:
        v.__getnewargs__(bogus=1)
    except TypeError as e:
        print(label, "kw->", e)

# --- unbound (type-qualified) form ---
print(int.__getnewargs__(7))
print(str.__getnewargs__("z"))
print(bytes.__getnewargs__(b"z"))
print(tuple.__getnewargs__((9,)))
print(float.__getnewargs__(2.5))
print(complex.__getnewargs__(4 + 5j))

# --- unbound receiver guard (method_descriptor "doesn't apply to" wording) ---
try:
    int.__getnewargs__("not an int")
except TypeError as e:
    print("recv-guard:", e)

# --- unbound missing-receiver guard ---
try:
    int.__getnewargs__()
except TypeError as e:
    print("no-recv:", e)
