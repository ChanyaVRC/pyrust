# super(...).__self_class__ in the metaclass branch (issue #2712).
#
# CPython's supercheck sets `obj_type` (exposed as __self_class__) differently
# for the two class-bound forms:
#   * standard classmethod case super(Base, Derived) where Derived is a
#     *subclass* of Base  -> obj_type == Derived
#   * metaclass case super(Meta, cls) where cls is an *instance* of Meta
#     (Meta is in type(cls)'s MRO, not cls's own MRO) -> obj_type == type(cls)


class A:
    pass


class B(A):
    pass


# --- metaclass branch: super(type, B) ---
y = super(type, B)
print(y.__self__ is B)           # True
print(y.__thisclass__ is type)   # True
print(y.__self_class__ is type)  # True  (type(B) is type, NOT B)

# --- 1-argument unbound form bound via __get__ to a class ---
x = super(type).__get__(B, type)
print(x.__self__ is B)           # True
print(x.__thisclass__ is type)   # True
print(x.__self_class__ is type)  # True

# --- standard classmethod case must be unchanged: super(A, B) ---
z = super(A, B)
print(z.__self__ is B)           # True
print(z.__thisclass__ is A)      # True
print(z.__self_class__ is B)     # True  (B is in B's own MRO)


# --- a user-defined metaclass, accessed via zero-arg super() ---
class Meta(type):
    def describe(cls):
        s = super()  # equivalent to super(Meta, cls)
        return (
            s.__thisclass__.__name__,
            s.__self__.__name__,
            s.__self_class__.__name__,
        )


class C(metaclass=Meta):
    pass


print(C.describe())              # ('Meta', 'C', 'Meta')  (type(C) is Meta)

# Explicit two-argument form on the user metaclass.
sc = super(Meta, C)
print(sc.__self__ is C)          # True
print(sc.__self_class__ is Meta)  # True


# --- method resolution through metaclass super still works (#1956) ---
class M1(type):
    def hi(cls):
        return "M1.hi"


class M2(M1):
    def hi(cls):
        return "M2->" + super().hi()


class D(metaclass=M2):
    pass


print(D.hi())                    # M2->M1.hi
