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
