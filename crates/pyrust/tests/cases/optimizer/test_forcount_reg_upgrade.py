# pass_forcount_reg_upgrade: when the stop register in a ForCountReg is loaded
# from a single LoadConst, upgrade it to ForCountConst/ForCountConstInline so
# the per-iteration register read and const-pool lookup are eliminated.
#
# The canonical pattern is `while var < n: ... var += step` where `n` is set
# once from a literal — the compiler emits ForCountReg because `n` is a named
# variable at the call site; after copy-prop and this upgrade pass the loop
# runs as ForCountConstInline with stop/step inlined.

# Basic: while-range loop with named stop variable
n = 100
s = 0
i = 0
while i < n:
    s += i
    i += 1
assert s == 4950
print("while-range-named-stop", s)

# Descending loop
n = 10
total = 0
j = n - 1
while j >= 0:
    total += j
    j -= 1
assert total == 45
print("while-range-desc", total)

# Stop variable used after loop (value must be preserved)
limit = 5
k = 0
while k < limit:
    k += 1
assert k == 5
assert limit == 5
print("while-range-stop-preserved", k, limit)

# Non-trivial step
step_n = 20
acc = 0
x = 0
while x < step_n:
    acc += x
    x += 2
assert acc == 90  # 0+2+4+6+8+10+12+14+16+18
print("while-range-step2", acc)
