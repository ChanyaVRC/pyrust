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

# id: same object → same id; distinct objects → different id
x = [1, 2]
y = x
print("id-same", id(x) == id(y))
a = [1]
b = [2]
print("id-list-diff", id(a) != id(b))

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
