# isinstance, type, id, hasattr, getattr, setattr — Issue #98

# isinstance
print("isinstance-int", isinstance(5, int))
print("isinstance-float", isinstance(5.0, float))
print("isinstance-str", isinstance("x", str))
print("isinstance-bool", isinstance(True, bool))
print("isinstance-bool-is-int", isinstance(True, int))
print("isinstance-false", isinstance("x", int))
print("isinstance-list", isinstance([], list))
print("isinstance-tuple", isinstance((), tuple))
print("isinstance-set", isinstance({1}, set))
print("isinstance-dict", isinstance({}, dict))

# type().__name__
print("type-int", type(5).__name__)
print("type-float", type(5.0).__name__)
print("type-str", type("x").__name__)
print("type-list", type([]).__name__)
print("type-bool", type(True).__name__)

# type() is singleton (same object returned each call)
print("type-is-int", type(5) is type(5))
print("type-is-str", type("x") is type("x"))

# isinstance(x, type(x)) round-trip
print("isinstance-type-int", isinstance(5, type(5)))
print("isinstance-type-str", isinstance("x", type("x")))
print("isinstance-type-list", isinstance([], type([])))

# id: same object → same id; distinct objects → different id
x = [1, 2]
y = x
print("id-same", id(x) == id(y))
a = [1]
b = [2]
print("id-list-diff", id(a) != id(b))
s1 = "hello"
s2 = "world"
print("id-str-diff", id(s1) != id(s2))
t1 = (1,)
t2 = (2,)
print("id-tuple-diff", id(t1) != id(t2))

# hasattr / getattr / setattr on a class instance
class Foo:
    pass

f = Foo()
g = Foo()
print("id-diff", id(f) != id(g))
f.x = 1
print("hasattr-true", hasattr(f, "x"))
print("hasattr-false", hasattr(f, "y"))
print("getattr-hit", getattr(f, "x"))
print("getattr-default", getattr(f, "y", 99))
setattr(f, "z", "hello")
print("setattr-read", f.z)

# setattr with non-string attribute name must raise TypeError
try:
    setattr(f, 123, "val")
    print("setattr-nonstr-noerr")
except TypeError:
    print("setattr-nonstr-typeerror", True)
