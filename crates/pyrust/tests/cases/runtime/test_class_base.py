# Parity fixture for issue #1969: type.__base__ and object.__bases__.

class A:
    pass

class B(A):
    pass

# __base__ is the single primary base.
print(A.__base__)        # <class 'object'>
print(B.__base__)        # <class '__main__.A'>
print(object.__base__)   # None
print(int.__base__)      # <class 'object'>
print(bool.__base__)     # <class 'int'>

# __base__ for multiple inheritance is the first declared base.
class M1:
    pass

class M2:
    pass

class Multi(M1, M2):
    pass

print(Multi.__base__)    # <class '__main__.M1'>

# object has no bases; everything else is unchanged.
print(object.__bases__)  # ()
print(A.__bases__)       # (<class 'object'>,)
print(B.__bases__)       # (<class '__main__.A'>,)
print(int.__bases__)     # (<class 'object'>,)
print(bool.__bases__)    # (<class 'int'>,)
print(Multi.__bases__)   # (<class '__main__.M1'>, <class '__main__.M2'>)

# __base__ == object / A by identity.
print(A.__base__ is object)   # True
print(B.__base__ is A)        # True
print(object.__base__ is None)  # True
