# Parity fixture for pass_loop_inversion (issue #870).
#
# Loop inversion replaces the unconditional back-edge Jump in while-condition
# loops with a CmpJumpIfTrue* at the loop tail, eliminating one VM dispatch per
# iteration on the hot path.
#
# These tests verify that the transformed bytecode produces output identical to
# CPython 3.12 across a range of loop patterns: basic iteration, zero-trip,
# break, continue, and nested loops.

# Basic while loop
i = 0
s = 0
while i < 1000000:
    s += i
    i += 1
print(s)  # 499999500000

# Zero-trip loop (initial guard exits immediately; no back-edge reached)
x = 10
while x < 5:
    x += 1
print(x)  # 10

# Loop with break (break jumps forward past the loop exit — must be unaffected)
i = 0
while i < 100:
    if i == 50:
        break
    i += 1
print(i)  # 50

# Loop with continue (continue targets the header guard, which re-checks condition)
i = 0
s = 0
while i < 100:
    i += 1
    if i % 2 == 0:
        continue
    s += i
print(s)  # 2500  (sum of odd numbers 1, 3, 5, ..., 99)

# Nested while loops (each loop is independently inverted)
i = 0
s = 0
while i < 100:
    j = 0
    while j < 100:
        s += 1
        j += 1
    i += 1
print(s)  # 10000

# Loop with register-register condition (CmpJumpIfFalse variant)
def count_down(start, stop):
    i = start
    while i > stop:
        i -= 1
    return i

print(count_down(10, 0))   # 0
print(count_down(5, 3))    # 3
print(count_down(0, 0))    # 0  (zero-trip)

# while True: if cond: break; body — CmpJumpIfTrueConst header shape.
# The back-edge Jump should be inverted to CmpJumpIfFalseConst, eliminating
# one unconditional jump per iteration on the hot path.
n = 1000000
acc = 0
while True:
    if n == 0:
        break
    acc += n
    n -= 1
print(acc)  # 500000500000

# Zero-trip: break fires on first test (body never executes)
n = 0
acc = 0
while True:
    if n == 0:
        break
    acc += n
    n -= 1
print(acc)  # 0

# Nested: outer is a standard while, inner uses while True + break
s = 0
i = 0
while i < 100:
    j = 0
    while True:
        if j >= 100:
            break
        s += 1
        j += 1
    i += 1
print(s)  # 10000
