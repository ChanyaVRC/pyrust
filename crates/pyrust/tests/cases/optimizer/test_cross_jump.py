# Verify the cross-jump optimisation is transparent — output must match CPython.

def with_merge(cond):
    if cond:
        x = 10
    else:
        x = 20
    return x + 1   # common tail: BinOpConst + Return

print(with_merge(True))   # 11
print(with_merge(False))  # 21

def multi_tail(n):
    if n == 1:
        a = "one"
        print(a)
        return len(a)
    elif n == 2:
        a = "two"
        print(a)
        return len(a)
    return 0

print(multi_tail(1))  # one\n3
print(multi_tail(2))  # two\n3
print(multi_tail(3))  # 0

# Three-arm if/elif/else: all arms share the same 2-instruction tail.
# Fixed-point iteration merges both duplicate tails; output must still be correct.
def three_arms(n):
    if n == 1:
        x = 1
        return x + 100
    elif n == 2:
        x = 2
        return x + 100
    else:
        x = 3
        return x + 100

print(three_arms(1))  # 101
print(three_arms(2))  # 102
print(three_arms(3))  # 103
