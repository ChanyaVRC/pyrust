# str(int) formats via a fixed 24-byte stack buffer (#alloc).  The widest output
# is i64::MIN ("-9223372036854775808", 20 chars), so verify the boundaries and
# that the result is byte-identical to CPython (and to the slow path for BigInt,
# which is one past the i64 range).

print(str(0))
print(str(1), str(-1))
print(str(9223372036854775807))  # i64::MAX
print(str(-9223372036854775808))  # i64::MIN (20 chars incl. sign)
print(str(-(2**63)))  # same as i64::MIN, reached via unary minus
print(str(2**63))  # i64::MAX + 1 → BigInt (slow path)
print(str(-(2**63) - 1))  # i64::MIN - 1 → BigInt
print(str(10**30))

# round-trips and use inside larger expressions
print(str(42) + "!", len(str(123456789)))
print([str(i) for i in range(-3, 4)])
print(str(True), str(False))  # bool is not int fast-path → "True"/"False"
