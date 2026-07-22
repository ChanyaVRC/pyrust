# list.sort() with the type-specialized comparator (shared with sorted() via
# pyrust-core's classify_sort): homogeneous all-int / all-str lists take a native
# fast path; every other case stays on the general comparator and must match
# CPython byte-for-byte.


class Rev:
    def __init__(self, n):
        self.n = n

    def __lt__(self, other):
        return self.n > other.n

    def __repr__(self):
        return f"Rev({self.n})"


def sorted_copy(seq, **kw):
    x = list(seq)
    x.sort(**kw)
    return x


# All-int fast path + reverse.
print(sorted_copy([5, 3, 8, 1, 9, 2, 7]))
print(sorted_copy([5, 3, 8, 1, 9, 2, 7], reverse=True))
print(sorted_copy([-3, -1, -2, 0, 5, -100]))
print(sorted_copy([2, 2, 1, 1, 3, 3]))
print(sorted_copy([1]))
print(sorted_copy([]))

# All-str fast path + reverse.
print(sorted_copy(["banana", "apple", "cherry", "apple"]))
print(sorted_copy(["b", "a", "c"], reverse=True))
print(sorted_copy(["", "a", "ab", "aa"]))

# key= (classified by key).
print(sorted_copy([3, 1, 2], key=lambda x: -x))
print(sorted_copy(["aa", "b", "ccc"], key=len))
print(sorted_copy(["Banana", "apple", "Cherry"], key=str.lower))

# General path: mixed numeric, BigInt, tuples/lists.
print(sorted_copy([1.5, 2, True, 0, 3.0]))
print(sorted_copy([10**30, 5, 10**40, 3]))
print(sorted_copy([(1, "b"), (1, "a"), (0, "z")]))
print(sorted_copy([[2], [1], [1, 0]]))

# Instances with a user __lt__ -> general comparator.
print(sorted_copy([Rev(1), Rev(3), Rev(2)]))
print(sorted_copy([Rev(1), Rev(3), Rev(2)], reverse=True))

# Stability: equal keys keep input order (both fast and general paths).
print(sorted_copy([(1, "x"), (0, "z"), (1, "y"), (0, "w")], key=lambda t: t[0]))

# reverse= coerced via bool() (non-bool argument).
print(sorted_copy([3, 1, 2], reverse=1))
print(sorted_copy([3, 1, 2], reverse=[]))

# sort() returns None and mutates in place.
m = [3, 1, 2]
print(m.sort(), m)
