# property exposes the descriptor protocol (__get__/__set__/__delete__) as
# introspectable, callable attributes on both the type and its instances.

# Type object exposes the descriptor dunders.
print(hasattr(property, "__get__"))
print(hasattr(property, "__set__"))
print(hasattr(property, "__delete__"))

# Instances expose them too.
p = property(lambda self: 42)
print(hasattr(p, "__get__"))
print(hasattr(p, "__set__"))
print(hasattr(p, "__delete__"))


class C:
    pass


obj = C()

# __get__ on an instance calls the getter.
print(p.__get__(obj, C))

# __get__ with obj=None (class-level access) returns the property itself.
print(p.__get__(None, C) is p)

# __set__ / __delete__ on a property with no setter/deleter raise
# AttributeError.  A plain property() carries no __set_name__ name, so the
# message uses CPython 3.12's unnamed form on both CPython and pyrust.
try:
    p.__set__(obj, 1)
except AttributeError as e:
    print("set:", e)
try:
    p.__delete__(obj)
except AttributeError as e:
    print("del:", e)

# A property with no getter raises on __get__.
empty = property()
try:
    empty.__get__(obj, C)
except AttributeError as e:
    print("get:", e)

# The bound method-wrapper can be stored then called later.
f = p.__get__
print(f(obj, C))

# A fully-populated property round-trips through the descriptor protocol.
store = {}


def _get(self):
    return store.get("v", "unset")


def _set(self, value):
    store["v"] = value


def _del(self):
    store.pop("v", None)


full = property(_get, _set, _del)
full.__set__(obj, "hello")
print(full.__get__(obj, C))
full.__delete__(obj)
print(full.__get__(obj, C))


# The @property decorator (including setter) still works end to end.
class D:
    def __init__(self):
        self._x = 1

    @property
    def x(self):
        return self._x

    @x.setter
    def x(self, value):
        self._x = value


d = D()
print(d.x)
d.x = 9
print(d.x)
