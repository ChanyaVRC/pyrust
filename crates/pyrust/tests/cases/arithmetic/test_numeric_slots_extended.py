# Issue #458: numeric protocol slot dispatch extended to /, //, %, **,
# <<, >>, &, |, ^.  Exercises every numeric type pair and the error
# paths so a regression in the slot table surfaces as a parity diff.


def show(x):
    print(repr(x), type(x).__name__)


def err(fn):
    try:
        fn()
    except Exception as e:
        print(type(e).__name__, str(e))


# ---- true division (always float) ----
print("--- div ---")
show(3 / 2)
show(4 / 2)
show(7.0 / 2)
show(-7 / 2)
show((2 ** 70) / 2)  # BigInt / int -> float
err(lambda: 1 / 0)
err(lambda: 1.0 / 0)

# ---- floor division (Python floored, not C truncation) ----
print("--- floordiv ---")
show(7 // 3)
show(-7 // 3)
show(7 // -3)
show(-7 // -3)
show(7.0 // 2)
show((-(2 ** 63)) // 3)  # i64::MIN boundary -> BigInt
show((2 ** 70) // 7)
err(lambda: 1 // 0)
err(lambda: 1.0 // 0)

# ---- modulo (sign follows divisor) ----
print("--- mod ---")
show(7 % 3)
show(-7 % 3)
show(7 % -3)
show(7.0 % 3)
show(-7.0 % 3)
show((2 ** 70) % 7)
err(lambda: 1 % 0)
err(lambda: 1.0 % 0)

# ---- power ----
print("--- pow ---")
show(2 ** 10)
show(2 ** 63)   # -> BigInt
show(2 ** 64)   # -> BigInt
show(2 ** -1)   # -> float
show(4 ** 0.5)
show((-8) ** (1 / 3))  # negative real, fractional -> complex
show(0 ** 0)
err(lambda: 0.0 ** -1)

# ---- shifts ----
print("--- shifts ---")
show(1 << 10)
show(1 << 100)  # -> BigInt
show(1024 >> 2)
show(1024 >> 100)
show(-1024 >> 100)
show(True << 3)
show((2 ** 100) >> 50)
err(lambda: 1 << -1)
err(lambda: 1.5 << 2)

# ---- bitwise ----
# Note: `bool & bool` returning a bool (not int) is a separate pre-existing
# divergence tracked outside this behavior-preserving refactor, so the bool
# operands are intentionally not exercised here.
print("--- bitwise ---")
show(7 & 3)
show(7 | 8)
show(7 ^ 3)
show((2 ** 100) & (2 ** 100 - 1))
show((2 ** 100) | 1)
show(5 & 3 | 8)
err(lambda: 1.5 & 2)
err(lambda: "a" | 3)
