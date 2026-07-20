# Typed built-ins (`ord`, `abs`, `chr`, `hex`, `oct`, `bin`, `ascii`, `repr`, …)
# gain a "vectorcall" fast dispatch: a warm-cached positional call passes its
# argument values straight from the registers, skipping the ExpandedCallArg
# buffer + kwarg/arity validation. This must be observationally identical to
# the general path, including overload dispatch, re-entry into user code,
# rebinding, and the arity/type errors.

# values
print(ord("A"), ord("z"), ord("😀"), ord(b"B"))
print(abs(-5), abs(3.14), abs(-2), abs(complex(3, 4)))
print(chr(65), chr(0x1F600))
print(hex(255), oct(8), bin(5), ascii("café"))
print(repr("x"), repr([1, 2]), repr((3,)))

# warm the fast path in a loop
s = 0
for c in "abcdefghij" * 100:
    s += ord(c)
print(s)
t = 0
for i in range(1000):
    t += abs(i - 500)
print(t)

# overload dispatch (str vs bytes) at one repeated call site
for v in ["A", b"B", "z", b"9"]:
    print(ord(v))

# a fast built-in that re-enters user code (holds a register subslice while
# calling __abs__ / __index__ — exercises the borrow soundness)
class WithAbs:
    def __abs__(self):
        return "custom-abs"

class WithIndex:
    def __index__(self):
        return 66

print(abs(WithAbs()))
print(chr(WithIndex()))

# polymorphic call site: fast built-in one iteration, user fn the next
def user(x):
    return ("user", x)

for f in (ord, user, abs):
    if f is ord:
        print(f("Q"))
    elif f is user:
        print(f(1))
    else:
        print(f(-3))

# rebinding a fast built-in takes effect immediately
saved = ord
ord = lambda c: -1  # noqa: E731
print(ord("a"))
del ord
print(ord("a"))

# a local shadowing a fast built-in inside a function
def shadowed():
    abs = lambda x: "shadow"  # noqa: E731
    return abs(-9)
print(shadowed(), abs(-9))

# arity / type errors must match the general path
for expr in [
    'ord("ab")',
    'ord("")',
    'ord()',
    'ord("a", "b")',
    'abs()',
    'abs(1, 2)',
    'chr(-1)',
]:
    try:
        eval(expr)
        print(expr, "-> ok")
    except (TypeError, ValueError) as e:
        print(expr, "->", type(e).__name__)
