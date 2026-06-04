# Inter-procedural inlining of small pure leaf functions (issue #349).
#
# The optimizer splices the body of a small, statically-known, pure leaf
# function at its call site, eliminating the call frame.  Every case below must
# produce output byte-identical to CPython: inlining is a pure performance
# transform with no observable semantic change.


def square(x):
    return x * x


def add3(a, b, c):
    return a + b + c


def neg(x):
    return -x


def pair(a, b):
    return [a, b]


def shifted(x):
    return (x << 2) + 1


def nothing(x):
    y = x + 1  # noqa: F841


# ── Inlined in a tight loop: result must be exact. ──
s = 0
for i in range(10):
    s += square(i)
print("loop sum", s)

# ── Multiple positional arguments. ──
print("add3", add3(10, 20, 30))

# ── Unary op body. ──
print("neg", neg(7), neg(-3), neg(0))

# ── Body that builds a fresh object. ──
print("pair", pair(1, 2), pair("a", "b"))

# ── Folded-constant body. ──
print("shifted", shifted(5), shifted(0))

# ── ReturnNone body (no explicit return). ──
print("nothing", nothing(99))

# ── Inlined inside a while loop and a conditional. ──
n = 0
acc = 0
while n < 6:
    if n % 2 == 0:
        acc += square(n)
    else:
        acc += neg(n)
    n += 1
print("while/if", acc)

# ── Nested expression: inlined calls feeding inlined calls. ──
print("nested", add3(square(2), neg(3), square(-4)))

# ── Boundary integers: large products promote to BigInt, identical to CPython. ──
print("big", square(10**18))
print("min", square(-(2**40)))

# ── An exception raised inside the inlined body must report the same type and
#    message as CPython (the traceback frame list is not part of parity).
try:
    print(square("ab"))
except TypeError as e:
    print("caught", type(e).__name__)


# ── A helper with defaults must stay correct (defaults disqualify inlining,
#    so this exercises the real-call fallback path). ──
def with_default(x, y=100):
    return x + y


print("default", with_default(1), with_default(1, 2))


# ── A *args helper must stay correct (variadic disqualifies inlining). ──
def variadic(*args):
    return sum(args)


print("variadic", variadic(1, 2, 3, 4))


# ── Recursion must never be inlined (would explode); result stays correct. ──
def fact(k):
    if k <= 1:
        return 1
    return k * fact(k - 1)


print("fact", fact(6))


# ── Binding stability: rebinding through globals() must be observed, so a
#    scope that reifies its namespace is never inlined. ──
def helper(x):
    return x + 1


g = globals()
out = []
for j in range(3):
    if j == 2:
        g["helper"] = lambda x: x + 1000
    out.append(helper(j))
print("rebind", out)
