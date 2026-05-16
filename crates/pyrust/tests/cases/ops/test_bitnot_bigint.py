# Parity fixture for ~ (BitNot) on BigInt operands — issue #495
# CPython 3.12 reference: ~x == -(x + 1)

# Small int cases — must be unchanged
print(~0)
print(~1)
print(~(-1))

# Bool cases — bool subclasses int, ~True == -2, ~False == -1
print(~True)
print(~False)

# BigInt cases (values that overflow i64)
big = 2 ** 64
print(~big)                    # -(2**64) - 1
print(~(-big))                 # 2**64 - 1

# Larger BigInt
very_big = 2 ** 128
print(~very_big)               # -(2**128) - 1
print(~(-very_big))            # 2**128 - 1

# Verify identity ~x == -(x + 1) for a few values
for x in [2**64, -(2**64), 2**100, -(2**100), 0, 1, -1]:
    assert ~x == -(x + 1), f"~{x} != {-(x+1)}: got {~x}"
print("identity-ok")

# TypeError on non-integer types
try:
    ~1.5
except TypeError:
    print("float-type-error")

try:
    ~"x"
except TypeError:
    print("str-type-error")
