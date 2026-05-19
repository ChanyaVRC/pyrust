# Sequence repetition with BigInt repeat count raises OverflowError (#740)
# and with huge-but-fitting int raises MemoryError instead of aborting (#741).

try:
    _ = "x" * (10**20)
except OverflowError:
    print("OverflowError str*BigInt")

try:
    _ = [1] * (10**20)
except OverflowError:
    print("OverflowError list*BigInt")

try:
    _ = b"x" * (10**20)
except OverflowError:
    print("OverflowError bytes*BigInt")

# Edge cases: zero and negative repeat count produce empty sequences.
print(repr("x" * 0))
print(repr("x" * -1))
