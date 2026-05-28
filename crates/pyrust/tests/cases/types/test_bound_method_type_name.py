# Parity fixture: type(instance.method).__name__ returns "method" for bound
# user-defined methods and classmethod-bound methods, matching CPython 3.12.
# See issue #1502.

class Foo:
    def bar(self): pass
    @classmethod
    def clsbar(cls): pass
    @staticmethod
    def statbar(): pass

f = Foo()

# Bound instance method -> "method"
print(type(f.bar).__name__)

# Classmethod bound to instance -> "method"
print(type(f.clsbar).__name__)

# Staticmethod accessed on instance -> "function" (plain function, not bound)
print(type(f.statbar).__name__)

# Unbound function accessed on class -> "function"
print(type(Foo.bar).__name__)

# Builtin functions/methods are unaffected
print(type(len).__name__)
print(type([].append).__name__)

# type().__qualname__ matches __name__ for bound methods
print(type(f.bar).__qualname__)
print(type(f.clsbar).__qualname__)
