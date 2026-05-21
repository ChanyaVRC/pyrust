# pass_linear_loop_fold: constant-accumulation loops are folded at compile time.
#
# When a loop has the shape:
#   for _ in range(N): acc += K  (or -= K)
# and `acc` is initialised to a known integer before the loop, the pass
# replaces the entire loop with `acc = acc_init +/- K * N` at compile time,
# eliminating all N VM dispatches.

# Basic: for _ in range(N): acc += K
result = 0
for _ in range(1000000):
    result += 55
assert result == 55000000
print("linear-fold-add", result)

# Subtraction variant
total = 1000000
for _ in range(500000):
    total -= 2
assert total == 0
print("linear-fold-sub", total)

# Smaller count
s = 10
for _ in range(100):
    s += 3
assert s == 310
print("linear-fold-small", s)

# Loop variable _ retains its last-iteration value post-loop (999999 for range(1000000))
x = 0
for _ in range(5):
    x += 7
assert x == 35
assert _ == 4
print("linear-fold-iv-post", x, _)

# Zero iterations: loop body never runs
z = 42
for _ in range(0):
    z += 100
assert z == 42
print("linear-fold-zero-iters", z)

# Negative stop: ForCountConstInline with stop < 0 is also zero-trip
neg = 7
for _ in range(-3):
    neg += 100
assert neg == 7
print("linear-fold-neg-stop", neg)
