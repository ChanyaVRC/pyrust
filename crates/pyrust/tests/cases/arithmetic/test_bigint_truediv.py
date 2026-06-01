# int / int true division (issue #1923): the quotient is computed with
# exact integer arithmetic and correctly rounded (round-half-to-even),
# mirroring CPython's long_true_divide.  Operands beyond f64 range must NOT
# overflow when the quotient itself is representable; only a too-large
# *result* raises OverflowError, and a zero divisor raises ZeroDivisionError.


def show(s):
    try:
        print(repr(eval(s)))
    except Exception as e:
        print(type(e).__name__ + ": " + str(e))


# Large operands, representable quotient (formerly raised OverflowError).
show("(10**400) / (10**390)")
show("(10**500) / (10**400)")
show("(7*10**400) / (2*10**400)")  # exactly 3.5
show("(10**400) / (10**400)")  # exactly 1.0
show("(-10**400) / (10**390)")  # negative result
show("(10**400) / (-10**390)")
show("(-10**400) / (-10**390)")
show("(2**1000) / (2**990)")  # 1024.0 (within f64 range)

# Quotient genuinely too large for f64 -> OverflowError (correct message).
show("(10**500) / (10**100)")
show("(2**2000) / (2)")
show("(2**1024) / 1")

# Quotient underflows to (signed) zero.
show("1 / (10**400)")
show("(-1) / (10**400)")
show("1 / (2**1075)")  # below half the smallest subnormal -> 0.0
show("3 / (2**1075)")

# Subnormal results survive (would be lost by naive m * 2**exp).
show("1 / (2**1074)")  # smallest positive subnormal: 5e-324
show("(2**52) / (2**1110)")

# Zero numerator keeps the sign of the (nonzero) divisor.
show("0 / (10**400)")
show("0 / (-10**400)")

# Zero divisor -> ZeroDivisionError (even for huge numerators).
show("(10**400) / 0")
show("5 / 0")
show("0 / 0")

# Small int / int correctly rounded (f64 fast path).
show("1 / 3")
show("2 / 3")
show("7 / 2")
show("10 / 5")
show("(-7) / 2")
show("7 / (-2)")

# i64-boundary operands that exceed exact f64 precision (2**53) take the
# exact integer path, not the lossy f64 fast path.
show("9007199254740993 / 1")  # 2**53 + 1
show("9223372036854775807 / 1")  # i64::MAX
show("(-9223372036854775808) / 1")  # i64::MIN
show("9223372036854775807 / 3")
show("(2**53 + 1) / (2**53)")

# Mixed int/float still uses float division semantics and wording.
show("1.0 / 0")
show("(10**400) / 2.0")  # BigInt operand with a float -> OverflowError
show("5 / 0.0")
