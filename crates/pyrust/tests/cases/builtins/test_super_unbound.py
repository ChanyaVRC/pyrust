# One-argument super(cls) returns an *unbound* super object that acts as a
# descriptor (issue #2704). Binding it via __get__ produces a concrete super.

class A:
    x = 10
    def greet(self):
        return "A"

class B(A):
    pass


# --- super(cls) constructs an unbound super ---
s = super(B)
print(type(s).__name__)        # super
print(repr(s))                 # <super: <class 'B'>, NULL>

# --- introspection attributes on the unbound super ---
print(s.__thisclass__ is B)    # True
print(s.__self__)              # None
print(s.__self_class__)        # None

# --- binding to an instance via __get__ ---
obj = B()
bs = s.__get__(obj, B)
print(type(bs).__name__)       # super
print(bs.greet())              # A
print(bs.x)                    # 10
print(bs.__thisclass__ is B)   # True
print(bs.__self__ is obj)      # True
print(bs.__self_class__ is B)  # True

# --- binding to None returns an unbound super ---
u = s.__get__(None, B)
print(type(u).__name__)        # super
print(u.__self__)              # None

# --- the two-argument form is unchanged and also exposes introspection ---
s2 = super(B, obj)
print(s2.greet())              # A
print(s2.__thisclass__ is B)   # True
print(s2.__self__ is obj)      # True
print(s2.__self_class__ is B)  # True

# --- error parity ---
try:
    super(5)
except TypeError as e:
    print(e)                   # super() argument 1 must be a type, not int

class C:
    pass

try:
    super(A).__get__(C(), C)
except TypeError as e:
    print(e)                   # super(type, obj): obj must be an instance or subtype of type
