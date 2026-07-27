# Regression test: dead-store elimination must preserve the function and
# argument registers read by recursive calls.

def count_down(n):
    if n <= 0:
        return 0
    return count_down(n - 1)

assert count_down(100) == 0


# Mutual recursion ensures the function loaded into the callee slot is kept.
def is_even(n):
    if n == 0:
        return True
    return is_odd(n - 1)

def is_odd(n):
    if n == 0:
        return False
    return is_even(n - 1)

assert is_even(50) == True
assert is_odd(51) == True


# Deeper recursion — verifies DSE stays correct at larger call depths.
def count_down_deep(n):
    if n <= 0:
        return 0
    return count_down_deep(n - 1)

assert count_down_deep(500) == 0


print("recursive call dse OK")
