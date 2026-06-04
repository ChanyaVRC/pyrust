# Exercises the user-function call dispatch fast path (#frame-setup-trim):
# `call_function_expanded` routes a plain `UserFunction` value straight to
# `call_user_function_expanded`, bypassing the super-bound-builtin probe,
# the builtin registry lookup, and the large `match function.kind()` cascade.
# All of these call forms must remain byte-identical to CPython.

# Direct positional call.
def add(a, b, c):
    return a + b + c
print(add(1, 2, 3))

# Keyword + default arguments through the fast path.
def greet(name, greeting="hi", punct="!"):
    return greeting + " " + name + punct
print(greet("x"))
print(greet("y", punct="?"))
print(greet("z", "yo", "."))

# Recursion (callee value is a UserFunction reached via the self-bind register).
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)
print(fib(10))

# Passed as a first-class value into builtins that re-enter the dispatcher.
def sq(x):
    return x * x
print(list(map(sq, [1, 2, 3, 4])))
print(list(filter(lambda v: v % 2 == 0, range(10))))
print(sorted([3, 1, 2], key=lambda v: -v))

# Closure / nested user function.
def make_adder(n):
    def inner(x):
        return x + n
    return inner
print(make_adder(10)(5))

# *args / **kwargs still take the variadic path correctly.
def collect(*args, **kwargs):
    return (args, sorted(kwargs.items()))
print(collect(1, 2, 3, a=1, b=2))

# A user function that itself calls other user functions (nested frames).
def outer(v):
    return add(v, sq(v), make_adder(1)(v))
print(outer(4))

# Exception raised from a user function called via the fast path, caught outside.
def boom():
    raise ValueError("nope")
try:
    boom()
except ValueError as e:
    print("caught", e.args[0])
