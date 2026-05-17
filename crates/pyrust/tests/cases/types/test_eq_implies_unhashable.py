# CPython rule: a class that defines __eq__ without an explicit __hash__
# gets __hash__ = None implicitly, making instances unhashable (issue #501).

# 1. Basic case: __eq__ defined, __hash__ absent -> unhashable.
class C:
    def __eq__(self, other):
        return True

try:
    hash(C())
    print("no error")
except TypeError as e:
    print("TypeError:", e)

# 2. type(C()).__hash__ must be None, not missing.
print(type(C()).__hash__ is None)

# 3. Explicit __hash__ wins; instance must be hashable.
class D:
    def __eq__(self, other):
        return True

    def __hash__(self):
        return 42

print(hash(D()))

# 4. Class without __eq__ stays hashable by identity.
class E:
    pass

print(type(hash(E())))

# 5. Subclass inheriting __eq__ from a parent with implicit __hash__=None
# is also unhashable via lookup_class_attr base-chain walk.
class F(C):
    pass

try:
    hash(F())
    print("no error")
except TypeError as e:
    print("TypeError:", e)
