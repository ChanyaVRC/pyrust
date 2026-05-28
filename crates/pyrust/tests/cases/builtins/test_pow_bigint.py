# Parity fixture for issue #1028: pow(base, exp, mod) with BigInt arguments.
#
# Tests that 3-argument pow works correctly when any argument is a BigInt
# (i.e., a value outside the i64 range, produced by large arithmetic).

# Core acceptance criteria from the issue.
print(pow(2**100, 2**50, 10**9 + 7))   # 304943220
print(pow(10**20, 10**10, 10**9 + 7))  # 89024422
print(pow(2**64, 3, 10))               # 6

# Mixed arms: BigInt base with i64 exp and mod.
print(pow(2**100, 3, 10))              # 6

# Mixed arms: i64 base with BigInt exp.
print(pow(3, 2**100, 10))              # 1

# Mixed arms: i64 base and exp with BigInt modulus.
print(pow(3, 3, 2**64))               # 27 (27 < 2**64)

# Negative BigInt modulus is allowed (result is in (modulus, 0]).
print(pow(2**64, 3, -(10**9+7)))

# bool is a subtype of int; Bool as any argument must work.
print(pow(True, 2**100, 3))            # 1
print(pow(2**100, True, 7))            # 2
print(pow(2**100, 2, True))            # 0 (mod 1)

# ValueError: modulus of zero.
try:
    pow(2**100, 3, 0)
except ValueError as e:
    print(e)

# TypeError: float argument is rejected.
try:
    pow(2**100, 3, 1.0)
except TypeError as e:
    print(e)
