# Shared numeric-protocol parity for constructors and float-like consumers.
# CPython accepts builtin subclasses returned by numeric slots (with a
# DeprecationWarning); the parity harness strips warning context, while the
# value and its plain builtin result type remain observable here.

import math


events = []


class IntSubclass(int):
    def __index__(self):
        events.append("int-subclass-override")
        return 99


class FloatSubclass(float):
    pass


class ComplexSubclass(complex):
    pass


class IndexValue:
    def __init__(self, value, label):
        self.value = value
        self.label = label

    def __index__(self):
        events.append(self.label)
        return self.value


class MissingIndex:
    pass


class BadIndexResult(float):
    pass


class RaisingIndex:
    def __index__(self):
        raise ValueError("index boom")


class BadIndexIterable:
    def __index__(self):
        return 1.5

    def __iter__(self):
        return iter([67])


class BadIndexOnly:
    def __index__(self):
        return 1.5


class TypeErrorIndexIterable:
    def __index__(self):
        raise TypeError("index type boom")

    def __iter__(self):
        return iter([68])


class TypeErrorIndexOnly:
    def __index__(self):
        raise TypeError("index type boom")


class ValueErrorIndexIterable:
    def __index__(self):
        raise ValueError("index value boom")

    def __iter__(self):
        return iter([69])


class IntSlotSubclassResult:
    def __int__(self):
        return IntSubclass(12)


class TruncSlotSubclassResult:
    def __trunc__(self):
        return IntSubclass(13)


class TruncSlotIndexResult:
    def __trunc__(self):
        return IndexValue(IntSubclass(14), "trunc-index")


class FloatSlotSubclassResult:
    def __float__(self):
        return FloatSubclass(3.5)


class ComplexSlotSubclassResult:
    def __complex__(self):
        return ComplexSubclass(4, 5)


class NumericMeta(type):
    def __float__(cls):
        return FloatSubclass(3.5)

    def __index__(cls):
        events.append("metaclass-index")
        return IntSubclass(2)


class NumericClass(metaclass=NumericMeta):
    pass


class NoNumericMeta(type):
    pass


class NoNumericClass(metaclass=NoNumericMeta):
    pass


class DirectInt(int):
    def __int__(self):
        events.append("DirectInt.__int__")
        return 41

    def __float__(self):
        events.append("DirectInt.__float__")
        return 4.25

    def __complex__(self):
        events.append("DirectInt.__complex__")
        return 4 + 1j


class InheritedInt(DirectInt):
    pass


class DirectFloat(float):
    def __int__(self):
        events.append("DirectFloat.__int__")
        return 42

    def __float__(self):
        events.append("DirectFloat.__float__")
        return 5.25

    def __complex__(self):
        events.append("DirectFloat.__complex__")
        return 5 + 2j


class InheritedFloat(DirectFloat):
    pass


class DirectComplex(complex):
    def __int__(self):
        events.append("DirectComplex.__int__")
        return 43

    def __float__(self):
        events.append("DirectComplex.__float__")
        return 6.25

    def __complex__(self):
        events.append("DirectComplex.__complex__")
        return 6 + 3j


class InheritedComplex(DirectComplex):
    pass


class PlainInt(int):
    pass


class PlainFloat(float):
    pass


class PlainComplex(complex):
    pass


class IndexOverrideInt(int):
    def __index__(self):
        events.append("IndexOverrideInt.__index__")
        return 99


class IntSlotNone(int):
    __int__ = None


class FloatSlotNone(float):
    __float__ = None


class ComplexSlotsNone(complex):
    __complex__ = None
    __float__ = None
    __index__ = None


class BadIntOverride(int):
    def __int__(self):
        return 1.5


class BadFloatOverride(float):
    def __float__(self):
        return 1


class BadComplexOverride(complex):
    def __complex__(self):
        return 1


class FloatCopiedInt(float):
    __int__ = int.__int__


class InheritedFloatCopiedInt(FloatCopiedInt):
    pass


def show(label, fn):
    try:
        value = fn()
        print(label, "=", repr(value), type(value).__name__)
    except Exception as error:
        print(label, "!", type(error).__name__, error)


