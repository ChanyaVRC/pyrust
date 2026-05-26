# Tests for type.__repr__ / type.__str__ including module prefix (issue #1276).
# CPython 3.12 rule: use "{module}.{qualname}" when __module__ != "builtins",
# otherwise use just "{qualname}".

# --- Basic user-defined class ---
class Foo:
    pass

print(repr(Foo))       # <class '__main__.Foo'>
print(str(Foo))        # <class '__main__.Foo'>

# --- Nested class: qualname includes the outer name ---
class Outer:
    class Inner:
        pass

print(repr(Outer))        # <class '__main__.Outer'>
print(repr(Outer.Inner))  # <class '__main__.Outer.Inner'>

# --- Built-in types omit the module prefix ---
print(repr(int))          # <class 'int'>
print(repr(str))          # <class 'str'>
print(repr(list))         # <class 'list'>
print(repr(dict))         # <class 'dict'>
print(repr(float))        # <class 'float'>

# --- Built-in exceptions omit the module prefix ---
print(repr(ValueError))   # <class 'ValueError'>
print(repr(TypeError))    # <class 'TypeError'>

# --- Explicit __module__ override ---
class Bar:
    __module__ = "mymod"

print(repr(Bar))          # <class 'mymod.Bar'>

# --- __module__ = "builtins" -> omit prefix ---
class Qux:
    __module__ = "builtins"

print(repr(Qux))          # <class 'Qux'>

# --- __module__ = None -> omit prefix (same as CPython) ---
class Nul:
    __module__ = None

print(repr(Nul))          # <class 'Nul'>

# --- repr via an instance's type ---
class MyClass:
    pass

obj = MyClass()
print(repr(type(obj)))    # <class '__main__.MyClass'>

# --- __module__ accessible and correct ---
class Clz:
    pass

print(Clz.__module__)     # __main__
