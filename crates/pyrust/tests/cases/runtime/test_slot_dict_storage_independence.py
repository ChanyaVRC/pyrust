# C-style member/exception slots and the Python-visible __dict__ occupy
# independent key spaces, even when their names are identical.


class Slotted:
    __slots__ = ("x", "__dict__")


obj = Slotted()
obj.x = 1
obj.__dict__["x"] = 2
for _ in range(20):  # warm the slot read inline cache
    obj.x
print(obj.x, obj.__dict__)
print(obj.__dict__.pop("x"), obj.x, obj.__dict__)

obj.__dict__["x"] = 3
obj.__dict__["other"] = 4
obj.__dict__.clear()
print(obj.x, obj.__dict__)

# A proxy obtained before wholesale replacement remains attached to the old
# visible mapping; neither mapping aliases the slot backing.
old = obj.__dict__
old["old-only"] = 5
replacement = {"x": 6, "new-only": 7}
obj.__dict__ = replacement
old["late"] = 8
print(obj.x, obj.__dict__ is replacement, obj.__dict__, old)
print(hasattr(obj, "old-only"), hasattr(obj, "late"))

# BaseException.args has data-descriptor precedence over a same-named dict key.
exc = ValueError("slot-args")
exc.__dict__["args"] = "dict-args"
for _ in range(20):
    exc.args
print(exc.args, exc.__dict__["args"], exc.__dict__)
try:
    del exc.args
except TypeError as error:
    print(type(error).__name__, str(error), exc.__dict__["args"])
print(exc.__dict__.pop("args"), exc.args)

# Replacement dictionaries may also carry colliding keys without changing the
# exception slots.
exc_dict = {"args": "replacement-args"}
exc.__dict__ = exc_dict
print(exc.args, exc.__dict__ is exc_dict, exc.__dict__["args"])

# Class-specific exception fields follow the same rule. Deleting their native
# field exposes None while leaving the ordinary dict key untouched.
stop = StopIteration(11)
stop.__dict__["value"] = "dict-value"
print(stop.value, stop.__dict__["value"])
del stop.value
print(stop.value, stop.__dict__["value"])

name_error = NameError("missing")
name_error.name = "slot-name"
name_error.__dict__["name"] = "dict-name"
print(name_error.name, name_error.__dict__["name"])
del name_error.name
print(name_error.name, name_error.__dict__["name"])

os_error = OSError(2, "missing")
os_error.__dict__["errno"] = "dict-errno"
print(os_error.errno, os_error.__dict__["errno"])
del os_error.errno
print(os_error.errno, os_error.__dict__["errno"])
