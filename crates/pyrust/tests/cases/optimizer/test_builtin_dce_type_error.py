# Issue #2537: a dead pure-builtin / pure-function call must still raise when
# its (constant) arguments or its body would raise.  The optimizer used to
# eliminate dead "pure" calls, silently swallowing the observable exception.


def try_raises(label, fn):
    try:
        fn()
    except Exception as exc:
        print(label, type(exc).__name__, exc)
    else:
        print(label, "NO ERROR")


# Wrapped in a (pure) function whose result is discarded — the call site is the
# dead pure-call that the optimizer previously dropped.
def abs_str():
    abs("x")


def ord_two():
    ord("ab")


def range_float():
    range(1.5)


def sum_int():
    sum(5)


def pure_div_zero():
    return 1 / 0


try_raises("abs_str", abs_str)
try_raises("ord_two", ord_two)
try_raises("range_float", range_float)
try_raises("sum_int", sum_int)
try_raises("pure_div_zero", pure_div_zero)

# Direct dead statements (no wrapper) must raise too.
try_raises("abs_direct", lambda: abs("x"))
try_raises("range_direct", lambda: range(1.5))

# Valid dead calls remain side-effect-free and must not raise.
def abs_ok():
    abs(-5)
    abs(2**70)
    abs(True)


def range_ok():
    range(3)
    range(True)


abs_ok()
range_ok()
print("valid dead calls ok")

# Results that ARE used must still compute correctly.
print(abs(-7), list(range(3)), sum([1, 2, 3]))
