# float() parity fixture — covers string → ValueError and special-value strings.
# Issue #1058: float("abc") raised RuntimeError instead of ValueError.

# Invalid strings must raise ValueError with the correct message.
for bad in ["abc", "", "1.2.3", "  bad  "]:
    try:
        float(bad)
        print(f"FAIL: float({bad!r}) did not raise")
    except ValueError as e:
        print(f"ValueError: {e}")
    except Exception as e:
        print(f"FAIL {type(e).__name__}: {e}")

# except ValueError must catch it.
try:
    float("xyz")
except ValueError:
    print("except ValueError: caught")

# Special-value strings accepted by CPython must work.
print(float("nan"))
print(float("inf"))
print(float("-inf"))
print(float("NaN"))
print(float("Infinity"))
print(float("-Infinity"))
print(float("+inf"))
print(float("+nan"))
print(float("  inf  "))

# Numeric types — no regression.
print(float(0))
print(float(1))
print(float(-1))
print(float(1.5))
print(float(True))
print(float(False))

# No argument → 0.0
print(float())
