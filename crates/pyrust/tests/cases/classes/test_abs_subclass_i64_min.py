class MI(int):
    pass


# abs() on an int subclass backed by i64::MIN must promote to BigInt,
# matching CPython, rather than wrapping to the negative value.
print(abs(MI(-2**63)))  # 9223372036854775808

# Plain int at i64::MIN stays correct.
print(abs(-2**63))  # 9223372036854775808

# Subclass already backed by a BigInt.
print(abs(MI(2**63)))  # 9223372036854775808

# Ordinary subclass values are unaffected.
print(abs(MI(-5)))  # 5
print(abs(MI(0)))  # 0
