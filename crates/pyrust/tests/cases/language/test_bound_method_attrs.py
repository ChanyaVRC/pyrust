# Parity fixture for issue #1262: bound methods must expose __func__, __self__,
# __annotations__, __defaults__, __kwdefaults__, and forward __name__,
# __qualname__, __module__, __doc__ from the underlying function.

class Foo:
    def bar(self, x: int, y: str = "hi") -> str:
        """docstring for bar"""
        return str(x)

    @classmethod
    def cm(cls, n: int) -> None:
        pass

    def no_defaults(self, a, b):
        pass

    def kwonly(self, a, *, k=42):
        pass


f = Foo()

# --- __func__ ---
# __func__ is the underlying unbound function.
func = f.bar.__func__
print(func.__name__)          # bar
print(func.__qualname__)      # Foo.bar

# __func__ on classmethod
cm_func = Foo.cm.__func__
print(cm_func.__name__)       # cm

# --- __self__ ---
# __self__ is the bound instance for regular methods.
print(type(f.bar.__self__).__name__)    # Foo

# __self__ on classmethod is the class itself.
print(Foo.cm.__self__ is Foo)          # True

# --- __annotations__ ---
print(f.bar.__annotations__)           # {'x': <class 'int'>, 'y': <class 'str'>, 'return': <class 'str'>}
print(Foo.cm.__annotations__)          # {'n': <class 'int'>, 'return': <class 'NoneType'>}

# __annotations__ identity: bound method and __func__ share the same dict.
print(f.bar.__annotations__ is f.bar.__func__.__annotations__)  # True

# --- __defaults__ ---
print(f.bar.__defaults__)              # ('hi',)
print(f.no_defaults.__defaults__)     # None

# --- __kwdefaults__ ---
print(f.kwonly.__kwdefaults__)         # {'k': 42}
print(f.bar.__kwdefaults__)            # None

# --- forwarded attrs that already worked ---
print(f.bar.__name__)                  # bar
print(f.bar.__qualname__)              # Foo.bar
print(f.bar.__doc__)                   # docstring for bar
print(f.bar.__module__)                # __main__

# --- __dict__ is still forwarded ---
Foo.bar.custom = 99
print(f.bar.custom)                    # 99
print(f.bar.__dict__)                  # {'custom': 99}

# --- AttributeError on genuinely missing attr ---
try:
    _ = f.bar.__nonexistent__
except AttributeError:
    print("AttributeError ok")        # AttributeError ok