# int(): __int__ -> __index__ -> __trunc__, plus explicit-base coercion.
show("int(index subclass result)", lambda: int(IndexValue(IntSubclass(7), "int")))
show("int(__int__ subclass result)", lambda: int(IntSlotSubclassResult()))
show("int(__trunc__ subclass result)", lambda: int(TruncSlotSubclassResult()))
show("int(__trunc__ index result)", lambda: int(TruncSlotIndexResult()))
show("int(base index subclass result)", lambda: int("101", IndexValue(IntSubclass(2), "base")))
show("int(base bool invalid)", lambda: int("10", True))
show("int(base 40 invalid)", lambda: int("10", 40))
show("int(base huge invalid)", lambda: int("10", 2**80))
show("int(base missing)", lambda: int("10", MissingIndex()))

# Float-like consumers keep __float__ ahead of __index__, but share index
# result validation and float-subclass result normalization.
show("float(index subclass result)", lambda: float(IndexValue(IntSubclass(8), "float")))
show("float(__float__ subclass result)", lambda: float(FloatSlotSubclassResult()))
show("complex(index subclass result)", lambda: complex(IndexValue(IntSubclass(9), "complex")))
show("complex(__float__ subclass result)", lambda: complex(FloatSlotSubclassResult()))
show("complex(__complex__ subclass result)", lambda: complex(ComplexSlotSubclassResult()))
show(
    "complex(second index subclass result)",
    lambda: complex(1, IndexValue(IntSubclass(3), "complex-second")),
)
show("math(index subclass result)", lambda: math.sqrt(IndexValue(IntSubclass(16), "math")))
show("math(__float__ subclass result)", lambda: math.sqrt(FloatSlotSubclassResult()))
show("printf(index subclass result)", lambda: "%0.1f" % IndexValue(IntSubclass(6), "printf"))
show("printf(__float__ subclass result)", lambda: "%0.1f" % FloatSlotSubclassResult())

# Count form and iterable-element form both use the same index boundary.
show("bytes(count index subclass result)", lambda: bytes(IndexValue(IntSubclass(3), "bytes-count")))
show("bytes(element index subclass result)", lambda: bytes([IndexValue(IntSubclass(65), "bytes-item")]))
show("bytearray(count index subclass result)", lambda: bytearray(IndexValue(IntSubclass(2), "bytearray-count")))
show("bytearray(element index subclass result)", lambda: bytearray([IndexValue(IntSubclass(66), "bytearray-item")]))
show("bytes(direct int subclass)", lambda: bytes(IntSubclass(2)))
show("bytes([direct int subclass])", lambda: bytes([IntSubclass(67)]))

# A TypeError from the optional count-form index probe means "try the iterable
# form". Other exceptions still belong to the slot and must propagate.
show("bytes(bad index iterable fallback)", lambda: bytes(BadIndexIterable()))
show("bytearray(bad index iterable fallback)", lambda: bytearray(BadIndexIterable()))
show("bytes(bad index noniterable)", lambda: bytes(BadIndexOnly()))
show("bytearray(bad index noniterable)", lambda: bytearray(BadIndexOnly()))
show("bytes(TypeError index iterable fallback)", lambda: bytes(TypeErrorIndexIterable()))
show(
    "bytearray(TypeError index iterable fallback)",
    lambda: bytearray(TypeErrorIndexIterable()),
)
show("bytes(TypeError index noniterable)", lambda: bytes(TypeErrorIndexOnly()))
show(
    "bytearray(TypeError index noniterable)",
    lambda: bytearray(TypeErrorIndexOnly()),
)
show("bytes(ValueError index iterable)", lambda: bytes(ValueErrorIndexIterable()))
show(
    "bytearray(ValueError index iterable)",
    lambda: bytearray(ValueErrorIndexIterable()),
)

# A class object resolves numeric slots on its metaclass.
show("subscript(metaclass index)", lambda: [10, 20, 30][NumericClass])
show("int(metaclass index)", lambda: int(NumericClass))
show("int(base metaclass index)", lambda: int("101", NumericClass))
show("float(metaclass float first)", lambda: float(NumericClass))
show("complex(metaclass float first)", lambda: complex(NumericClass))
show("math(metaclass float first)", lambda: math.sqrt(NumericClass))
show("printf(metaclass float first)", lambda: "%0.1f" % NumericClass)
show("bytes(metaclass index)", lambda: bytes(NumericClass))
show("bytes([metaclass index])", lambda: bytes([NumericClass]))

