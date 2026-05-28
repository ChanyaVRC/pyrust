# Parity fixture for positional-only keyword-argument error messages.
# CPython 3.12 collects all offending names and lists them together.

def foo(a, b, /, c):
    return a + b + c

# Both positional-only args passed as keywords: message lists 'a, b'
try:
    foo(a=1, b=2, c=3)
except TypeError as e:
    print(e)

# Single positional-only arg as keyword: message lists single name
try:
    foo(1, b=2, c=3)
except TypeError as e:
    print(e)

# All args given positionally: no error
print(foo(1, 2, c=3))

# Three positional-only args all passed as keywords
def baz(x, y, z, /):
    return x + y + z

try:
    baz(x=1, y=2, z=3)
except TypeError as e:
    print(e)

# Mixed: one given positionally, two as keywords
def qux(a, b, c, /):
    return a + b + c

try:
    qux(1, b=2, c=3)
except TypeError as e:
    print(e)

# With **kwargs: positional-only names are absorbed into kwargs, no error
def bar(a, b, /, c, **kwargs):
    return (a, b, c, kwargs)

print(bar(1, 2, 3, a=10, b=20))

# Posonly violation + unknown keyword: posonly error takes priority (CPython 3.12)
def f(a, b, /, c): pass
try:
    f(a=1, d=99)
except TypeError as e:
    print(e)
