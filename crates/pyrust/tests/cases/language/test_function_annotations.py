# CPython 3.12 parity fixture: function.__annotations__
# Covers: annotated, unannotated, mixed, writable, return-only, *args/**kwargs.

# Basic: parameter and return annotations
def f(x: int, y: str) -> bool:
    pass

print(f.__annotations__)        # {'x': <class 'int'>, 'y': <class 'str'>, 'return': <class 'bool'>}
print('x' in f.__annotations__)  # True
print('return' in f.__annotations__)  # True

# Unannotated function returns empty dict, not AttributeError
def g(a, b):
    pass

print(g.__annotations__)  # {}

# Mixed: only some params annotated, no return annotation
def h(x: int, y):
    pass

print(h.__annotations__)  # {'x': <class 'int'>}

# Return-only annotation
def k() -> None:
    pass

print(k.__annotations__)  # {'return': <class 'NoneType'>}

# *args and **kwargs annotations
def variadic(*args: int, **kwargs: str) -> None:
    pass

print(variadic.__annotations__)  # {'args': <class 'int'>, 'kwargs': <class 'str'>, 'return': None}

# Writable: f.__annotations__ = {...}
f.__annotations__ = {'x': int}
print(f.__annotations__)  # {'x': <class 'int'>}

# Setting non-dict raises TypeError
try:
    f.__annotations__ = "not a dict"
except TypeError as e:
    print(f"TypeError: {e}")

# del f.__annotations__ resets to empty dict
def d(x: int) -> str:
    pass

del d.__annotations__
print(d.__annotations__)  # {}

# Annotations evaluated at definition time (not lazily)
effects = []

def side_effect_fn(x: effects.append(1)):
    pass

print(effects)  # [1]

# Closure: annotation can reference enclosing-scope variable
def outer():
    T = list
    def inner(x: T) -> T:
        pass
    return inner

fn = outer()
print(fn.__annotations__)  # {'x': <class 'list'>, 'return': <class 'list'>}

# Class method annotations
class C:
    def method(self, n: int) -> str:
        pass

print(C.method.__annotations__)  # {'n': <class 'int'>, 'return': <class 'str'>}

print("function annotations OK")
