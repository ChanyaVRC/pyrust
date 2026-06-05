# Issue #2215: reflected numeric/bitwise dunders are exposed as bound
# method-wrappers on primitive int/float/complex/bool instances, with
# swapped-operand semantics (a.__rOP__(b) computes b OP a) and the same
# tower-rank NotImplemented gating the forward slots use.

# Presence on int.
print(hasattr(5, "__radd__"))
print(hasattr(5, "__rdivmod__"))

# int reflected ops (swapped operands).
print((5).__radd__(3))        # 3 + 5
print((5).__rsub__(10))       # 10 - 5
print((5).__rmul__(3))        # 3 * 5
print((2).__rpow__(3))        # 3 ** 2
print((5).__rtruediv__(10))   # 10 / 5
print((5).__rfloordiv__(13))  # 13 // 5
print((5).__rmod__(13))       # 13 % 5
print((5).__rand__(3))        # 3 & 5
print((5).__ror__(2))         # 2 | 5
print((5).__rxor__(3))        # 3 ^ 5
print((1).__rlshift__(3))     # 3 << 1
print((1).__rrshift__(8))     # 8 >> 1
print((5).__rdivmod__(13))    # divmod(13, 5)

# Bound method-wrapper can be detached and called later.
m = (5).__rsub__
print(m(10))

# bool is an int subclass: same reflected set.
print(True.__radd__(5))

# float: arithmetic reflected ops, no bitwise/shift.
print((5.0).__radd__(3))
print((5.0).__rsub__(3))
print((5.0).__rtruediv__(10))
print((5.0).__rdivmod__(13))
print(hasattr(5.0, "__rand__"))
print(hasattr(5.0, "__rlshift__"))

# complex: add/sub/mul/truediv/pow only.
print(complex(1, 2).__radd__(3))
print(complex(1, 2).__rsub__(3))
print(complex(1, 2).__rtruediv__(4))
print(hasattr(complex(1, 2), "__rfloordiv__"))
print(hasattr(complex(1, 2), "__rmod__"))
print(hasattr(complex(1, 2), "__rdivmod__"))
print(hasattr(complex(1, 2), "__rand__"))

# NotImplemented gating: operand outranks receiver -> NotImplemented.
print((5).__radd__(2.5))
print((5).__rpow__(2.5))
print((5).__radd__(complex(1, 2)))
print((5.0).__radd__(complex(1, 2)))
print((5).__rdivmod__(2.5))

# Big-int operands flow through.
print((5).__radd__(10 ** 30))

# Reflected error propagation: 5 % 0 raises.
try:
    (0).__rmod__(5)
except ZeroDivisionError as e:
    print("ZeroDivisionError", e)
