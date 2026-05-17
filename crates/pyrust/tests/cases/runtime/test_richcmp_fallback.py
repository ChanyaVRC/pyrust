# Parity test for issue #555: richcmp error messages use the correct operator
# name for >, <=, >= (not always '<').
#
# CPython's do_richcompare emits the actual operator token in TypeError:
#   '<' not supported ...    for <
#   '>' not supported ...    for >
#   '<=' not supported ...   for <=
#   '>=' not supported ...   for >=
#
# Before this fix, pyrust hardcoded '<' in compare_values for all four ops.

class Unord:
    """Class with no ordering dunders defined."""
    pass

for op_str, fn in [
    ("<", lambda: Unord() < Unord()),
    (">", lambda: Unord() > Unord()),
    ("<=", lambda: Unord() <= Unord()),
    (">=", lambda: Unord() >= Unord()),
]:
    try:
        fn()
        print(f"{op_str}: no error")
    except TypeError as e:
        print(f"{op_str}: {e}")

# Cross-type primitives (list vs str): same operator in error
for op_str, fn in [
    ("<", lambda: [1] < "a"),
    (">", lambda: [1] > "a"),
    ("<=", lambda: [1] <= "a"),
    (">=", lambda: [1] >= "a"),
]:
    try:
        fn()
        print(f"list{op_str}str: no error")
    except TypeError as e:
        print(f"list{op_str}str: {e}")

# Class that only defines __le__ and __ge__: <= and >= work, < and > raise TypeError
class LeGe:
    def __init__(self, v):
        self.v = v

    def __le__(self, other):
        return self.v <= other.v

    def __ge__(self, other):
        return self.v >= other.v

a, b = LeGe(1), LeGe(2)
print(a <= b)   # True
print(b >= a)   # True

try:
    a < b
    print("< no error")
except TypeError as e:
    print(f"< TypeError: {e}")

try:
    b > a
    print("> no error")
except TypeError as e:
    print(f"> TypeError: {e}")

# sorted() still raises TypeError for classes with no __lt__
class NoOrder:
    pass

try:
    sorted([NoOrder(), NoOrder()])
    print("sorted: no error")
except TypeError:
    print("sorted: TypeError")

# sorted() works normally for classes with __lt__
class Orderable:
    def __init__(self, v):
        self.v = v

    def __lt__(self, other):
        return self.v < other.v

print([x.v for x in sorted([Orderable(3), Orderable(1), Orderable(2)])])
