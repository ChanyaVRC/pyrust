# Regression test for issue #1475: pass_copy_prop must evict aliases through
# YieldFrom's result_reg and sent_reg, because that instruction writes both.
# writable_dst() can only express one destination, so YieldFrom needs explicit
# eviction.  Without the fix a local aliased to result_reg or sent_reg before
# the YieldFrom would be incorrectly substituted with the stale pre-yield value.

def inner():
    val = yield "inner_yield"
    return val * 2

def outer():
    a = 7
    r = yield from inner()
    print(a, r)

gen = outer()
print(next(gen))   # inner_yield
try:
    gen.send(7)    # delivers 7; inner returns 14; outer prints "7 14"
except StopIteration:
    pass
