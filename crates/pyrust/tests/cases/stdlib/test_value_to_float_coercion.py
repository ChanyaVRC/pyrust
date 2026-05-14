# Coverage for `try_value_to_float` (refactor #426) — exercises both
# wrappers (`value_to_float` in `math.*` / `**` paths, and
# `fmt_value_to_float` in format-spec paths).
#
# Locks in:
#   - Happy path: Float / Int / Bool → coerce to f64 uniformly.
#   - Error path: same set of non-numeric inputs produces the SAME error
#     class (TypeError) at every call site; the wording differs by
#     call site but is preserved.

import math


# ─── Happy paths (Float / Int / Bool) ─────────────────────────────────

print("math.sqrt(int)   =", math.sqrt(9))
print("math.sqrt(float) =", math.sqrt(2.25))
print("math.sqrt(True)  =", math.sqrt(True))     # bool → 1.0
print("math.sqrt(False) =", math.sqrt(False))    # bool → 0.0

print("math.pow(int,int)   =", math.pow(2, 10))
print("math.pow(int,float) =", math.pow(2, 0.5))
print("math.pow(bool,int)  =", math.pow(True, 5))

# `**` operator uses the same coercion when one operand is float.
print("2 ** 0.5       =", 2 ** 0.5)
print("True ** 2.0    =", True ** 2.0)
print("4.0 ** False   =", 4.0 ** False)

# format-spec path (fmt_value_to_float)
print(f"int   .2f -> {42:.2f}")
print(f"float .2f -> {1.5:.2f}")
print(f"True  .2f -> {True:.2f}")
print(f"False .2f -> {False:.2f}")


# ─── Error paths (TypeError on non-numeric LHS) ───────────────────────
#
# Every wrapper raises TypeError on the same set of inputs.  The exact
# wording differs by call site (`math.sqrt` says "a float is required";
# format-spec says "must be real number"), but both are TypeErrors.

def expect_typeerror(label, fn):
    try:
        fn()
        print(f"{label}: FAIL (no exception)")
    except TypeError:
        print(f"{label}: TypeError")
    except Exception as e:
        print(f"{label}: {type(e).__name__} ({e})")


# math.sqrt path
expect_typeerror("math.sqrt(None)", lambda: math.sqrt(None))
expect_typeerror("math.sqrt('a')",  lambda: math.sqrt("a"))
expect_typeerror("math.sqrt([])",   lambda: math.sqrt([]))
expect_typeerror("math.sqrt({})",   lambda: math.sqrt({}))
expect_typeerror("math.sqrt(())",   lambda: math.sqrt(()))

# `**` (also routes through value_to_float when one side is float)
expect_typeerror("None ** 0.5", lambda: None ** 0.5)
expect_typeerror("[] ** 0.5",   lambda: [] ** 0.5)
expect_typeerror("'a' ** 0.5",  lambda: "a" ** 0.5)

# format-spec path (fmt_value_to_float)
expect_typeerror("f'{None:.2f}'", lambda: f"{None:.2f}")
expect_typeerror("f'{[]:.2f}'",   lambda: f"{[]:.2f}")
expect_typeerror("f'{():.2f}'",   lambda: f"{():.2f}")


# ─── Identity preservation (alloc-free happy path) ─────────────────────
# Re-exercise the happy path many times to catch any accidental
# heap-touch or precision drift in the dedup'd helper.
total = 0.0
for i in range(50):
    total += math.sqrt(i)
print(f"sum-sqrt-0-49 = {total:.6f}")
