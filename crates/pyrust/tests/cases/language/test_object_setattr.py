# Parity fixture: bare object() instances reject attribute writes, matching
# CPython's behaviour where object.__dict__ is absent on raw instances.

# Write on bare object() raises AttributeError
obj = object()
try:
    obj.x = 1
    print("FAIL: expected AttributeError")
except AttributeError as e:
    print(e)

# Read on bare object() also raises AttributeError (pre-existing behaviour)
obj2 = object()
try:
    _ = obj2.nonexistent
    print("FAIL: expected AttributeError")
except AttributeError as e:
    print(e)

# hasattr returns False when the attribute would raise
print(hasattr(object(), "value"))

# User-defined class (no explicit base) still allows attribute writes
class Foo:
    pass

f = Foo()
f.answer = 42
print(f.answer)

# User-defined class with explicit object base still allows attribute writes
class Bar(object):
    pass

b = Bar()
b.data = "hello"
print(b.data)

# Multi-level inheritance: grandchild of object still works
class Grandchild(Bar):
    pass

g = Grandchild()
g.extra = True
print(g.extra)
