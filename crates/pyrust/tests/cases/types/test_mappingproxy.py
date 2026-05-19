# Parity fixture for vars(SomeClass) returning a mappingproxy.
# Issue #661: vars(SomeClass) must return a read-only mappingproxy, not a
# plain mutable dict.  CPython 3.12 reference.


class Foo:
    x = 1
    y = "hello"


v = vars(Foo)

# type(vars(Foo)) is <class 'mappingproxy'>
print(type(v).__name__)

# Read access via subscript
print(v["x"])
print(v["y"])

# Contains check
print("x" in v)
print("z" in v)

# Iteration yields keys (strings)
keys = sorted(k for k in v if not k.startswith("__"))
print(keys)

# keys() / values() / items() methods
proxy_keys = sorted(k for k in v.keys() if not k.startswith("__"))
print(proxy_keys)

# get() with and without default
print(v.get("x"))
print(v.get("missing", 99))
print(v.get("missing"))

# copy() returns a plain dict
c = v.copy()
print(type(c).__name__)
# Mutating the copy does not affect the proxy
c["z"] = 42
print("z" in v)

# Live view: mutating the class is reflected
Foo.z = 999
print("z" in v)
print(v["z"])

# Mutation via subscript raises TypeError
try:
    v["new"] = 2
except TypeError as e:
    print("TypeError:", "item assignment" in str(e) or "does not support" in str(e))

# Deletion raises TypeError
try:
    del v["x"]
except TypeError as e:
    print("TypeError:", "item deletion" in str(e) or "does not support" in str(e))

# Subclasses have independent proxies
class Bar(Foo):
    a = 10


vb = vars(Bar)
print("a" in vb)
print("x" in vb)  # inherited attr not in own dict

# repr starts with "mappingproxy"
print(repr(v).startswith("mappingproxy("))
