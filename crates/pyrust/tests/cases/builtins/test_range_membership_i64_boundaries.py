# i64-backed range membership must widen the endpoint delta before modulo.
# Each endpoint still fits in i64, but their difference can approach 2**64.

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1


def show_membership(r, value):
    print(value in r, r.__contains__(value), r.count(value))


ascending = range(I64_MIN, I64_MAX, 2)
show_membership(ascending, I64_MAX - 1)
show_membership(ascending, I64_MAX - 2)
show_membership(ascending, I64_MAX)  # exclusive stop

descending = range(I64_MAX, I64_MIN, -2)
show_membership(descending, I64_MIN + 1)
show_membership(descending, I64_MIN + 2)
show_membership(descending, I64_MIN)  # exclusive stop

# These deltas exceed i64::MAX.  An i64 wrapping subtraction reverses their
# divisibility by three in release builds, so exercise both membership results.
wide_step = range(I64_MIN, I64_MAX, 3)
show_membership(wide_step, 1)
show_membership(wide_step, 2)

# Here `value - start == i64::MIN`; i64 remainder by -1 also overflows.
unit_descending = range(I64_MAX, I64_MIN, -1)
show_membership(unit_descending, -1)
