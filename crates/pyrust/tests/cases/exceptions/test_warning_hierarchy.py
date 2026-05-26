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
