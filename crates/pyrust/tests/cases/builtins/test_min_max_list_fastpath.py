# min(list)/max(list) borrowed-slice fast path: reduce over the list's backing
# without cloning every element (collect_iterable).  Must match CPython 3.12
# across all shapes; the fast path only engages for an all-primitive list with
# no key, so key/tuple/PyInstance/varargs/empty all fall through unchanged.

print(min([3, 1, 2]), max([3, 1, 2]))
print(min([-5]), max([-5]))
print(min([1.5, -2.0, 3.0]), max([1.5, -2.0, 3.0]))
print(min(["banana", "apple", "cherry"]), max(["banana", "apple", "cherry"]))
print(min([2**70, 2**70 + 1, 2**70 - 1]))  # BigInt elements
print(min([True, False, True]), max([False, True, False]))  # bool
print(min([1, True, 0, False]), max([0, False, 1, True]))   # bool/int mix
print(min([1, 2.0, 3]), max([1, 2.0, 3]))                   # int/float mix

# Fall-through paths (must be identical):
print(min([1, 2, 3], key=lambda x: -x), max([1, 2, 3], key=lambda x: -x))  # key
print(min((3, 1, 2)), max((3, 1, 2)))          # tuple (not list)
print(min(3, 1, 2), max(3, 1, 2))              # varargs
print(min([], default=99), max([], default=-99))  # empty + default
print(min(range(5, 10)), max(range(5, 10)))    # range iterable


class C:
    def __init__(self, v):
        self.v = v

    def __lt__(self, o):
        return self.v < o.v

    def __gt__(self, o):
        return self.v > o.v

    def __repr__(self):
        return f"C({self.v})"


print(min([C(3), C(1), C(2)]), max([C(3), C(1), C(2)]))  # PyInstance elements


def show_err(fn):
    try:
        print(fn())
    except Exception as e:
        print(type(e).__name__ + ":", e)


show_err(lambda: min([1, "a"]))   # unorderable primitives -> TypeError '<'
show_err(lambda: max([1, "a"]))   # -> TypeError '>'
show_err(lambda: min([]))          # empty, no default -> ValueError
show_err(lambda: max([]))
