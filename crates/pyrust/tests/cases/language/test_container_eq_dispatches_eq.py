# Container equality must dispatch element `__eq__` on PyInstance
# elements (issue #436).  Before the fix, `[a] == [b]` would call
# `Value::PartialEq::eq`, which uses `Rc::ptr_eq` for `PyInstance`,
# silently returning False whenever a user class defined `__eq__`.


class A:
    def __eq__(self, other):
        return True

    def __hash__(self):
        return 1


a, b = A(), A()

# Single-element containers — the headline regressions from the issue.
print(a == b)                            # True
print([a] == [b])                        # True
print((a,) == (b,))                      # True
print({a: 1} == {b: 1})                  # True
print({a} == {b})                        # True

# Multi-element with a primitive mixed in — element-wise recursion
# must dispatch __eq__ on element 0 AND match primitives on element 1.
print([a, 1] == [b, 1])                  # True
print((a, 1) == (b, 1))                  # True
print({a: 1, b: 2} == {b: 1, a: 2})      # True

# Nested containers — recursion must reach the inner PyInstance.
print([[a]] == [[b]])                    # True
print([[a, 1]] == [[b, 1]])              # True
print([(a, 1)] == [(b, 1)])              # True
print([{a: 1}] == [{b: 1}])              # True

# __eq__ returning False (e.g. always-unequal).
class N:
    def __eq__(self, other):
        return False

    def __hash__(self):
        return 1


n1, n2 = N(), N()
print([n1] == [n2])                      # False
print((n1,) == (n2,))                    # False

# `__eq__` returning NotImplemented falls back to identity (False for
# distinct instances; True for the same instance).
class NI:
    def __eq__(self, other):
        return NotImplemented


x = NI()
y = NI()
print([x] == [y])                        # False
print([x] == [x])                        # True

# Primitive-only containers — the fast `Value::eq` path must still work.
print([1, 2, 3] == [1, 2, 3])            # True
print((1, 2, 3) == (1, 2, 3))            # True
print({1: 'a', 2: 'b'} == {1: 'a', 2: 'b'})  # True
print({1, 2, 3} == {3, 2, 1})            # True

# Numeric coercion preserved.
print([1.0] == [1])                      # True
print([True] == [1])                     # True
print((1, 2.0, 3) == (1, 2, 3))          # True

# Length / shape mismatches.
print([a] == [b, b])                     # False
print((a, b) == (a,))                    # False
print({a: 1} == {a: 1, b: 2})            # False
print([a] == (a,))                       # False  list vs tuple

# Ne path mirrors Eq.
print([a] != [b])                        # False
print([n1] != [n2])                      # True
print([a, 1] != [b, 2])                  # True

# Dict-value comparison must also dispatch __eq__ on user-class values.
print({1: a} == {1: b})                  # True
print({1: n1} == {1: n2})                # False
