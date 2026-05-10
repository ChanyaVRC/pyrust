# range() edge cases
# Guards the wrapping_add overflow path in iter_value (pyrust-core/src/lib.rs)

# --- step=0 must raise ValueError ---

try:
    list(range(1, 10, 0))
    print("range-step-zero", "no-error")
except ValueError:
    print("range-step-zero", "ValueError")

# --- negative step: empty when start <= stop ---

print("range-neg-empty", list(range(0, 10, -1)))   # []
print("range-neg-empty2", list(range(5, 5, -1)))   # []
print("range-neg-empty3", list(range(3, 5, -1)))   # []

# --- negative step: correct values ---

print("range-neg-basic", list(range(5, 0, -1)))    # [5, 4, 3, 2, 1]
print("range-neg-step2", list(range(10, 0, -3)))   # [10, 7, 4, 1]
print("range-neg-to-neg", list(range(-1, -5, -1))) # [-1, -2, -3, -4]

# --- positive step: empty when start >= stop ---

print("range-pos-empty", list(range(5, 0, 1)))     # []
print("range-pos-empty2", list(range(5, 5, 1)))    # []

# range() with bool arguments — Issue #101
print("range-true", list(range(True)))
print("range-false-start", list(range(False, 3)))
print("range-bool-step", list(range(0, 4, True)))
