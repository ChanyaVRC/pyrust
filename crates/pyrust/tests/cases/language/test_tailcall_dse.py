# Regression test: dead-store elimination must not remove the callee register
# for a tail call.  In the original bug, insn_reads_reg for TailCall only
# checked argument registers, not args_base-1 (the function register).

def count_down(n):
    if n <= 0:
        return 0
    return count_down(n - 1)

assert count_down(100) == 0


# Mutual tail calls — ensures the function loaded into the callee slot is kept.
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


print("tailcall dse OK")
