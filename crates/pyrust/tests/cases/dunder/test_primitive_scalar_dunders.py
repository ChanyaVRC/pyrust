# Issue #2070: rich-comparison, numeric, bitwise, `__hash__`, `__neg__`,
# `__str__`, `__repr__`, `__bool__` dunders are exposed as bound method-wrappers
# on primitive instances (int/float/complex/bool/str/bytes/tuple/list/dict/set/
# frozenset).  Forward slots return `NotImplemented` for operand types they do
# not accept, exactly as CPython does.


# --- rich comparison ---------------------------------------------------------
print((1).__lt__(2))
print((1).__le__(1))
print((1).__gt__(2))
print((1).__ge__(2))
print((1).__eq__(1))
print((1).__ne__(2))
print((1).__eq__("x"))          # NotImplemented (non-numeric operand)
print((1).__eq__(1.0))          # NotImplemented (int slot rejects float)
print((1.0).__eq__(1))          # True (float slot accepts int)
print((1).__lt__(1.0))          # NotImplemented
print((1.0).__lt__(2))          # True
print("a".__lt__("b"))
print("a".__eq__("a"))
print("a".__eq__(5))            # NotImplemented
print(b"a".__lt__(b"b"))
print((1, 2).__lt__((1, 3)))
print((1,).__lt__([1]))          # NotImplemented (tuple vs list)
print([1].__lt__([2]))
print({1}.__lt__({1, 2}))
print({1}.__eq__(frozenset({1})))   # set/frozenset interchangeable
print(frozenset({1}).__eq__({1}))
print({1: 2}.__eq__({1: 2}))
print({1: 2}.__lt__({1: 3}))        # NotImplemented (dict has no ordering)
print((1j).__lt__(2j))              # NotImplemented (complex has no ordering)
print((1j).__eq__(1j))

# --- numeric / bitwise -------------------------------------------------------
print((1).__add__(2))
print((1).__sub__(3))
print((2).__mul__(4))
print((5).__truediv__(2))
print((5).__floordiv__(2))
print((5).__mod__(3))
print((2).__pow__(10))
print((5).__divmod__(2))
print((5).__and__(3), (5).__or__(2), (5).__xor__(1))
print((5).__lshift__(2), (5).__rshift__(1), (5).__invert__())
print((1).__add__(2.0))         # NotImplemented (int slot rejects float)
print((1.0).__add__(2))         # 3.0 (float slot accepts int)
print((1.5).__add__(0.5))
print((2 + 3j).__add__(1j))
print(True.__add__(1), True.__and__(False))

# --- unary -------------------------------------------------------------------
print((5).__neg__())
print((-5).__abs__())
print((5).__pos__())

# --- hash / str / repr / bool ------------------------------------------------
print((5).__hash__())
print("a".__hash__() == hash("a"))
print(frozenset({1}).__hash__() == hash(frozenset({1})))
print((5).__str__(), (5).__repr__())
print("a".__str__(), "a".__repr__())
print((5).__bool__(), (0).__bool__())

# --- unhashable types: __hash__ is None --------------------------------------
print([1].__hash__)
print({1: 2}.__hash__)
try:
    [1].__hash__()
except TypeError as e:
    print("TypeError:", e)
