# CPython 3.12: Warning exception hierarchy must be available as builtins.

# All warning classes are accessible
print(Warning("base"))
print(UserWarning("user"))
print(DeprecationWarning("deprecated"))
print(PendingDeprecationWarning("pending"))
print(RuntimeWarning("runtime"))
print(SyntaxWarning("syntax"))
print(ResourceWarning("resource"))
print(FutureWarning("future"))
print(ImportWarning("import"))
print(UnicodeWarning("unicode"))
print(BytesWarning("bytes"))
print(EncodingWarning("encoding"))

# Warning is a subclass of Exception
print(issubclass(Warning, Exception))
print(isinstance(Warning(), Exception))

# All warning subclasses are subclasses of Warning
for cls in [UserWarning, DeprecationWarning, PendingDeprecationWarning,
            RuntimeWarning, SyntaxWarning, ResourceWarning, FutureWarning,
            ImportWarning, UnicodeWarning, BytesWarning, EncodingWarning]:
    print(issubclass(cls, Warning))
    print(issubclass(cls, Exception))

# isinstance checks
print(isinstance(UserWarning(), Warning))
print(isinstance(DeprecationWarning(), Warning))
print(isinstance(DeprecationWarning(), Exception))

# raise and except
try:
    raise UserWarning("oops")
except Warning as e:
    print("caught Warning:", e)

try:
    raise DeprecationWarning("old api")
except Warning as e:
    print("caught Warning:", e)

try:
    raise RuntimeWarning("risky")
except Exception as e:
    print("caught Exception:", e)

# Warning itself can be raised and caught
try:
    raise Warning("base warning")
except Warning as e:
    print("caught base Warning:", e)

# __name__ and __module__ attributes
print(Warning.__name__)
print(Warning.__module__)
print(DeprecationWarning.__name__)

# BaseException subclass check
print(issubclass(Warning, BaseException))
print(issubclass(UserWarning, BaseException))

# User-defined subclass of a warning class
class MyWarning(UserWarning):
    pass

print(issubclass(MyWarning, UserWarning))
print(issubclass(MyWarning, Warning))
print(issubclass(MyWarning, Exception))

try:
    raise MyWarning("custom")
except Warning as e:
    print("caught user subclass:", type(e).__name__, str(e))

# Multi-type except with Warning
try:
    raise RuntimeWarning("multi")
except (TypeError, Warning) as e:
    print("caught in multi-except:", type(e).__name__)

# args attribute
w = UserWarning("msg", 42)
print(w.args)
