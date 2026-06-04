# The 3-arg type() constructor runs the same class-creation hooks as a `class`
# statement (issues #2130 / #2129): __module__, __init_subclass__, __set_name__,
# and __slots__ enforcement.

# --- __module__ is set and shows in repr ---
X = type('X', (), {})
print(repr(X))
print(X.__module__)

# --- A namespace that supplies __module__ keeps it ---
Y = type('Y', (), {'__module__': 'custom.mod'})
print(Y.__module__)

# --- __init_subclass__ on the base is called ---
log = []
class Base:
    def __init_subclass__(cls, **kw):
        log.append((cls.__name__, sorted(kw.items())))
type('Sub', (Base,), {})
print(log)

# --- __init_subclass__ receives forwarded keyword arguments ---
log.clear()
type('SubKw', (Base,), {}, color='red')
print(log)

# --- __set_name__ on descriptors in the namespace is called ---
names = []
class Desc:
    def __set_name__(self, owner, name):
        names.append((owner.__name__, name))
type('T', (), {'d': Desc(), 'e': Desc()})
print(names)

# --- __slots__ enforcement (issue #2129) ---
Slotted = type('Slotted', (), {'__slots__': ('a',)})
o = Slotted()
o.a = 1
print(o.a)
try:
    o.zzz = 2
    print("FAIL: undeclared attr allowed")
except AttributeError:
    print("ok: __slots__ enforced via type()")

# --- __slots__ / class-variable conflict still raises (issue #1971) ---
try:
    type('Conflict', (), {'__slots__': ('a',), 'a': 1})
    print("FAIL: no conflict error")
except ValueError as e:
    print("ok conflict:", e)

# --- type() result matches an equivalent class statement ---
class Z:
    __slots__ = ('a',)
Zt = type('Z', (), {'__slots__': ('a',)})
zi = Zt()
zi.a = 5
print(zi.a)
try:
    zi.other = 1
    print("FAIL")
except AttributeError:
    print("ok: parity with class statement")

print("type hooks OK")
