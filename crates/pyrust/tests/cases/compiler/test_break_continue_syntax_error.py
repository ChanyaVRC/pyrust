# Parity fixture for issue #754: break/continue outside a loop must be a
# SyntaxError detected at compile time, matching CPython 3.12 behaviour.
#
# Because these are compile-time errors, a file that contains bare `break` or
# `continue` at the top level would itself fail to compile under both
# interpreters, producing no stdout and making the parity harness mark it as
# an error.  exec() could work but is not yet implemented in pyrust.
#
# Instead this fixture exercises only the *valid* cases — break and continue
# inside loops — and verifies they still work correctly after the fix.  The
# SyntaxError path for invalid usage is confirmed via `cargo test --bin pyrust`
# unit tests and manual reproduction.

# break inside a for loop exits the loop
for i in range(5):
    if i == 2:
        break
print("break exits for loop:", i)

# continue inside a for loop skips the remainder of the iteration
acc = []
for i in range(5):
    if i % 2 == 0:
        continue
    acc.append(i)
print("continue skips even:", acc)

# break inside a while loop
n = 0
while True:
    if n == 3:
        break
    n += 1
print("break exits while loop:", n)

# continue inside a while loop
n = 0
evens = []
while n < 6:
    n += 1
    if n % 2 != 0:
        continue
    evens.append(n)
print("continue skips odd:", evens)

# nested loops: break only exits the innermost loop
outer_count = 0
for i in range(3):
    for j in range(3):
        if j == 1:
            break
    outer_count += 1
print("nested break outer count:", outer_count)

# for-else: else fires when no break occurred
result = "no-break"
for i in range(3):
    pass
else:
    result = "else-ran"
print("for-else without break:", result)

# for-else: else does not fire when break occurred
result = "no-break"
for i in range(3):
    if i == 1:
        break
else:
    result = "else-ran"
print("for-else with break:", result)

print("done")
