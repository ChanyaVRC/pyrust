# Issue #1942: `instance.__dict__ = {...}` replaces the instance attribute
# dict wholesale instead of storing a literal "__dict__" attribute.


class W:
    pass


# Replace: new keys become attributes, old attributes are dropped.
w = W()
w.a = 1
w.__dict__ = {"m": 7}
print(w.m)
try:
    w.a
except AttributeError as e:
    print("AttributeError:", e)
print(w.__dict__)

# New attributes added after a replace coexist with the replaced keys.
w.b = 99
print(w.b, w.m)

# In-place mutation of the dict proxy still works.
w2 = W()
w2.__dict__["k"] = 9
print(w2.k)

# Assigning a non-dict raises TypeError with the CPython message/type-name.
for bad in (5, [1, 2], (1,), "s", 1.5, None, True, {1, 2}):
    w3 = W()
    try:
        w3.__dict__ = bad
    except TypeError as e:
        print("TypeError:", e)

# A class instance as the value also reports its class name in the message.
w3 = W()
try:
    w3.__dict__ = W()
except TypeError as e:
    print("TypeError:", e)

# Setting a normal attribute after the bad assignments still works.
w3.x = 5
print(w3.x)

# object.__setattr__ routes through the same replacement path.
w4 = W()
object.__setattr__(w4, "__dict__", {"p": 11})
print(w4.p)
