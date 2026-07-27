# Parity fixture for issue #1943: augmented assignment must retain
# `BinOpInPlace` / `__iadd__` semantics for containers and other dynamic values.
#
# `obj[i] += <iterable>` where obj[i] is a list extends in place (list.extend,
# accepts ANY iterable). Treating it as plain `+` would build a new list, reject
# non-list operands, and can raise a spurious TypeError.


# --- subscript target, list += various iterables -----------------------------
def subscript_tuple():
    l = [[1, 2]]
    l[0] += (9,)
    return l


print(subscript_tuple())  # [[1, 2, 9]]


def subscript_set():
    l = [[1, 2]]
    l[0] += {9, 10}
    return [sorted(l[0])]


print(subscript_set())  # [[1, 2, 9, 10]]


def subscript_dict():
    l = [[1, 2]]
    l[0] += {9: "a", 10: "b"}  # extends with keys
    return l


print(subscript_dict())  # [[1, 2, 9, 10]]


def subscript_generator():
    l = [[1, 2]]
    l[0] += (x for x in range(3))
    return l


print(subscript_generator())  # [[1, 2, 0, 1, 2]]


def subscript_str():
    l = [["a"]]
    l[0] += "bc"  # str is iterable → extends with chars
    return l


print(subscript_str())  # [['a', 'b', 'c']]


# --- in-place identity is preserved (same object mutated, not replaced) -------
def identity_preserved():
    l = [[1, 2]]
    x = l[0]
    l[0] += (9,)
    return x is l[0]


print(identity_preserved())  # True


# --- attribute target ---------------------------------------------------------
class Box:
    pass


def attribute_list():
    b = Box()
    b.x = [1, 2]
    b.x += (9,)
    return b.x


print(attribute_list())  # [1, 2, 9]


# --- liveness stability: adding trailing statements must not change result ----
def trailing_stmts():
    l = [[1, 2]]
    l[0] += (9,)
    a = 5
    b = a + 1
    c = b * 2
    return l, c


print(trailing_stmts())  # ([[1, 2, 9]], 12)


# --- set |= / dict update in-place semantics ----------------------------------
def subscript_set_ior():
    l = [{1, 2}]
    l[0] |= {3, 4}
    return [sorted(l[0])]


print(subscript_set_ior())  # [[1, 2, 3, 4]]


# --- numeric primitive subscript remains correct -------------------------------
def subscript_int():
    l = [10]
    l[0] += 5
    return l


print(subscript_int())  # [15]


def subscript_float():
    l = [1.5]
    l[0] += 2.0
    return l


print(subscript_float())  # [3.5]


# --- user type with side-effecting __iadd__ via subscript ---------------------
class Counter:
    def __init__(self, n):
        self.n = n

    def __iadd__(self, other):
        self.n += other * 2  # doubles the increment; must be called
        return self

    def __repr__(self):
        return f"Counter({self.n})"


def subscript_user_iadd():
    l = [Counter(0)]
    l[0] += 5
    return l


print(subscript_user_iadd())  # [Counter(10)]
