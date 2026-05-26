# Keyword-only parameters (after bare * or *args) must not accept positional args.
# Issue #1261: pyrust was silently filling kwonly params with positional values.

# Basic: bare * separator
def f(a, *, b):
    return (a, b)

print(f(1, b=2))       # (1, 2)
print(f(a=1, b=2))     # (1, 2)

try:
    f(1, 2)
except TypeError as e:
    print(e)  # f() takes 1 positional argument but 2 were given

# Multiple positional params before *
def h(a, b, *, c):
    return (a, b, c)

print(h(1, 2, c=3))    # (1, 2, 3)

try:
    h(1, 2, 3)
except TypeError as e:
    print(e)  # h() takes 2 positional arguments but 3 were given

# No positional params at all
def g(*, a):
    return a

print(g(a=1))           # 1

try:
    g(1)
except TypeError as e:
    print(e)  # g() takes 0 positional arguments but 1 was given

# Keyword-only with default (the default must still apply)
def d(a, *, b=10):
    return (a, b)

print(d(1))             # (1, 10)
print(d(1, b=5))        # (1, 5)

try:
    d(1, 2)
except TypeError as e:
    print(e)  # d() takes 1 positional argument but 2 were given

# Kwonly after *args: *args absorbs positionals, kwonly must be keyword
def v(*args, sep):
    return sep.join(str(x) for x in args)

print(v(1, 2, 3, sep="-"))  # 1-2-3

try:
    v(1, 2, 3)
except TypeError as e:
    print(e)  # v() missing 1 required keyword-only argument: 'sep'

# **kwargs alongside kwonly params
def k(*, a, **kwargs):
    return (a, kwargs)

try:
    k(1, b=2)
except TypeError as e:
    print(e)  # k() takes 0 positional arguments but 1 was given

print(k(a=1, b=2))     # (1, {'b': 2})
