# Parity tests for loop-invariant code motion (LICM).
# These verify that hoisted instructions produce the same results as CPython —
# i.e., the optimisation is correct, not merely that hoisting happened.

# --- LoadConst inside loop body is invariant and gets hoisted ---
result = []
for i in range(5):
    k = 100
    result.append(i + k)
print(result)  # [100, 101, 102, 103, 104]

# --- BinOpConst with loop-invariant source is hoisted ---
# offset is set before the loop and never modified inside it.
offset = 7
result2 = []
for i in range(4):
    x = offset + 3  # BinOpConst(x_reg, offset_reg, Add, 3) — invariant
    result2.append(i + x)
print(result2)  # [10, 11, 12, 13]

# --- UnaryOp with loop-invariant source is hoisted ---
base = 5
result3 = []
for i in range(4):
    neg = -base  # UnaryOp(neg_reg, Neg, base_reg) — base not written in loop
    result3.append(neg + i)
print(result3)  # [-5, -4, -3, -2]

# --- Nested loops: invariant from inner loop hoisted correctly ---
result4 = []
for i in range(3):
    for j in range(3):
        c = 10  # LoadConst — invariant wrt inner loop, hoisted before inner header
        result4.append(i * 10 + j + c)
print(result4)  # [10, 11, 12, 20, 21, 22, 30, 31, 32]

# --- Loop with try/except: LICM skips exception regions entirely ---
# k = 42 stays inside the loop body; the program must still be correct.
result5 = []
for i in range(3):
    k = 42
    try:
        if i == 1:
            raise ValueError
        result5.append(i + k)
    except ValueError:
        result5.append(-1)
print(result5)  # [42, -1, 44]

# --- Conditional in body: instructions after the branch are not hoisted ---
# Only instructions in the straight-line prefix (before any conditional) may
# be hoisted.  c = 20 is after the if-branch so it stays in-body.
result6 = []
for i in range(5):
    k = 1000  # LoadConst in safe prefix — hoisted
    if i % 2 == 0:
        c = 20
        result6.append(i + c + k)
print(result6)  # [1020, 1022, 1024]

# --- Invariant that uses a value modified in the loop must NOT be hoisted ---
# total is accumulated each iteration; `total + 5` reads a variant register.
total = 0
log = []
for i in range(4):
    total += i
    v = total + 5  # BinOpConst reads total, which IS written → stays in body
    log.append(v)
print(log)  # [5, 6, 8, 11]

# --- empty loop: no body ever runs, hoisted init must not crash ---
result7 = []
for i in range(0):
    k = 99
    result7.append(k)
print(result7)  # []

print("OK")
