# Issue #1846: property __get__/__set__/__delete__ AttributeError messages name
# the property using the attribute name recorded via __set_name__ during class
# creation.  Properties never bound in a class body use the unnamed form.


# Decorator form: name comes from the method / attribute name.
class Rect:
    @property
    def area(self):
        return 42


r = Rect()
try:
    r.area = 5
except AttributeError as e:
    print(e)  # property 'area' of 'Rect' object has no setter
try:
    del r.area
except AttributeError as e:
    print(e)  # property 'area' of 'Rect' object has no deleter


# Setter present but no deleter: the name is still reported.
class Account:
    @property
    def balance(self):
        return 100

    @balance.setter
    def balance(self, value):
        pass


a = Account()
a.balance = 50  # ok
try:
    del a.balance
except AttributeError as e:
    print(e)  # property 'balance' of 'Account' object has no deleter


# Getter-less property: reading it names the property too.
class Lazy:
    field = property()


lz = Lazy()
try:
    lz.field
except AttributeError as e:
    print(e)  # property 'field' of 'Lazy' object has no getter


# The reported name follows the assigned attribute name, not the getter's name.
class Widget:
    def _getter(self):
        return 7

    size = property(_getter)


w = Widget()
try:
    w.size = 1
except AttributeError as e:
    print(e)  # property 'size' of 'Widget' object has no setter


# Inherited property reports the runtime subclass as the owner.
class Base:
    @property
    def p(self):
        return 1


class Sub(Base):
    pass


s = Sub()
try:
    s.p = 5
except AttributeError as e:
    print(e)  # property 'p' of 'Sub' object has no setter


# Anonymous property assigned outside a class body never had __set_name__
# called, so it uses the unnamed form for get/set/delete.
class Plain:
    pass


Plain.x = property(lambda self: 42)
plain = Plain()
try:
    plain.x = 1
except AttributeError as e:
    print(e)  # property of 'Plain' object has no setter
try:
    del plain.x
except AttributeError as e:
    print(e)  # property of 'Plain' object has no deleter

Plain.g = property()
try:
    plain.g
except AttributeError as e:
    print(e)  # property of 'Plain' object has no getter


# Direct descriptor-protocol calls report the name the same way.
prop = Rect.__dict__["area"]
try:
    prop.__set__(r, 1)
except AttributeError as e:
    print(e)  # property 'area' of 'Rect' object has no setter
try:
    prop.__delete__(r)
except AttributeError as e:
    print(e)  # property 'area' of 'Rect' object has no deleter

anon = Plain.__dict__["x"]
try:
    anon.__set__(plain, 1)
except AttributeError as e:
    print(e)  # property of 'Plain' object has no setter


# fget remains accessible through the class __dict__.
print(Rect.__dict__["area"].fget is not None)  # True


# Chained assignment in a class body shares one property object; __set_name__
# fires once per attribute, so the last name wins for both (matches CPython).
class Chained:
    a = b = property(lambda self: 1)


c = Chained()
try:
    c.a = 9
except AttributeError as e:
    print(e)  # property 'b' of 'Chained' object has no setter
try:
    c.b = 9
except AttributeError as e:
    print(e)  # property 'b' of 'Chained' object has no setter
