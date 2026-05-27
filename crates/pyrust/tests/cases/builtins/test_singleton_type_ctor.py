# Verify that calling singleton types as constructors matches CPython 3.12.
#
# CPython 3.12 behaviour:
#   type(None)()           -> None (the singleton)
#   type(NotImplemented)() -> NotImplemented (the singleton)
#   type(...)()            -> Ellipsis (the singleton)
#   Any of the above with arguments -> TypeError: <TypeName> takes no arguments
#
# Before fix (#1451), pyrust fell through to call_class_expanded which
# allocated a bogus PyInstance instead.

# Zero-arg calls return the singletons.
print(type(None)() is None)
print(type(NotImplemented)() is NotImplemented)
print(type(...)() is ...)

# Calls with positional arguments raise TypeError.
try:
    type(None)(1)
except TypeError as e:
    print(str(e))

try:
    type(NotImplemented)(1)
except TypeError as e:
    print(str(e))

try:
    type(...)(1)
except TypeError as e:
    print(str(e))

# Multiple arguments also raise TypeError.
try:
    type(None)(1, 2)
except TypeError as e:
    print(str(e))
