# Parity fixture for issue #1959: function introspection attributes
# __defaults__ / __kwdefaults__ / __globals__ / __closure__ / __code__.

def myf(a, b=2, *args, c=3, **kw):
    return a

# __defaults__ is a tuple of positional-param defaults.
print(myf.__defaults__)            # (2,)
# __kwdefaults__ is a dict of keyword-only defaults.
print(myf.__kwdefaults__)          # {'c': 3}
# __globals__ is the module namespace dict (mutations are visible).
print(isinstance(myf.__globals__, dict))   # True
# __closure__ is None for a non-closure function.
print(myf.__closure__)             # None

# __code__ exposes co_name / co_argcount / co_varnames.
print(type(myf.__code__).__name__)   # code
print(myf.__code__.co_name)          # myf
print(myf.__code__.co_argcount)      # 2
print(myf.__code__.co_varnames)      # ('a', 'b', 'c', 'args', 'kw')

# A function with no defaults / no keyword-only params reports None for both.
def g():
    pass

print(g.__defaults__)              # None
print(g.__kwdefaults__)            # None
print(g.__closure__)              # None
print(g.__code__.co_argcount)     # 0
print(g.__code__.co_varnames)     # ()

# Falsy defaults are preserved (not collapsed to None).
def h(x=0, y=False, z=None):
    return x

print(h.__defaults__)              # (0, False, None)

# Positional-only + keyword-only mix; co_varnames orders positionals first.
def mixed(a, b, /, c, d=4, *, e=5, f=6):
    return 0

print(mixed.__defaults__)          # (4,)
print(mixed.__kwdefaults__)        # {'e': 5, 'f': 6}
print(mixed.__code__.co_argcount)  # 4
print(mixed.__code__.co_varnames)  # ('a', 'b', 'c', 'd', 'e', 'f')

# A keyword-only param without a default is absent from __kwdefaults__.
def kwonly(*, only):
    return only

print(kwonly.__kwdefaults__)       # None

# __globals__ is the live module dict.
myf.__globals__['INJECTED'] = 123
print(INJECTED)                    # 123
