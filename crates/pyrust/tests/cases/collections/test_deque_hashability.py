import collections
import sys


def show(label, operation):
    try:
        operation()
    except TypeError as exc:
        print(label + " -> TypeError: " + str(exc))
    else:
        print(label + " -> <success>")


# CPython exposes the unhashable slot on the type itself.  All hashed-container
# entry points must reject the exact canonical type with its qualified tp_name.
print("deque.__hash__ is None:", collections.deque.__hash__ is None)
show("hash deque", lambda: hash(collections.deque([1, 2])))
show("dict key deque", lambda: {collections.deque([1]): "value"})
show("set member deque", lambda: {collections.deque([1])})


# Plain subclasses inherit unhashability but retain their own bare diagnostic
# name.  An explicit __hash__ override must still re-enable every key path.
class Child(collections.deque):
    pass


print("Child.__hash__ is None:", Child.__hash__ is None)
show("hash Child", lambda: hash(Child([1])))
show("dict key Child", lambda: {Child([1]): "value"})
show("set member Child", lambda: {Child([1])})


class CustomHash(collections.deque):
    def __hash__(self):
        return 7


custom = CustomHash([1])
print("custom hash:", hash(custom))
print("custom dict key:", {custom: "value"}[custom])
print("custom set member:", custom in {custom})


class ReenabledHash(collections.deque):
    __hash__ = object.__hash__


print("reenabled hash type:", type(hash(ReenabledHash())).__name__)


# Re-importing collections creates a fresh class generation.  Retained old
# values and the fresh canonical class must independently keep the None slot.
old_deque = collections.deque
old_value = old_deque([1])
sys.modules.pop("collections", None)
import collections as reloaded_collections

print("old deque.__hash__ is None:", old_deque.__hash__ is None)
print(
    "new deque.__hash__ is None:",
    reloaded_collections.deque.__hash__ is None,
)
show("hash old deque", lambda: hash(old_value))
show("hash new deque", lambda: hash(reloaded_collections.deque([1])))


# Existing mutable-container behaviour and diagnostic spellings are regression
# controls.  Counter stays bare; C-implemented collection types stay qualified.
for label, value in [
    ("list", []),
    ("dict", {}),
    ("set", set()),
    ("bytearray", bytearray()),
    ("OrderedDict", collections.OrderedDict()),
    ("Counter", collections.Counter()),
    ("defaultdict", collections.defaultdict(int)),
]:
    show("control " + label, lambda value=value: hash(value))
