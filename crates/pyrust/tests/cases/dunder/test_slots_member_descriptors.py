# Issue #2084: `__slots__` installs a `member_descriptor` per slot on the class.
class S:
    __slots__ = ("a", "b")


# The slot is a member_descriptor on the class, in dir() and __dict__.
print(type(S.a).__name__)        # member_descriptor
print(repr(S.a))                 # <member 'a' of 'S' objects>
print("a" in dir(S))             # True
print("a" in S.__dict__)         # True
print(S.a is S.a)                # True (same object stored in the class dict)

s = S()
# Reading an unset slot raises AttributeError via the descriptor.
try:
    s.a
except AttributeError as e:
    print(e)                     # 'S' object has no attribute 'a'

# get / set / delete through ordinary attribute access.
s.a = 10
print(s.a)                       # 10
del s.a
try:
    s.a
except AttributeError as e:
    print(e)

# The descriptor protocol methods are directly callable.
s.a = 99
print(S.a.__get__(s, S))         # 99
print(S.a.__get__(None, S) is S.a)  # True (class-level access)
S.a.__set__(s, 7)
print(s.a)                       # 7
S.a.__delete__(s)
print(hasattr(s, "a"))           # False
try:
    S.a.__delete__(s)
except AttributeError as e:
    print(e)                     # a  (CPython member_delete message is the slot name)

# Applying the descriptor to a wrong-type object raises TypeError.
try:
    S.a.__get__(5)
except TypeError as e:
    print(e)  # descriptor 'a' for 'S' objects doesn't apply to a 'int' object


# An instance of an *unrelated* class is also the wrong type: get / set / delete
# must all raise TypeError (in particular __set__ must not write into it).
class W:
    pass


w = W()
for m, extra in (("__get__", ()), ("__set__", (1,)), ("__delete__", ())):
    try:
        getattr(S.a, m)(w, *extra)
    except TypeError as e:
        print(e)  # descriptor 'a' for 'S' objects doesn't apply to a 'W' object

# Inheritance: a subclass adding slots gets its own member_descriptors and the
# union of all slots is enforced.
class A:
    __slots__ = ("p",)


class B(A):
    __slots__ = ("q",)


print(type(B.q).__name__)        # member_descriptor
print("p" in dir(B), "q" in dir(B))  # True True
b = B()
b.p = 1
b.q = 2
print(b.p, b.q)                  # 1 2
try:
    b.r = 3
except AttributeError as e:
    print(e)                     # 'B' object has no attribute 'r'

# Empty __slots__ creates no descriptors but still blocks attributes.
class E:
    __slots__ = ()


print(dir(E).count("__dict__"))  # 0  (no __dict__ descriptor on the class)
e = E()
try:
    e.z = 1
except AttributeError as ex:
    print(ex)                    # 'E' object has no attribute 'z'
