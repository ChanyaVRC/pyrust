# Issue #485: BigInt x int cross-type arms for //, %, divmod, bitwise, shifts.
#
# Once #421 promotes int overflow to BigInt, ops mixing BigInt with int / bool
# must compute symmetrically (matching CPython).  Pure-int cases stay on the
# fast path; this fixture pins the BigInt cross arms so a regression that
# brings back `TypeError: expected number` / `bitwise op requires integer`
# is caught by the parity comparator.

big = 2 ** 64
big2 = 2 ** 100
nbig = -(2 ** 64)

# ---- floor division ----
print(big // 2)
print(2 // big)
print(big // big2)
print(big2 // big)
print(big // -7)            # negative divisor: floor toward -inf
print(nbig // 7)
print(-100 // big)
print(big // True)          # bool coerces to int
print(big // big)

# ---- modulo ----
print(big % 7)
print(big % -7)             # remainder sign matches divisor
print(nbig % 7)
print(-100 % big)
print(big2 % big)
print(big % big)
print(big % True)

# ---- divmod ----
print(divmod(big, 100))
print(divmod(big, -7))
print(divmod(nbig, 7))
print(divmod(big2, big))
print(divmod(big, big2))
print(divmod(big, True))

# ---- bitwise AND ----
print(big & 0xFF)
print(0xFF & big)
print(big & big2)
print(big & True)
print(big & False)
print(True & big)

# ---- bitwise OR ----
print(big | 1)
print(1 | big)
print(big | big2)
print(big | True)
print(True | big)

# ---- bitwise XOR ----
print(big ^ 1)
print(1 ^ big)
print(big ^ big2)
print(big ^ big)            # x ^ x == 0
print(big ^ False)

# ---- left shift (BigInt LHS) ----
print(big << 1)
print(big << 0)
print(big << True)

# ---- right shift (BigInt LHS) ----
print(big >> 1)
print(big >> 64)
print(big >> 0)
print(big >> True)

# ---- ZeroDivisionError parity ----
try:
    big // 0
except ZeroDivisionError:
    print("big // 0 ZeroDivisionError")
try:
    big % 0
except ZeroDivisionError:
    print("big % 0 ZeroDivisionError")
try:
    divmod(big, 0)
except ZeroDivisionError:
    print("divmod(big, 0) ZeroDivisionError")

# ---- ValueError on negative shift ----
try:
    big << -1
except ValueError:
    print("big << -1 ValueError")
try:
    big >> -1
except ValueError:
    print("big >> -1 ValueError")

# ---- Pow already works post-#484 review; pin it here too ----
print(big ** 2)
print(big ** 0)
print(big ** 1)
