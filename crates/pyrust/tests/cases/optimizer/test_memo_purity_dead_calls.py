# Parity fixture for issue #2523: memo-purity only authorizes caching and reuse
# of supported results. It never authorizes eliminating a dead-result call.
#
# A function whose body is a bare comparison (`a < a`), a unary op (`-a`) or a
# raise-capable binary op (`a / a`) can dispatch a user dunder or raise. A
# dead-result call must keep that effect, while memo-pure self-recursive
# functions using `<`/`-` (fib/fact) retain their result cache. The observable
# output must match CPython 3.12.


# --- comparison dispatches __lt__ on a dead-result call -----------------------
class SpyLt:
    def __lt__(self, other):
        print("SpyLt.__lt__ called")
        return False


def cmp_dead(a):
    a < a  # result unused, but MUST dispatch __lt__


cmp_dead(SpyLt())
print("cmp_dead done")


# --- unary op dispatches __neg__ on a dead-result call ------------------------
class SpyNeg:
    def __neg__(self):
        print("SpyNeg.__neg__ called")
        return self


def neg_dead(a):
    -a  # result unused, but MUST dispatch __neg__


neg_dead(SpyNeg())
print("neg_dead done")


# --- comparison TypeError must propagate, not be swallowed --------------------
def cmp_typeerror(a):
    1 < a


try:
    cmp_typeerror("x")
    print("cmp_typeerror: no error (WRONG)")
except TypeError as e:
    print("cmp_typeerror:", e)


# --- raise-capable binary op (ZeroDivisionError), dead result -----------------
def div_dead(a):
    a / a


try:
    div_dead(0)
    print("div_dead: no error (WRONG)")
except ZeroDivisionError as e:
    print("div_dead:", e)


# --- transitive memo-pure call can still raise --------------------------------
# `inner` is memo-pure for supported results but can raise; `mid` calls it. A
# dead-result `mid(0)` must still surface the exception.
def make_transitive():
    def inner(x):
        y = x
        y = y
        return y / x  # can raise ZeroDivisionError

    def mid(x):
        return inner(x)

    return mid


mid = make_transitive()
try:
    mid(0)
    print("transitive: no error (WRONG)")
except ZeroDivisionError as e:
    print("transitive:", e)


# --- memoization preserved for <-using self-recursive functions ---------------
# fib uses `n < 2` and `n - 1` / `n - 2`, which remain memo-pure so the result
# cache keeps it fast and correct.
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


print("fib(30) =", fib(30))


# fact uses `n <= 1` and `n - 1`: same property.
def fact(n):
    if n <= 1:
        return 1
    return n * fact(n - 1)


print("fact(10) =", fact(10))


# --- a dead call to a memo-pure function remains a call -----------------------
# pure_add uses only non-raising arithmetic on its argument. Its dead-result
# invocation remains a runtime call, while live uses must still compute
# correctly.
def pure_add(a):
    return a + 1


def uses_pure(a):
    pure_add(a)  # dead result, but still observable under rebinding/tracing
    return pure_add(a) + pure_add(a)  # live uses


print("uses_pure(10) =", uses_pure(10))
print("all done")
