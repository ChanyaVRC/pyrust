# Tests for __slots__ enforcement (issue #1106)

# Basic slot enforcement: declared slot works, undeclared raises AttributeError
class Foo:
    __slots__ = ('x', 'y')

f = Foo()
f.x = 1
f.y = 2
print(f.x, f.y)

try:
    f.z = 3
    print("FAIL: should raise AttributeError")
except AttributeError as e:
    print("ok: undeclared slot blocked:", e)

# Empty slots: no attributes allowed
class Empty:
    __slots__ = ()

e = Empty()
try:
    e.x = 1
    print("FAIL: should raise AttributeError")
except AttributeError as e2:
    print("ok: empty slots blocks all attrs:", e2)

# __dict__ in __slots__: arbitrary attributes allowed
class WithDict:
    __slots__ = ('x', '__dict__')

wd = WithDict()
wd.x = 10
wd.arbitrary = 99
print(wd.x, wd.arbitrary)

# Tuple-style slots
class Point:
    __slots__ = ('x', 'y')

p = Point()
p.x = 3
p.y = 4
print(p.x, p.y)
try:
    p.z = 0
    print("FAIL: should raise AttributeError")
except AttributeError:
    print("ok: z blocked on Point")

# Single string slot
class Single:
    __slots__ = 'value'

s = Single()
s.value = 42
print(s.value)
try:
    s.other = 1
    print("FAIL: should raise AttributeError")
except AttributeError:
    print("ok: non-slot blocked on Single")
