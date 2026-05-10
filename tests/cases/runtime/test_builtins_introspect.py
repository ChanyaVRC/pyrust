# isinstance, type, id, hasattr, getattr, setattr — Issue #98

# isinstance
print("isinstance-int", isinstance(5, int))
print("isinstance-float", isinstance(5.0, float))
print("isinstance-str", isinstance("x", str))
print("isinstance-bool", isinstance(True, bool))
print("isinstance-bool-is-int", isinstance(True, int))
print("isinstance-false", isinstance("x", int))

# type().__name__
print("type-int", type(5).__name__)
print("type-float", type(5.0).__name__)
print("type-str", type("x").__name__)
print("type-list", type([]).__name__)
print("type-bool", type(True).__name__)

# id: same object → same id
x = [1, 2]
y = x
print("id-same", id(x) == id(y))

# hasattr / getattr / setattr on a class instance
class Foo:
    pass

f = Foo()
f.x = 1
print("hasattr-true", hasattr(f, "x"))
print("hasattr-false", hasattr(f, "y"))
print("getattr-hit", getattr(f, "x"))
print("getattr-default", getattr(f, "y", 99))
setattr(f, "z", "hello")
print("setattr-read", f.z)
