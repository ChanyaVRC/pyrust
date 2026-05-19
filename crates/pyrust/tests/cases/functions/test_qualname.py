# Parity fixture for issue #606: __qualname__ and __name__ on user-defined functions.

# Top-level function: qualname equals name.
def foo():
    pass

print(foo.__qualname__)   # foo
print(foo.__name__)       # foo

# Method: qualname is "ClassName.method_name".
class C:
    def method(self):
        pass

print(C.method.__qualname__)  # C.method
print(C.method.__name__)      # method

# Nested function: qualname includes "<locals>".
def outer():
    def inner():
        pass
    return inner

print(outer().__qualname__)   # outer.<locals>.inner
print(outer().__name__)       # inner

# Lambda: both __name__ and __qualname__ are "<lambda>".
f = lambda: None
print(f.__name__)             # <lambda>
print(f.__qualname__)         # <lambda>

# Bound method: delegates to underlying function.
obj = C()
print(obj.method.__qualname__)  # C.method
print(obj.method.__name__)      # method

# Writable: __name__ and __qualname__ can be reassigned.
def bar():
    pass

bar.__qualname__ = "new_qualname"
bar.__name__ = "new_name"
print(bar.__qualname__)   # new_qualname
print(bar.__name__)       # new_name

# Function is still callable after attribute mutation.
bar()

# Error: assigning a non-string raises TypeError.
try:
    foo.__qualname__ = 42
except TypeError as e:
    print(e)   # __qualname__ must be set to a string object

try:
    foo.__name__ = []
except TypeError as e:
    print(e)   # __name__ must be set to a string object

# Error: deleting __qualname__ / __name__ raises TypeError.
try:
    del foo.__qualname__
except TypeError as e:
    print(e)   # __qualname__ must be set to a string object

try:
    del foo.__name__
except TypeError as e:
    print(e)   # __name__ must be set to a string object

# Error: accessing an unknown attribute raises AttributeError.
try:
    _ = foo.no_such_attr
except AttributeError as e:
    print(e)   # 'function' object has no attribute 'no_such_attr'

# Class inside a function: qualname uses "<locals>" prefix.
def make_cls():
    class Inner:
        pass
    return Inner

print(make_cls().__qualname__)  # make_cls.<locals>.Inner

# Deeply nested function: qualname accumulates all levels.
def level1():
    def level2():
        def level3():
            pass
        return level3
    return level2

print(level1()().__qualname__)  # level1.<locals>.level2.<locals>.level3
