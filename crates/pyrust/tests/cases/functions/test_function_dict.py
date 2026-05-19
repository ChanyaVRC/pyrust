# Parity fixture for issue #666: arbitrary attribute assignment on user functions
# via __dict__. CPython 3.12 allows f.x = v, f.__dict__, f.__dict__ = {...},
# del f.x, and bound-method delegation.

def foo(): pass

# --- Basic read/write ---
foo.x = 1
print(foo.x)             # 1
print(foo.__dict__)      # {'x': 1}

# --- Multiple attributes maintain insertion order ---
foo.y = 2
foo.z = 3
print(list(foo.__dict__.keys()))   # ['x', 'y', 'z']

# --- Replace __dict__ ---
foo.__dict__ = {'a': 10}
print(foo.__dict__)      # {'a': 10}
# Old attrs are gone
try:
    print(foo.x)
except AttributeError as e:
    print(e)             # 'function' object has no attribute 'x'

# --- del attribute ---
del foo.a
print(foo.__dict__)      # {}

# --- del unknown attribute raises AttributeError ---
try:
    del foo.z
except AttributeError as e:
    print(e)             # 'function' object has no attribute 'z'

# --- __dict__ = non-dict raises TypeError ---
try:
    foo.__dict__ = 42
except TypeError as e:
    print(e)             # __dict__ must be set to a dictionary, not a 'int'

try:
    foo.__dict__ = None
except TypeError as e:
    print(e)             # __dict__ must be set to a dictionary, not a 'NoneType'

# --- del __dict__ raises TypeError ---
try:
    del foo.__dict__
except TypeError as e:
    print(e)             # cannot delete __dict__

# --- __name__ / __qualname__ / __module__ / __doc__ are not in __dict__ ---
print('__name__' in foo.__dict__)       # False
print('__qualname__' in foo.__dict__)   # False

# --- BoundMethod delegates __dict__ to underlying function ---
class Cls:
    def method(self): pass

obj = Cls()
Cls.method.attr = 99
print(Cls.method.attr)        # 99
print(obj.method.attr)        # 99 (bound method delegates)
print(Cls.method.__dict__)    # {'attr': 99}
print(obj.method.__dict__)    # {'attr': 99}

# --- BoundMethod cannot have attrs assigned ---
try:
    obj.method.attr = 0
except AttributeError as e:
    print(e)              # 'method' object has no attribute 'attr'

# --- @classmethod bound method delegates __dict__ to underlying function ---
class Cls2:
    @classmethod
    def cm(cls): pass

print(Cls2.cm.__dict__)       # {} (underlying function attrs are empty)

# --- __dict__ returns a live object (mutations propagate back) ---
def bar(): pass
bar.p = 10
d = bar.__dict__
d['q'] = 20
print(bar.q)                  # 20 — live reference, not a snapshot
bar.__dict__['r'] = 30
print(bar.r)                  # 30 — subscript-assign via returned dict

# --- del __defaults__ / __annotations__ / __kwdefaults__ succeeds ---
def baz(): pass
del baz.__defaults__          # no error
del baz.__annotations__       # no error
del baz.__kwdefaults__        # no error
print('slots cleared ok')     # slots cleared ok
