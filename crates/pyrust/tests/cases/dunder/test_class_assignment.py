# Issue #1957: `obj.__class__ = NewType` re-types the instance.
class X:
    pass


class Y:
    pass


o = X()
o.__class__ = Y
print(type(o).__name__)
print(isinstance(o, Y))
print(isinstance(o, X))
print(o.__class__.__name__)

# Re-typing preserves the instance's existing attributes.
o2 = X()
o2.tag = 7
o2.__class__ = Y
print(o2.tag)

# Round-trip back to the original class.
o2.__class__ = X
print(type(o2).__name__)

# Assigning a non-class raises TypeError with CPython's message.
try:
    o.__class__ = 5
except TypeError as e:
    print(e)

try:
    o.__class__ = "str"
except TypeError as e:
    print(e)

# Re-typing to a built-in immutable class is rejected (CPython parity).
for T in (int, str, list, tuple, dict, object):
    try:
        X().__class__ = T
        print(T.__name__, "unexpected OK")
    except TypeError as e:
        print(T.__name__, e)


# Re-typing works even when the instance's class declares __slots__:
# __class__ is a type-level slot, not subject to __slots__ enforcement.
class SA:
    __slots__ = ("x",)


class SB:
    __slots__ = ("x",)


sa = SA()
sa.x = 11
sa.__class__ = SB
print(type(sa).__name__, sa.x)

# Re-typing rejects classes whose physical instance layouts differ.
def try_retype(label, value, target):
    try:
        value.__class__ = target
        print(label, "OK")
    except TypeError as e:
        print(label, str(e))


class DifferentSlot:
    __slots__ = ("y",)


class PlainLayout:
    pass


class EmptySlots:
    __slots__ = ()


class SlotWithDict:
    __slots__ = ("x", "__dict__")


try_retype("different-slot", SA(), DifferentSlot)
try_retype("slot-to-plain", SA(), PlainLayout)
try_retype("empty-to-plain", EmptySlots(), PlainLayout)
try_retype("slot-dict-difference", SlotWithDict(), SA)

# Layout-neutral subclasses may differ, but adding a slot on top of distinct
# slotted bases makes those base identities part of the layout contract.
class BaseSlotA:
    __slots__ = ("base",)


class BaseSlotB:
    __slots__ = ("base",)


class SharedChildA(BaseSlotA):
    __slots__ = ("child",)


class SharedChildB(BaseSlotA):
    __slots__ = ("child",)


class DifferentBaseChild(BaseSlotB):
    __slots__ = ("child",)


shared = SharedChildA()
shared.base = 1
shared.child = 2
shared.__class__ = SharedChildB
print("shared-base", type(shared).__name__, shared.base, shared.child)
try_retype("different-slot-base", SharedChildA(), DifferentBaseChild)

# Methods resolve through the new class after re-typing.
class WhoA:
    def who(self):
        return "A"


class WhoB:
    def who(self):
        return "B"


w = WhoA()
print(w.who())
w.__class__ = WhoB
print(w.who())
