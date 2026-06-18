# super() must invoke the descriptor protocol (__get__) on attributes
# resolved through a base class, like CPython's super.__getattribute__.


# property chained through super()
class A:
    @property
    def val(self):
        return 42


class B(A):
    def get_val(self):
        return super().val


b = B()
print(b.get_val())
print(super(B, b).val)


# overriding a property and chaining to the base via super()
class P:
    @property
    def val(self):
        return 10


class Q(P):
    @property
    def val(self):
        return super().val + 5


print(Q().val)


# custom non-data descriptor through super()
class Desc:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return obj._x * 2


class Base:
    d = Desc()


class Sub(Base):
    def __init__(self):
        self._x = 10

    def get_d(self):
        return super().d


s = Sub()
print(s.get_d())
print(super(Sub, s).d)


# data descriptor through super()
class DataDesc:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return "data-" + str(obj._y)

    def __set__(self, obj, value):
        obj._y = value


class DBase:
    dd = DataDesc()


class DSub(DBase):
    def __init__(self):
        self._y = 7

    def get_dd(self):
        return super().dd


d = DSub()
print(d.get_dd())


# classmethod super() resolving a descriptor on the class itself works too;
# plain (non-descriptor) attribute through super() is returned unchanged.
class Plain:
    x = 99


class PlainSub(Plain):
    def get_x(self):
        return super().x


print(PlainSub().get_x())
