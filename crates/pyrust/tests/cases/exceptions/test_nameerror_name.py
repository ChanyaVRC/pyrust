# CPython 3.12: NameError instances expose the identifier that was not found
# as the `.name` attribute.  Manually raised instances have `name = None`.

# Simple undeclared name
try:
    foo_bar_baz
except NameError as e:
    print(e.name)                     # foo_bar_baz
    print(e.args[0])                  # name 'foo_bar_baz' is not defined

# Manually raised NameError has name=None
e = NameError("msg")
print(e.name)                         # None

# UnboundLocalError is a subclass of NameError; its .name is None in CPython 3.12
e2 = UnboundLocalError("msg")
print(e2.name)                        # None

# del of an undefined name: .name carries the identifier
try:
    del __nonexistent_del__
except NameError as e:
    print(e.name)                     # __nonexistent_del__

# User-defined NameError subclass: .name defaults to None
class MyNameError(NameError):
    pass
e3 = MyNameError("test")
print(e3.name)                        # None

# Catching pattern — e.name == identifier is a real-world idiom
def check():
    try:
        some_undefined_var
    except NameError as e:
        if e.name == "some_undefined_var":
            print("matched")

check()                               # matched

# Module-scope del raises NameError with the identifier in .name
x = 42
del x
try:
    print(x)
except NameError as e:
    print(e.name)                     # x
