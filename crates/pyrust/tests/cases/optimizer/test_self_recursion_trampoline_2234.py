# Self-recursion trampoline (#2234): a direct self-recursive call loops in the
# dispatch loop instead of recursing through the native call machinery.  These
# cases exercise the trampoline and the conditions that must fall back to the
# normal call path, all asserted against CPython 3.12 semantics.

# Plain self-recursion (trampolined).
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)
print([fib(i) for i in range(12)])

def fact(n):
    if n == 0:
        return 1
    return n * fact(n - 1)
print(fact(10))

# Self-recursion that returns None implicitly / explicitly.
def count_down(n, acc):
    acc.append(n)
    if n == 0:
        return
    return count_down(n - 1, acc)
acc = []
count_down(5, acc)
print(acc)

# Deep self-recursion (well within the limit) accumulating a value.
def summ(n):
    if n == 0:
        return 0
    return n + summ(n - 1)
print(summ(500))

# Unbounded self-recursion must still hit the recursion limit (the trampoline
# bypasses the native call guard, so it counts depth explicitly).
def runaway(n):
    return runaway(n + 1)
try:
    runaway(0)
except RecursionError:
    print("recursion limit enforced")

# Self-recursion with a try/except INSIDE the function: must NOT trampoline
# (has handlers) and must still catch a raise from a deeper frame.
def guarded(n):
    if n == 0:
        raise ValueError("bottom")
    try:
        return guarded(n - 1)
    except ValueError:
        return ("caught", n)
print(guarded(4))

# Self-recursion whose unhandled raise propagates out to a module-level handler.
def boom(n):
    if n == 0:
        raise KeyError("k")
    return boom(n - 1)
try:
    boom(3)
except KeyError as e:
    print("propagated:", str(e))

# Self-recursion that ends by calling a *different* function.
def helper(x):
    return x * 100

def chain(n):
    if n == 0:
        return helper(7)
    return chain(n - 1)
print(chain(5))

# Mutual recursion (not self-recursion: must use the normal call path).
def is_even(n):
    if n == 0:
        return True
    return is_odd(n - 1)

def is_odd(n):
    if n == 0:
        return False
    return is_even(n - 1)
print(is_even(10), is_odd(10), is_even(7))

# Self-recursion with a default argument (exact-arity call still trampolines;
# the defaulted call must fall back and bind the default).
def with_default(n, step=1):
    if n <= 0:
        return 0
    return step + with_default(n - step)
print(with_default(5))

# A method that recurses on a free function name (self-recursion through a
# global) — still trampolines via the self-bind register.
def ackermann_ish(m, n):
    if m == 0:
        return n + 1
    if n == 0:
        return ackermann_ish(m - 1, 1)
    return ackermann_ish(m - 1, ackermann_ish(m, n - 1))
print(ackermann_ish(2, 3))
