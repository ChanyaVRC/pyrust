# Issue #3294: pyrust resolves the class objects in `builtins` (the exception
# hierarchy and the built-in types) on first use instead of building them at
# startup.  That must be invisible: every lookup path, rebinding, deletion and
# `dict`-ordering rule below behaves exactly as it does on CPython, where the
# classes simply exist from the start.
#
# The order of the module's own names is deliberately not printed: pyrust lists
# them in declaration order, CPython in its C table order (see
# `stdlib/test_module_dict_order.py`).  Only order *relations* are asserted.

import builtins

SLOTS = ["__name__", "__doc__", "__package__", "__loader__", "__spec__"]


def raises(kind, action):
    try:
        action()
    except kind as error:
        return str(error)
    return None


# --- Reads, before the namespace has ever been observed as a whole ----------
print(ValueError is builtins.ValueError, getattr(builtins, "KeyError") is KeyError)
print(builtins.memoryview is memoryview, builtins.int is int, builtins.object is object)
print(hasattr(builtins, "frozenset"), hasattr(builtins, "BaseExceptionGroup"))
print(hasattr(builtins, "no_such_builtin"), getattr(builtins, "NoSuchError", "default"))
print(IOError is OSError, EnvironmentError is OSError)
print(issubclass(ZeroDivisionError, ArithmeticError), ZeroDivisionError.__mro__[-2:])

try:
    1 / 0
except ZeroDivisionError as error:
    print("caught", type(error) is builtins.ZeroDivisionError, error)

try:
    undefined_name_3294
except NameError as error:
    print("caught", type(error) is builtins.NameError, error.name)


# --- Rebinding a class nobody has listed yet --------------------------------
def read_value_error():
    return ValueError


class Custom(Exception):
    pass


original = read_value_error()
builtins.ValueError = Custom
print(read_value_error() is Custom, ValueError is Custom, builtins.ValueError is Custom)
print(getattr(builtins, "ValueError") is Custom, vars(builtins)["ValueError"] is Custom)
try:
    raise ValueError("custom")
except Custom as error:
    print("caught", type(error).__name__, error)
builtins.ValueError = original
print(read_value_error() is original)

# --- The namespace as a dict -------------------------------------------------
namespace = vars(builtins)
print(namespace is builtins.__dict__, list(namespace)[:5] == SLOTS)
print(len(namespace) == len(list(namespace)), sorted(namespace) == dir(builtins))
for name in ("ValueError", "memoryview", "bytearray", "len", "None", "__debug__"):
    print(name, name in namespace, name in dir(builtins), namespace[name] is getattr(builtins, name))

# Rebinding keeps a name's position; deleting shifts; re-binding appends.
before = list(namespace)
builtins.KeyError = KeyError
builtins.len = len
print(list(namespace) == before)

# --- Deletion never brings the class back -----------------------------------
saved = builtins.LookupError
del builtins.LookupError
print(raises(NameError, lambda: LookupError))
print(raises(AttributeError, lambda: builtins.LookupError))
print(getattr(builtins, "LookupError", "gone"), hasattr(builtins, "LookupError"))
print("LookupError" in vars(builtins), "LookupError" in dir(builtins))
print(raises(AttributeError, lambda: delattr(builtins, "LookupError")))
print(list(namespace) == [name for name in before if name != "LookupError"])
# A subclass is still catchable through its own binding.
try:
    {}["missing"]
except KeyError as error:
    print("caught", type(error).__name__, saved in type(error).__mro__)

builtins.LookupError = saved
print(LookupError is saved, list(namespace)[-1] == "LookupError")

# --- The same through the dict itself ---------------------------------------
popped = namespace.pop("TabError")
print(raises(NameError, lambda: TabError), hasattr(builtins, "TabError"))
namespace["TabError"] = popped
print(TabError is popped, list(namespace)[-1] == "TabError")

del namespace["EOFError"]
print(raises(NameError, lambda: EOFError), "EOFError" in dir(builtins))
namespace["EOFError"] = Custom
print(EOFError is Custom, builtins.EOFError is Custom)

namespace["probe_3294"] = 7
print(builtins.probe_3294, probe_3294, list(namespace)[-1])
del builtins.probe_3294
print(raises(NameError, lambda: probe_3294), "probe_3294" in namespace)
