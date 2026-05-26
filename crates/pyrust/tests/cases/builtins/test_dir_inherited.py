"""
dir(instance) includes inherited dunder methods from object and the MRO chain.

Checks that the key names from object's interface appear in dir() for user-defined
class instances, that instance and class attributes are included, that the result
is sorted, and that multi-level and multi-inheritance chains are handled.
"""

# Basic case: user class with no explicit base
class Foo:
    class_var = 1
    def method(self): pass
    def __init__(self): self.x = 2

f = Foo()
d = dir(f)

# Result must be sorted
print(d == sorted(d))

# Instance attrs
print("x" in d)

# Direct class attrs
print("class_var" in d)
print("method" in d)

# Standard object dunders must be present
print("__class__" in d)
print("__dict__" in d)
print("__dir__" in d)
print("__doc__" in d)
print("__eq__" in d)
print("__format__" in d)
print("__hash__" in d)
print("__init__" in d)
print("__ne__" in d)
print("__new__" in d)
print("__repr__" in d)
print("__str__" in d)

# Inherited attrs from base class appear in dir of derived class
class Base:
    base_attr = 10
    def base_method(self): pass

class Derived(Base):
    derived_attr = 20

obj = Derived()
dd = dir(obj)
print("base_attr" in dd)
print("base_method" in dd)
print("derived_attr" in dd)
print("__str__" in dd)

# Multi-level inheritance
class A:
    a = 1

class B(A):
    b = 2

class C(B):
    c = 3

c_inst = C()
dc = dir(c_inst)
print("a" in dc)
print("b" in dc)
print("c" in dc)
print("__repr__" in dc)

# Multiple inheritance
class M1:
    m1 = "one"

class M2:
    m2 = "two"

class Multi(M1, M2):
    pass

m = Multi()
dm = dir(m)
print("m1" in dm)
print("m2" in dm)
print("__hash__" in dm)

# dir(object) itself includes the standard dunders
do = dir(object)
print("__class__" in do)
print("__eq__" in do)
print("__str__" in do)
print("__repr__" in do)
print(do == sorted(do))
