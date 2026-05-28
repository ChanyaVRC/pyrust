# Parity fixture for str.center/ljust/rjust/zfill with very large width arguments.
# CPython 3.12 raises MemoryError when the requested fill exceeds available memory,
# and OverflowError when the width overflows C ssize_t (i.e. is a BigInt).

# --- MemoryError for widths that require huge allocations (fit in i64) ---
try:
    "x".center(2**60)
except MemoryError:
    print("center 2**60: MemoryError")

try:
    "x".ljust(2**60)
except MemoryError:
    print("ljust 2**60: MemoryError")

try:
    "x".rjust(2**60)
except MemoryError:
    print("rjust 2**60: MemoryError")

try:
    "x".zfill(2**60)
except MemoryError:
    print("zfill 2**60: MemoryError")

# --- OverflowError for BigInt widths that overflow C ssize_t ---
try:
    "x".center(2**200)
except OverflowError:
    print("center 2**200: OverflowError")

try:
    "x".ljust(2**200)
except OverflowError:
    print("ljust 2**200: OverflowError")

try:
    "x".rjust(2**200)
except OverflowError:
    print("rjust 2**200: OverflowError")

try:
    "x".zfill(2**200)
except OverflowError:
    print("zfill 2**200: OverflowError")

# --- Normal operation is unaffected ---
print(repr("x".center(9)))
print(repr("x".center(10)))
print(repr("x".ljust(5)))
print(repr("x".rjust(5)))
print(repr("x".zfill(5)))
print(repr("+x".zfill(5)))
print(repr("-x".zfill(5)))

# --- width <= len(s): returns s unchanged ---
print(repr("hello".center(3)))
print(repr("hello".ljust(3)))
print(repr("hello".rjust(3)))
print(repr("hello".zfill(3)))

# --- custom fill char ---
print(repr("x".center(9, "*")))
print(repr("x".ljust(5, "-")))
print(repr("x".rjust(5, ".")))

# --- negative width treated as zero ---
print(repr("x".center(-5)))
print(repr("x".ljust(-5)))
print(repr("x".rjust(-5)))
print(repr("x".zfill(-5)))
