# dict.update() with keyword arguments — issue #1755
# All cases verified against CPython 3.12.

# Basic kwargs only
d = {}
d.update(a=1, b=2)
print(d)

# Kwargs merge into non-empty dict
d = {'x': 0}
d.update(y=1)
print(d)

# Mixed positional mapping + kwargs
d = {}
d.update({'a': 1}, b=2)
print(d)

# No arguments — no change
d = {}
d.update()
print(d)

# kwargs overwrite key from positional mapping
d = {}
d.update({'a': 1}, a=2)
print(d)

# Multiple kwargs
d = {}
d.update(x=1, y=2, z=3)
print(d)

# kwargs with iterable-of-pairs positional arg
d = {}
d.update([('a', 1), ('b', 2)], c=3)
print(d)

# Too many positional args raises TypeError
try:
    d = {}
    d.update({'a': 1}, {'b': 2})
except TypeError as e:
    print(f"TypeError: {e}")

# Subclass inherits the fixed behaviour
class MyDict(dict):
    pass

md = MyDict()
md.update(a=1, b=2)
print(md)
