# Parity fixture for type.mro() method (issue #1234).
# Uses __name__ to print class names to avoid repr() format differences.

# Simple single-inheritance chain
class A:
    pass


class B(A):
    pass


print([c.__name__ for c in B.mro()])
print([c.__name__ for c in A.mro()])

# mro() returns a list, __mro__ returns a tuple
print(type(B.mro()).__name__)
print(type(B.__mro__).__name__)

# mro() and __mro__ contain the same classes
print([c.__name__ for c in B.__mro__])

# Built-in types
print([c.__name__ for c in int.mro()])
print([c.__name__ for c in bool.mro()])
print([c.__name__ for c in str.mro()])
print([c.__name__ for c in float.mro()])

# object is always the last entry
print([c.__name__ for c in object.mro()])

# type(B).mro(B) — unbound descriptor call
print([c.__name__ for c in type(B).mro(B)])
print(type(B).mro(B) == B.mro())

# type.mro(B) — access via type class directly
print([c.__name__ for c in type.mro(B)])

# Error: type.mro() with no args
try:
    type.mro()
except TypeError as e:
    print("TypeError:", e)

# Error: B.mro(extra_arg)
try:
    B.mro(1)
except TypeError as e:
    print("TypeError:", e)
