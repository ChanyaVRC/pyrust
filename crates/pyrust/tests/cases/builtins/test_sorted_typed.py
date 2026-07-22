# sorted() with the type-specialized comparator: homogeneous all-int / all-str
# slices take a native fast path; every other case (reverse, key, mixed numeric,
# tuples, BigInt, instances, stability) must stay byte-identical to CPython.


class Rev:
    def __init__(self, n):
        self.n = n

    def __lt__(self, other):
        return self.n > other.n

    def __repr__(self):
        return f"Rev({self.n})"


# All-int (fast path) + reverse.
print(sorted([5, 3, 8, 1, 9, 2, 7]))
print(sorted([5, 3, 8, 1, 9, 2, 7], reverse=True))
print(sorted([-3, -1, -2, 0, 5, -100]))
print(sorted([1]))
print(sorted([]))
print(sorted([2, 2, 1, 1, 3, 3]))  # duplicates
print(sorted([True, False, 2, 0]))  # bool is int-kind... mixed with int -> all "int-ish"?

# All-str (fast path) + reverse.
print(sorted(["banana", "apple", "cherry", "apple"]))
print(sorted(["b", "a", "c"], reverse=True))
print(sorted(["", "a", "ab", "aa"]))

# key= (classified by key).
print(sorted([3, 1, 2], key=lambda x: -x))
print(sorted(["aa", "b", "ccc"], key=len))
print(sorted(["Banana", "apple", "Cherry"], key=str.lower))

# Mixed numeric -> General path (int/float/bool ordering).
print(sorted([1.5, 2, True, 0, 3.0]))
print(sorted([10**30, 5, 10**40, 3]))  # BigInt mix

# Tuples / lists -> General (lexicographic).
print(sorted([(1, "b"), (1, "a"), (0, "z")]))
print(sorted([[2], [1], [1, 0]]))

# Instances with user __lt__ -> HasInstance path.
print(sorted([Rev(1), Rev(3), Rev(2)]))
print(sorted([Rev(1), Rev(3), Rev(2)], reverse=True))

# Stability: equal keys preserve input order.
pairs = [(1, "x"), (0, "z"), (1, "y"), (0, "w")]
print(sorted(pairs, key=lambda t: t[0]))
