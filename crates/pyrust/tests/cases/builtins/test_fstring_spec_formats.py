# Parity fixture for issues #2357 / #2372: constant f-string format specs are
# pre-parsed and cached per call site, so a spec like ".2f" in f"{x:.2f}" is
# parsed once and reused across a hot loop.  The cache is keyed by the spec
# string's backing pointer and validated each iteration, so it must produce
# byte-identical output to a fresh parse for every spec kind and every value.

# A broad sweep of presentation types, each rendered inside a small loop so the
# cached-spec fast path is exercised (first iter parses + caches, later iters
# hit the cache).
for _ in range(3):
    print(f"{3.14:.2f}")
    print(f"{3.14:e}")
    print(f"{42:08x}")
    print(f"{1000000:,}")
    print(f"{0.5:%}")
    print(f"{'hello':>10}")
    print(f"{42:+05d}")
    print(f"{255:#x}")
    print(f"{3.14159:10.3f}")
    print(f"{-0.0:+.1f}")
    print(f"{0:b}")
    print(f"{1234567.891:,.2f}")
    print(f"{'x':*^9}")
    print(f"{True:d}")
    print(f"{2 + 3j:.2f}")
    print(f"{1234567:_}")
    print(f"{3.14159265:.5g}")
    print(f"{-42:=+8d}")

# The same constant-spec call site applied to different value *types* across
# iterations: the parse is value-independent, so the cached parse must format
# each value correctly (or raise the same error a fresh parse would).
for v in [3.14, 42, "hi", True, 2 + 1j]:
    print(repr(f"{v:>8}"))

for v in [10, 2.5, "no", 7]:
    try:
        print(repr(f"{v:.2f}"))
    except (ValueError, TypeError) as e:
        print("ERR", type(e).__name__, e)

# A constant spec that always errors, inside a loop: every iteration must raise
# the identical ValueError (the cache must not swallow or mutate the error).
for _ in range(3):
    try:
        print(f"{'s':d}")
    except ValueError as e:
        print("VE", e)

# Dynamic (non-constant) spec — built from a runtime value.  This path never
# caches (its spec string is freshly allocated each iteration) but must still
# format correctly.
for w in range(4, 8):
    print(repr(f"{3.14:{w}.2f}"))

prec = 3
for v in [3.14159, 2.71828, 1.41421]:
    print(f"{v:.{prec}f}")

# Empty spec via f-string (routes through the spec opcode with an empty string).
for v in [42, 3.14, "x", True]:
    print(repr(f"{v:}"))

# User __format__ must still be dispatched (PyInstance values bypass the cache).
class C:
    def __format__(self, spec):
        return f"C<{spec}>"


c = C()
for _ in range(3):
    print(f"{c:.2f}")
    print(f"{c}")

# Built-in subclass without its own __format__ resolves to the backing type's
# __format__ (int.__format__ here).
class MyInt(int):
    pass


m = MyInt(255)
print(f"{m:x}")
print(f"{m:08b}")
