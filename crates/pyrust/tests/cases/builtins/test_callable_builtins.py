# callable() with built-in bound methods, bare builtin functions, and non-callables.
# Issue #719: callable("".upper) and siblings returned False instead of True.

# Built-in bound methods produced by attribute access on Tier 1 types.
print(callable("".upper))      # True
print(callable([].append))     # True
print(callable({}.update))     # True
print(callable(b"x".upper))    # True
print(callable((1,).count))    # True
print(callable(set().add))     # True

# Bare built-in functions.
print(callable(len))           # True
print(callable(print))         # True
print(callable(abs))           # True

# Non-callables.
print(callable(42))            # False
print(callable("hello"))       # False
print(callable(None))          # False
print(callable(True))          # False
print(callable([]))            # False
print(callable({}))            # False

# User classes with and without __call__.
class MyCallable:
    def __call__(self):
        pass

class NotCallable:
    pass

print(callable(MyCallable()))  # True
print(callable(NotCallable())) # False

# Classes themselves are callable (they construct instances).
print(callable(int))           # True
print(callable(str))           # True
print(callable(list))          # True
