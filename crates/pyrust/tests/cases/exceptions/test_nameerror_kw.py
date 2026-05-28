# CPython 3.12: NameError.__init__ accepts a `name=` keyword argument;
# ImportError.__init__ accepts `name=` and `path=`.
# These keyword values are stored as instance attributes but are NOT
# included in `.args` (only positional args appear there).

# --- NameError with name= ---
e = NameError("msg", name="x")
print(e.name)        # x
print(e.args)        # ('msg',)

# --- NameError without name= has name=None ---
e2 = NameError("msg")
print(e2.name)       # None

# --- NameError name= overrides the default None ---
e3 = NameError("test", name="myvar")
print(e3.name)       # myvar

# --- UnboundLocalError (subclass of NameError) accepts name= ---
e4 = UnboundLocalError("unbound", name="y")
print(e4.name)       # y

# --- User-defined NameError subclass accepts name= ---
class MyNameError(NameError):
    pass
e5 = MyNameError("custom", name="z")
print(e5.name)       # z

# --- NameError: unknown kwarg raises TypeError with specific message ---
try:
    NameError("msg", foo="bar")
except TypeError as te:
    print(te)        # 'foo' is an invalid keyword argument for NameError()

# --- NameError: two kwargs raises "takes at most 1" error ---
try:
    NameError("msg", name="x", extra="y")
except TypeError as te:
    print(te)        # NameError() takes at most 1 keyword argument (2 given)

# --- NameError: two unknown kwargs also raises "takes at most 1" ---
try:
    NameError("msg", foo="a", bar="b")
except TypeError as te:
    print(te)        # NameError() takes at most 1 keyword argument (2 given)

# --- NameError: path= is not accepted ---
try:
    NameError("msg", path="/x")
except TypeError as te:
    print(te)        # 'path' is an invalid keyword argument for NameError()

# --- ImportError with name= and path= ---
e6 = ImportError("msg", name="mymod", path="/some/path.py")
print(e6.name)       # mymod
print(e6.path)       # /some/path.py
print(e6.args)       # ('msg',)

# --- ImportError without kwargs has name=None, path=None ---
e7 = ImportError("msg")
print(e7.name)       # None
print(e7.path)       # None

# --- ImportError with only name= ---
e8 = ImportError("msg", name="somemod")
print(e8.name)       # somemod
print(e8.path)       # None

# --- ImportError with only path= ---
e9 = ImportError("msg", path="/p")
print(e9.name)       # None
print(e9.path)       # /p

# --- ModuleNotFoundError (subclass of ImportError) accepts name= and path= ---
e10 = ModuleNotFoundError("msg", name="foo", path="/bar")
print(e10.name)      # foo
print(e10.path)      # /bar

# --- User-defined ImportError subclass accepts name= and path= ---
class MyImportError(ImportError):
    pass
e11 = MyImportError("msg", name="mod", path="/m")
print(e11.name)      # mod
print(e11.path)      # /m

# --- ImportError: unknown kwarg raises TypeError ---
try:
    ImportError("msg", baz="x")
except TypeError as te:
    print(te)        # 'baz' is an invalid keyword argument for ImportError()

# --- ImportError: name + path + unknown raises TypeError for unknown ---
try:
    ImportError("msg", name="m", path="/p", bad="x")
except TypeError as te:
    print(te)        # 'bad' is an invalid keyword argument for ImportError()

# --- Error messages use base class name, not subclass name ---
# UnboundLocalError errors say "NameError()" not "UnboundLocalError()"
try:
    UnboundLocalError("msg", foo="x")
except TypeError as te:
    print(te)        # 'foo' is an invalid keyword argument for NameError()

try:
    UnboundLocalError("msg", foo="a", bar="b")
except TypeError as te:
    print(te)        # NameError() takes at most 1 keyword argument (2 given)

# ModuleNotFoundError errors say "ImportError()" not "ModuleNotFoundError()"
try:
    ModuleNotFoundError("msg", bad="x")
except TypeError as te:
    print(te)        # 'bad' is an invalid keyword argument for ImportError()

# --- Other exception classes still reject all kwargs ---
try:
    ValueError("msg", foo="bar")
except TypeError as te:
    print(te)        # ValueError() takes no keyword arguments

try:
    SyntaxError("msg", foo="bar")
except TypeError as te:
    print(te)        # SyntaxError() takes no keyword arguments
