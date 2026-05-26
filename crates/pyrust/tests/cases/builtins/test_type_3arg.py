# Parity fixture for type(name, bases, dict) — 3-argument form.
# Covers multiple inheritance, empty bases, single base, namespace dict,
# 1-arg pass-through, and TypeError messages.

class A:
    def method_a(self):
        return "a"

class B:
    def method_b(self):
        return "b"

# Multiple inheritance: both A and B methods are accessible.
C = type("C", (A, B), {})
c = C()
print(c.method_a())
print(c.method_b())

# isinstance works for all bases.
print(isinstance(c, A))
print(isinstance(c, B))
print(isinstance(c, C))

# MRO order: C, A, B, object.
mro_names = [cls.__name__ for cls in C.__mro__]
print(mro_names)

# Empty bases tuple — inherits from object.
X = type("X", (), {"foo": 1})
print(X.foo)

# Single base with dict namespace.
Y = type("Y", (A,), {"bar": 2})
y = Y()
print(y.method_a())
print(Y.bar)

# Namespace dict attributes are accessible on instances.
D = type("D", (A, B), {"x": 10})
d = D()
print(d.x)
print(d.method_b())

# 1-argument form: type(obj) → type of obj.
print(type(42).__name__)
print(type("hello").__name__)
print(type([]).__name__)

# TypeError: argument 1 must be str.
try:
    type(42, (), {})
except TypeError as e:
    print(e)

# TypeError: argument 2 must be tuple.
try:
    type("bad", "notuple", {})
except TypeError as e:
    print(e)

# TypeError: argument 3 must be dict.
try:
    type("bad", (A,), [])
except TypeError as e:
    print(e)
