# i64-backed bounds can describe a range whose logical length needs 65 bits.
# All operations other than len()/__len__() must continue to use that exact
# length; len itself raises because CPython's Py_ssize_t is signed 64-bit.

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
wide = range(I64_MIN, I64_MAX)

for label, operation in (("len", lambda: len(wide)), ("dunder-len", wide.__len__)):
    try:
        operation()
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))

print("truth", bool(wide))
print("first-last", wide[0], wide[-1])
print("wide-positive-index", wide[2**63])
print("wide-negative-index", wide[-(2**64 - 1)])
print("wide-index-result", wide.index(I64_MAX - 1))
print("wide-count", wide.count(I64_MIN), wide.count(I64_MAX))
print("wide-equality", wide == range(I64_MIN, I64_MAX), wide == range(I64_MIN, I64_MAX - 1))
print("wide-hash", hash(wide))
print("wide-slices", wide[:3], wide[-3:], wide[::-2][:3])

try:
    _ = wide[2**64 - 1]
except Exception as exc:
    print("wide-index-error", type(exc).__name__, str(exc))

# Negating i64::MIN must happen after widening.
min_step = range(I64_MAX, I64_MIN, I64_MIN)
print("min-step", len(min_step), list(min_step), min_step.index(-1), hash(min_step))

# Materialisers must stop after the final value instead of wrapping the
# one-past-end cursor across the opposite i64 boundary.
print("positive-materialise", list(range(I64_MAX - 1, I64_MAX, 2)))
print("negative-materialise", list(range(I64_MIN + 1, I64_MIN, -2)))
print("set-materialise", sorted(set(range(I64_MAX - 1, I64_MAX, 2))))
