# PEP 487: __set_name__ is called on descriptor class attributes after class
# creation.  CPython 3.12 looks up __set_name__ on type(value) (the class),
# not on the value itself, and calls it with (owner, name).

# Basic usage: descriptor records its attribute name and owner class.
class Descriptor:
    def __set_name__(self, owner, name):
        self.public_name = name
        self.private_name = '_' + name
        self.owner = owner

class MyClass:
    field = Descriptor()

print(MyClass.field.public_name)   # field
print(MyClass.field.private_name)  # _field
print(MyClass.field.owner is MyClass)  # True

# Multiple descriptors in one class body — both are notified.
class D:
    def __set_name__(self, owner, name):
        self.attr_name = name

class C:
    x = D()
    y = D()

print(C.x.attr_name)  # x
print(C.y.attr_name)  # y

# Values without __set_name__ are silently skipped.
class Plain:
    pass

class C2:
    p = Plain()

print("no error")  # no error

# __set_name__ defined only as an instance attribute (not a class attr)
# must NOT be called — CPython requires type(value).__set_name__.
class NotADescriptor:
    pass

obj = NotADescriptor()
obj.__set_name__ = lambda owner, name: print("WRONG: should not be called")

class C3:
    x = obj

print("instance attr not called")

# __set_name__ error propagates as the original exception.
class Bad:
    def __set_name__(self, owner, name):
        raise ValueError("set_name error")

try:
    class C4:
        b = Bad()
except ValueError as e:
    print(type(e).__name__)  # ValueError
    print(str(e))            # set_name error

# __set_name__ sees the fully-constructed class (can access class attrs).
class Recorder:
    def __set_name__(self, owner, name):
        self.name = name
        # The owner class is fully constructed at this point.
        self.owner_name = owner.__name__

class Named:
    r = Recorder()

print(Named.r.name)        # r
print(Named.r.owner_name)  # Named

# __set_name__ runs before __init_subclass__ (PEP 487 ordering guarantee).
set_name_order = []

class OrderDesc:
    def __set_name__(self, owner, name):
        set_name_order.append("set_name:" + name)

class OrderBase:
    def __init_subclass__(cls, **kw):
        set_name_order.append("init_subclass:" + cls.__name__)

class OrderSub(OrderBase):
    x = OrderDesc()

print(set_name_order)  # ['set_name:x', 'init_subclass:OrderSub']

# Inherited __set_name__ works (descriptor subclass that doesn't override it).
class BaseDesc:
    def __set_name__(self, owner, name):
        self.name = name

class SubDesc(BaseDesc):
    pass  # inherits __set_name__ from BaseDesc

class C5:
    x = SubDesc()

print(C5.x.name)  # x
