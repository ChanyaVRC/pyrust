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


# A proper RHS subtype gets first refusal through the swapped rich-comparison
# slot.  This is the do_richcompare rule (distinct from arithmetic __r* slot
# priority) and applies to equality as well as all four ordering operations.
events = []


class RichBase:
    def __eq__(self, other):
        events.append("base eq")
        return "base eq result"

    def __lt__(self, other):
        events.append("base lt")
        return "base lt result"

    def __le__(self, other):
        events.append("base le")
        return "base le result"

    def __gt__(self, other):
        events.append("base gt")
        return "base gt result"

    def __ge__(self, other):
        events.append("base ge")
        return "base ge result"


class RichSub(RichBase):
    def __eq__(self, other):
        events.append("sub eq")
        return "sub eq result"

    def __lt__(self, other):
        events.append("sub lt")
        return "sub lt result"

    def __le__(self, other):
        events.append("sub le")
        return "sub le result"

    def __gt__(self, other):
        events.append("sub gt")
        return "sub gt result"

    def __ge__(self, other):
        events.append("sub ge")
        return "sub ge result"


left = RichBase()
right = RichSub()
for label, operation in (
    ("eq", lambda: left == right),
    ("lt", lambda: left < right),
    ("le", lambda: left <= right),
    ("gt", lambda: left > right),
    ("ge", lambda: left >= right),
):
    events.clear()
    print("subtype priority", label, operation(), events)


# If the priority slot declines, the LHS slot runs next and the RHS slot is
# not invoked a second time.
class DecliningBase:
    def __lt__(self, other):
        events.append("base lt")
        return "base fallback"


class DecliningSub(DecliningBase):
    def __gt__(self, other):
        events.append("sub gt")
        return NotImplemented


events.clear()
print("subtype not implemented", DecliningBase() < DecliningSub(), events)


# Priority also applies when the subtype inherits the swapped method without
# replacing it: CPython's type-level richcmp slot is still tried on the RHS.
class InheritedBase:
    def __lt__(self, other):
        events.append(type(self).__name__ + " lt")
        return NotImplemented

    def __gt__(self, other):
        events.append(type(self).__name__ + " gt")
        return "inherited rhs"


class InheritedSub(InheritedBase):
    pass


events.clear()
print("subtype inherited", InheritedBase() < InheritedSub(), events)
