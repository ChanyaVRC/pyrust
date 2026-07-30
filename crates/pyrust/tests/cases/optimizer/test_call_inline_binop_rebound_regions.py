# `pass_inline_leaf_binop` binds each call site to the latest `MakeFunction`
# store *before* it, so one name re-`def`ed across a module guards each region
# against its own proto.  Every shape below re-points a guarded name at runtime;
# the guard's code-object identity check has to decide per call whether the
# inline body is still the one the site was compiled for, and every printed
# value here is what the real call would produce.
#
# Sibling of test_call_inline_binop_deopt.py, which pins the guard's behaviour
# for one `def` per name; this file is specifically about several `def`s sharing
# a name, and about `def`s the compiler cannot pin to a single stream position
# (branch arms, `del` + re-`def`, `exec`).

ONE = 1
TWO = 2
THREE = 3
BIG = 1 << 70
HALF = 0.5


# ── Two regions of one re-`def`ed name ───────────────────────────────────────
def region(a, b):
    return a + b


first = 0
for i in range(60):
    first += region(i, ONE)
print("region 1 (add)", first)

print("region 1 literals", region(1073741824, 1073741825), region(BIG, ONE))


def region(a, b):
    return a * b


second = 0
for i in range(60):
    second += region(i, TWO)
print("region 2 (mul)", second)

print("region 2 literals", region(1 << 40, 1 << 40), region(HALF, THREE))


def region(a, b):
    return b - a


third = 0
for i in range(60):
    third += region(i, THREE)
print("region 3 (swapped sub)", third)


# The three protos are distinct code objects even though two share a body.
def twin(a, b):
    return a + b


twin_first = twin
twin_first_result = 0
for i in range(30):
    twin_first_result += twin(i, ONE)


def twin(a, b):
    return a + b


twin_second_result = 0
for i in range(30):
    twin_second_result += twin(i, ONE)
print("twins", twin_first_result, twin_second_result, twin is twin_first)
print("twin still callable", twin_first(2, 3), twin(2, 3))


# ── A region re-`def`ed to a body the pass cannot inline ─────────────────────
def narrowing(a, b):
    return a + b


narrow_add = 0
for i in range(40):
    narrow_add += narrowing(i, ONE)
print("narrowing add", narrow_add)


def narrowing(a, b):
    return a // b


narrow_div = 0
for i in range(1, 40):
    narrow_div += narrowing(i, TWO)
print("narrowing floordiv", narrow_div)

try:
    narrowing(1, 0)
except ZeroDivisionError as error:
    frames = []
    frame = error.__traceback__
    while frame is not None:
        frames.append(frame.tb_frame.f_code.co_name)
        frame = frame.tb_next
    print("narrowing frames", frames, str(error))


# And back to an inlinable body again.
def narrowing(a, b):
    return a - b


narrow_sub = 0
for i in range(40):
    narrow_sub += narrowing(i, ONE)
print("narrowing sub", narrow_sub)


# ── `def`s in branch arms: the site binds one arm, the other deopts ──────────
def branchy(flag):
    if flag:

        def pick(a, b):
            return a + b

    else:

        def pick(a, b):
            return a * b

    left = 7
    right = 6
    total = 0
    for i in range(40):
        total += pick(left, right)
    return total, pick(2, 3)


print("branch true", branchy(True))
print("branch false", branchy(False))


def looping_redef(n):
    def step(a, b):
        return a + b

    one = 1
    seen = []
    for i in range(n):
        seen.append(step(i, one))
        if i == 2:

            def step(a, b):
                return a * b

    return seen


print("redef inside loop", looping_redef(6))


# ── `del` between regions ────────────────────────────────────────────────────
def deleted(a, b):
    return a + b


deleted_before = 0
for i in range(30):
    deleted_before += deleted(i, ONE)
print("before del", deleted_before)

del deleted

try:
    deleted(1, 2)
except NameError as error:
    print("after del", type(error).__name__, str(error))


def deleted(a, b):
    return a * b


deleted_after = 0
for i in range(30):
    deleted_after += deleted(i, TWO)
print("after redef", deleted_after)


# ── `exec` installing a binding the compiler never saw ───────────────────────
def installed(a, b):
    return a + b


installed_first = 0
for i in range(30):
    installed_first += installed(i, ONE)
print("installed compiled", installed_first)

exec("def installed(a, b):\n    return a * b\n")

installed_second = 0
for i in range(30):
    installed_second += installed(i, TWO)
print("installed via exec", installed_second)


# A name that only ever exists because `exec` created it has no compile-time
# proto at all, so no site there can be guarded.
exec("def only_execed(a, b):\n    return b - a\n")
only_execed_total = 0
for i in range(30):
    only_execed_total += only_execed(i, THREE)
print("only exec", only_execed_total)


# ── Rebinding a re-`def`ed name mid-loop, per region ─────────────────────────
def swapper(a, b):
    return a + b


def replacement(a, b):
    return a * 100


swapped_first = []
for i in range(6):
    swapped_first.append(swapper(i, ONE))
    if i == 2:
        swapper = replacement
print("swap region 1", swapped_first)


def swapper(a, b):
    return a - b


swapped_second = []
for i in range(6):
    swapped_second.append(swapper(i, ONE))
    if i == 2:
        swapper = replacement
print("swap region 2", swapped_second)


# ── Non-int arguments still reach the region's own body ──────────────────────
def typed(a, b):
    return a + b


print("typed add", typed("a", "b"), typed([1], [2]), typed(HALF, ONE), typed(BIG, BIG))


def typed(a, b):
    return a * b


print("typed mul", typed("ab", THREE), typed((7,), TWO), typed(HALF, THREE), typed(BIG, TWO))

try:
    typed("ab", "cd")
except TypeError as error:
    print("typed error", type(error).__name__)


# ── Nested scopes: each function's own stream is scanned independently ───────
def outer(n):
    def inner(a, b):
        return a + b

    one = 1
    total = 0
    for i in range(n):
        total += inner(i, one)

    def inner(a, b):
        return a * b

    product = 1
    two = 2
    for i in range(n):
        product = inner(product, two)
    return total, product


print("nested regions", outer(10))
