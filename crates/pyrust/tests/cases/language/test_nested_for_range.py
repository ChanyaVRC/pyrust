# Regression test for issue #966: jump threading must not follow an outer
# loop's backward edge while rewriting a nested iterator loop's exit.


def sum_products():
    # Two-level nesting with non-trivial body.  Correct result: 18.
    total = 0
    for i in range(4):
        for j in range(3):
            total += i * j
    return total


def two_level_print():
    # Two-level nesting exercising i/j values.
    for i in range(2):
        for j in range(2):
            print(i, j)


def three_level():
    # Three-level nesting to ensure the fix scales.
    total = 0
    for i in range(3):
        for j in range(3):
            for k in range(3):
                total += i + j + k
    return total


def empty_inner():
    # Empty inner body (pass) — off == 1 for inner, must not crash.
    for i in range(4):
        for j in range(4):
            pass
    return 42


print(sum_products())    # 18
two_level_print()
print(three_level())     # 81
print(empty_inner())     # 42
