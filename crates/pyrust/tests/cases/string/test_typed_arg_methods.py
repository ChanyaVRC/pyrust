# Parity fixture for string methods with typed argument extraction.
# Covers center, ljust, rjust, zfill, expandtabs, removeprefix, removesuffix,
# partition, and rpartition — all migrated to explicit typed signatures in
# pyrust-builtins with CPython-3.12-compatible TypeError messages.

# ── center ────────────────────────────────────────────────────────────────────
print(repr("x".center(5)))           # '  x  '
print(repr("x".center(5, "*")))      # '**x**'
print(repr("hello".center(3)))       # 'hello'  (no padding needed)
print(repr("".center(3)))            # '   '
print(repr("ab".center(5)))          # ' ab  ' (asymmetric: right > left)
print(repr("a".center(4, "-")))      # '-a--'

# ── ljust / rjust ─────────────────────────────────────────────────────────────
print(repr("x".ljust(5)))            # 'x    '
print(repr("x".ljust(5, "-")))       # 'x----'
print(repr("x".rjust(5)))            # '    x'
print(repr("x".rjust(5, "-")))       # '----x'
print(repr("hello".ljust(3)))        # 'hello'  (no padding needed)

# ── zfill ─────────────────────────────────────────────────────────────────────
print(repr("42".zfill(5)))           # '00042'
print(repr("-42".zfill(5)))          # '-0042'
print(repr("+42".zfill(5)))          # '+0042'
print(repr("42".zfill(2)))           # '42'  (no padding)
print(repr("".zfill(3)))             # '000'

# ── expandtabs ────────────────────────────────────────────────────────────────
print(repr("x\ty".expandtabs()))     # 'x       y'  (default tabsize=8)
print(repr("x\ty".expandtabs(4)))    # 'x   y'
print(repr("x\ty".expandtabs(0)))    # 'xy'  (tab removed)
print(repr("x\ty".expandtabs(True))) # 'xy'  (True==1, treated as int)
print(repr("x\ty".expandtabs(-1)))   # 'xy'  (negative treated as 0)
print(repr("\tx\ty".expandtabs(4)))  # '    x   y'

# ── removeprefix / removesuffix ───────────────────────────────────────────────
print(repr("hello world".removeprefix("hello ")))   # 'world'
print(repr("hello world".removeprefix("bye")))      # 'hello world'
print(repr("hello world".removesuffix(" world")))   # 'hello'
print(repr("hello world".removesuffix("bye")))      # 'hello world'
print(repr("aaa".removeprefix("a")))                # 'aa'  (only first occurrence)

# ── partition / rpartition ────────────────────────────────────────────────────
print(repr("hello world".partition(" ")))           # ('hello', ' ', 'world')
print(repr("hello world".partition("xyz")))         # ('hello world', '', '')
print(repr("one two three".partition(" ")))         # ('one', ' ', 'two three')
print(repr("hello world".rpartition(" ")))          # ('hello', ' ', 'world')
print(repr("hello world".rpartition("xyz")))        # ('', '', 'hello world')
print(repr("one two three".rpartition(" ")))        # ('one two', ' ', 'three')

# ── TypeError: integer arguments ──────────────────────────────────────────────
def check_type_error(desc, fn):
    try:
        fn()
        print(f"ERROR: {desc} should have raised TypeError")
    except TypeError as e:
        print(f"TypeError: {e}")

check_type_error("center(str)", lambda: "x".center("a"))
check_type_error("center(float)", lambda: "x".center(1.5))
check_type_error("center(None)", lambda: "x".center(None))
check_type_error("center(list)", lambda: "x".center([]))
check_type_error("ljust(str)", lambda: "x".ljust("a"))
check_type_error("rjust(float)", lambda: "x".rjust(1.5))
check_type_error("zfill(str)", lambda: "x".zfill("a"))
check_type_error("expandtabs(str)", lambda: "x".expandtabs("bad"))
check_type_error("expandtabs(None)", lambda: "\t".expandtabs(None))

# ── TypeError: fill character ─────────────────────────────────────────────────
check_type_error("center(5, int)", lambda: "x".center(5, 1))
check_type_error("center(5, 'ab')", lambda: "x".center(5, "ab"))
check_type_error("ljust(5, int)", lambda: "x".ljust(5, 1))
check_type_error("rjust(5, 'ab')", lambda: "x".rjust(5, "ab"))

# ── TypeError: str-typed arguments ───────────────────────────────────────────
check_type_error("removeprefix(int)", lambda: "x".removeprefix(1))
# Note: removeprefix/removesuffix(None) reports "not None" in some CPython builds
# and "not NoneType" in others (tp_name vs type.__name__ discrepancy).  Omitted.
check_type_error("removesuffix(float)", lambda: "x".removesuffix(1.5))
check_type_error("partition(int)", lambda: "x".partition(1))
check_type_error("rpartition(list)", lambda: "x".rpartition([]))

# ── TypeError: arity ─────────────────────────────────────────────────────────
check_type_error("center() no args", lambda: "x".center())
check_type_error("center() too many", lambda: "x".center(5, " ", "x"))
check_type_error("ljust() no args", lambda: "x".ljust())
check_type_error("ljust() too many", lambda: "x".ljust(5, " ", "z"))
check_type_error("rjust() no args", lambda: "x".rjust())
check_type_error("rjust() too many", lambda: "x".rjust(5, " ", "z"))
check_type_error("zfill() no args", lambda: "x".zfill())
check_type_error("zfill() too many", lambda: "x".zfill(5, 6))
check_type_error("expandtabs() too many", lambda: "x".expandtabs(8, 4))
check_type_error("removeprefix() no args", lambda: "x".removeprefix())
check_type_error("removeprefix() too many", lambda: "x".removeprefix("a", "b"))
check_type_error("removesuffix() no args", lambda: "x".removesuffix())
check_type_error("removesuffix() too many", lambda: "x".removesuffix("a", "b"))
check_type_error("partition() no args", lambda: "x".partition())
check_type_error("rpartition() no args", lambda: "x".rpartition())
