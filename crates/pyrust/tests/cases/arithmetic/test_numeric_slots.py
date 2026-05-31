# NumericOps slot dispatch (issue #458): one canonical implementation per
# numeric op (Add / Sub / Mul) covers every type pair.  Exercises the
# numeric slots across Int / Float / BigInt / Bool, i64-overflow promotion
# to BigInt, the BigInt-with-Float OverflowError path, complex coercion,
# and the non-numeric fallthrough (sequence concat / repetition).


def show(s):
    try:
        print(repr(eval(s)))
    except Exception as e:
        print(type(e).__name__ + ": " + str(e))


# Int x Int, including i64-overflow promotion to bigint.
show("1 + 2")
show("10 - 3")
show("3 * 4")
show("-5 + 2")
show("0 + 0")
show("9223372036854775807 + 1")
show("-9223372036854775808 - 1")
show("9223372036854775807 * 2")

# Float mixed with Int / Float (Int->Float promotion both directions).
show("1.5 + 2")
show("2 + 1.5")
show("2.0 - 5")
show("2.5 * 4")
show("1.5 + 2.5")
show("1.5 - 2.5")
show("2.0 * 2.0")

# Bool behaves as Int (subtype) in arithmetic.
show("True + 1")
show("True + True")
show("False * 5")
show("True - 2")
show("False + 0.5")

# BigInt cross-type arms (Int promotion, BigInt+BigInt, BigInt*Int).
show("2**70 + 1")
show("1 + 2**70")
show("2**70 - 2**70")
show("2**70 * 2")
show("(2**63) * (2**63)")
show("2**100 - 2**99")

# BigInt with Float: small bigint coerces to float.
show("2**70 + 1.5")
show("1.5 + 2**70")
show("2**70 * 2.0")

# BigInt too large for float -> OverflowError (int too large to convert).
show("(10**400) + 1.0")
show("1.0 - (10**400)")
show("(10**400) * 2.0")

# Complex still routes through its own coercion, not the int/float slots.
show("1 + 2j")
show("2j + 1")
show("(2**70) + 2j")
show("1.5 - 2j")
show("3 * 2j")

# Non-numeric fallthrough: sequence concat and repetition stay intact.
show("[1, 2] + [3]")
show("(1, 2) + (3,)")
show("'ab' + 'cd'")
show("'ab' * 3")
show("3 * 'ab'")
show("[0] * 3")
show("b'x' + b'y'")
show("b'x' * 2")

# Non-numeric fallthrough: sequence repetition by a bigint count.
show("'a' * (2**70)")
show("[1] * (2**70)")
show("(1,) * (2**70)")