# Class-object diagnostics name the object's metaclass, not always "type".
show("int(custom metaclass diagnostic)", lambda: int(NoNumericClass))
show("float(custom metaclass diagnostic)", lambda: float(NoNumericClass))
show("complex(custom metaclass diagnostic)", lambda: complex(NoNumericClass))
show("bytes(custom metaclass diagnostic)", lambda: bytes(NoNumericClass))
show("subscript(custom metaclass diagnostic)", lambda: [1][NoNumericClass])

# Direct and inherited overrides on scalar builtin subclasses take precedence
# over their primitive backing. complex() uses __complex__ for its first
# operand and __float__ for a real-valued second operand.
show("int(direct int override)", lambda: int(DirectInt(2)))
show("int(inherited float override)", lambda: int(InheritedFloat(2.5)))
show("int(direct complex override)", lambda: int(DirectComplex(2, 3)))
show("float(direct int override)", lambda: float(DirectInt(2)))
show("float(inherited float override)", lambda: float(InheritedFloat(2.5)))
show("float(direct complex override)", lambda: float(DirectComplex(2, 3)))
show("complex(direct int override)", lambda: complex(DirectInt(2)))
show("complex(inherited float override)", lambda: complex(InheritedFloat(2.5)))
show("complex(direct complex override)", lambda: complex(DirectComplex(2, 3)))
show("complex(first inherited int override)", lambda: complex(InheritedInt(2), 10))
show("complex(first direct float override)", lambda: complex(DirectFloat(2.5), 10))
show(
    "complex(first inherited complex override)",
    lambda: complex(InheritedComplex(2, 3), 10),
)
show("complex(second direct int override)", lambda: complex(10, DirectInt(2)))
show("complex(second inherited float override)", lambda: complex(10, InheritedFloat(2.5)))
show(
    "complex(second direct complex backing)",
    lambda: complex(10, DirectComplex(2, 3)),
)

# Canonical inherited slots retain the O(1) backing path, while a lower
# __index__ override is not consulted when __int__/__float__ already applies.
show("int(plain int subclass)", lambda: int(PlainInt(7)))
show("int(plain float subclass)", lambda: int(PlainFloat(7.75)))
show("float(plain int subclass)", lambda: float(PlainInt(8)))
show("float(plain float subclass)", lambda: float(PlainFloat(8.5)))
show("complex(plain int subclass)", lambda: complex(PlainInt(9)))
show("complex(plain complex subclass)", lambda: complex(PlainComplex(2, 3)))
show("complex(first plain complex subclass)", lambda: complex(PlainComplex(2, 3), 10))
show("complex(second plain complex subclass)", lambda: complex(10, PlainComplex(2, 3)))
show("int(lower index ignored)", lambda: int(IndexOverrideInt(5)))
show("float(lower index ignored)", lambda: float(IndexOverrideInt(5)))
show("complex(lower index ignored)", lambda: complex(IndexOverrideInt(5)))
show("complex(second lower index ignored)", lambda: complex(1, IndexOverrideInt(5)))

# A user-owned non-callable or wrong-result slot blocks lower fallbacks.
# Copied builtin descriptors are validated against the original receiver so
# their error retains the concrete subclass name.
show("int(noncallable override)", lambda: int(IntSlotNone(5)))
show("float(noncallable override)", lambda: float(FloatSlotNone(5.0)))
show("complex(noncallable first override)", lambda: complex(ComplexSlotsNone(2, 3)))
show(
    "complex(noncallable first override two args)",
    lambda: complex(ComplexSlotsNone(2, 3), 10),
)
show(
    "complex(second ignores all overrides)",
    lambda: complex(10, ComplexSlotsNone(2, 3)),
)
show("int(bad override result)", lambda: int(BadIntOverride(5)))
show("float(bad override result)", lambda: float(BadFloatOverride(5.0)))
show("complex(bad override result)", lambda: complex(BadComplexOverride(2, 3)))
show("int(copied descriptor receiver)", lambda: int(FloatCopiedInt(2.5)))
show(
    "int(inherited copied descriptor receiver)",
    lambda: int(InheritedFloatCopiedInt(2.5)),
)

# Invalid results retain their original subclass name; raised exceptions are
# never mistaken for a missing optional index slot.
show(
    "bad index subclass result",
    lambda: float(IndexValue(BadIndexResult(2.0), "bad-index")),
)
show("raising index", lambda: bytes(RaisingIndex()))

print("events =", events)
