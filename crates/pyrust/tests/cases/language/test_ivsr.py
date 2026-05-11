# Parity tests for induction variable strength reduction (IVSR).
# Each loop multiplies the loop counter by a constant K; IVSR replaces the
# multiplication with a running accumulator.  The output must match CPython
# exactly to confirm the accumulator is initialised and incremented correctly.

# --- basic: range(5), K=3 ---
result = []
for i in range(5):
    result.append(i * 3)
print(result)  # [0, 3, 6, 9, 12]

# --- range starts at non-zero: range(2, 6), K=4 ---
result2 = []
for i in range(2, 6):
    result2.append(i * 4)
print(result2)  # [8, 12, 16, 20]

# --- first iteration only (range(1)): accumulator must equal start*K ---
result3 = []
for i in range(1):
    result3.append(i * 7)
print(result3)  # [0]

# --- empty range: no body runs, accumulator never read ---
result4 = []
for i in range(0):
    result4.append(i * 5)
print(result4)  # []

# --- K used in an expression, not just appended ---
total = 0
for i in range(4):
    total += i * 2
print(total)  # 0+2+4+6 = 12

print("OK")
