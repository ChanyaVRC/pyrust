# Parity fixture for object.__setattr__, object.__delattr__,
# object.__getattribute__, and object.__new__ exposed as unbound methods on the
# object type (issue #1402).

# ── hasattr checks ────────────────────────────────────────────────────────────

print(hasattr(object, '__setattr__'))       # True
print(hasattr(object, '__delattr__'))       # True
print(hasattr(object, '__getattribute__'))  # True
print(hasattr(object, '__new__'))           # True

# ── object.__setattr__ ────────────────────────────────────────────────────────

class MyClass:
    def __setattr__(self, name, value):
        if name == 'protected':
            raise AttributeError("read-only")
        object.__setattr__(self, name, value)  # must not recurse infinitely

obj = MyClass()
obj.x = 10
print(obj.x)  # 10

try:
    obj.protected = 99
except AttributeError as e:
    print(e)  # read-only

# Direct call form
class Foo:
    pass

f = Foo()
object.__setattr__(f, 'y', 42)
print(f.y)  # 42

# Non-string name raises TypeError
try:
    object.__setattr__(f, 123, 1)
except TypeError as e:
    print(e)  # attribute name must be string, not 'int'

# Bare object() instance raises AttributeError
obj2 = object()
try:
    object.__setattr__(obj2, 'x', 1)
except AttributeError as e:
    print(e)  # 'object' object has no attribute 'x'

# Error: no args
try:
    object.__setattr__()
except TypeError as e:
    print(type(e).__name__)  # TypeError

# Error: wrong arg count
try:
    object.__setattr__(f)
except TypeError as e:
    print(type(e).__name__)  # TypeError

# ── object.__delattr__ ────────────────────────────────────────────────────────

class Bar:
    def __delattr__(self, name):
        if name == 'locked':
            raise AttributeError("cannot delete")
        object.__delattr__(self, name)  # must not recurse infinitely

b = Bar()
b.z = 99
del b.z
print(hasattr(b, 'z'))  # False

# Non-existent attribute raises AttributeError
try:
    object.__delattr__(b, 'nonexistent')
except AttributeError as e:
    print(e)  # 'Bar' object has no attribute 'nonexistent'

# Non-string name raises TypeError
try:
    object.__delattr__(b, 123)
except TypeError as e:
    print(e)  # attribute name must be string, not 'int'

# Error: no args
try:
    object.__delattr__()
except TypeError as e:
    print(type(e).__name__)  # TypeError

# Error: wrong arg count
try:
    object.__delattr__(b)
except TypeError as e:
    print(type(e).__name__)  # TypeError

# ── object.__getattribute__ ───────────────────────────────────────────────────

class Baz:
    def __getattribute__(self, name):
        if name == 'secret':
            return 'intercepted'
        return object.__getattribute__(self, name)

baz = Baz()
baz.val = 7
print(baz.val)     # 7
print(baz.secret)  # intercepted

# ── object.__new__ ────────────────────────────────────────────────────────────

class Qux:
    pass

q = object.__new__(Qux)
print(type(q).__name__)  # Qux
print(isinstance(q, Qux))  # True

# ── descriptor protocol via object.__setattr__ ────────────────────────────────

class Desc:
    def __set__(self, obj, value):
        obj.__dict__['_d'] = value * 2

class WithDesc:
    d = Desc()
    def __setattr__(self, name, value):
        object.__setattr__(self, name, value)

wd = WithDesc()
wd.d = 5
print(wd.__dict__.get('_d'))  # 10
