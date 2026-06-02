# Issue #1939: list/tuple/dict/set subclasses work with +, *, ==, < against
# their base type (operators/comparisons inherited; result type is the base).
class L(list):
    pass


class T(tuple):
    pass


class D(dict):
    pass


class St(set):
    pass


# Concatenation / repetition yield the *base* type.
print(L([1]) + L([2]))
print([1] + L([2]))
print(L([1]) + [2])
print(T((1,)) * 2)
print(2 * T((1,)))
print(type(L([1]) + L([2])).__name__)
print(type(T((1,)) * 2).__name__)

# Ordering.
print(L([1]) < L([2]))
print(L([1, 2]) < [1, 3])
print(sorted([L([2]), [1]]))

# Equality against the base value (and nested).
print(L([1, 2]) == [1, 2])
print(D({1: "a"}) == {1: "a"})
print(St({1, 2}) == {1, 2})
print(T((1, 2)) == (1, 2))
print(D({1: [1, 2]}) == {1: [1, 2]})

# Membership relies on ==.
print(L([1, 2]) in [[1, 2]])

# A user dunder override still wins.
class LAdd(list):
    def __add__(self, other):
        return "custom"


print(LAdd([1]) + [2])

class LEq(list):
    def __eq__(self, other):
        return False

    __hash__ = None


print(LEq([1, 2]) == [1, 2])
