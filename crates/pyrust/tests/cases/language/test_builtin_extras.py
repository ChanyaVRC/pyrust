# Parity tests for repr, any, all, map, filter, callable

# ── repr ─────────────────────────────────────────────────────────────────────
print("repr-int", repr(42))
print("repr-neg", repr(-7))
print("repr-float", repr(3.14))
print("repr-str", repr("hello"))
print("repr-bool-true", repr(True))
print("repr-bool-false", repr(False))
print("repr-none", repr(None))
print("repr-list", repr([1, 2, 3]))
print("repr-tuple", repr((1, 2)))
print("repr-empty-list", repr([]))

class MyObj:
    def __repr__(self):
        return "MyObj()"

obj = MyObj()
print("repr-user-repr", repr(obj))

class NoRepr:
    pass

# No __repr__ defined — falls back to built-in "<ClassName object>" style
r = repr(NoRepr())
print("repr-no-repr-contains-NoRepr", "NoRepr" in r)

# ── any ──────────────────────────────────────────────────────────────────────
print("any-empty", any([]))
print("any-all-false", any([False, 0, ""]))
print("any-one-true", any([False, 0, 1]))
print("any-all-true", any([1, 2, 3]))

# ── all ──────────────────────────────────────────────────────────────────────
print("all-empty", all([]))
print("all-all-true", all([1, True, "x"]))
print("all-one-false", all([1, 0, 3]))
print("all-all-false", all([False, 0]))

# ── map ──────────────────────────────────────────────────────────────────────
# Use list() to materialise the iterator so output matches between CPython and PyRust
print("map-square", list(map(lambda x: x * x, [1, 2, 3, 4])))
print("map-str", list(map(str, [1, 2, 3])))
print("map-empty", list(map(lambda x: x, [])))

def double(x):
    return x * 2

print("map-named-fn", list(map(double, [5, 10, 15])))

# ── filter ───────────────────────────────────────────────────────────────────
print("filter-even", list(filter(lambda x: x % 2 == 0, [1, 2, 3, 4, 5, 6])))
print("filter-none", list(filter(None, [0, 1, "", "hi", False, True])))
print("filter-empty", list(filter(lambda x: x, [])))

def is_positive(x):
    return x > 0

print("filter-positive", list(filter(is_positive, [-1, 0, 2, 3, -5])))

# ── callable ─────────────────────────────────────────────────────────────────
print("callable-lambda", callable(lambda: None))
print("callable-fn", callable(double))
print("callable-builtin", callable(len))
print("callable-class", callable(MyObj))
print("callable-int", callable(42))
print("callable-str", callable("hello"))
print("callable-none", callable(None))
print("callable-list", callable([1, 2]))

m = MyObj()
print("callable-instance", callable(m))

class Callable:
    def __call__(self):
        return 0

# bound method is callable
c = Callable()
# __call__ is a user function; get it as a bound method via getattr
method = getattr(c, "__call__")
print("callable-bound-method", callable(method))
