# Regression for #432: any/all/filter must dispatch __bool__ on user classes.
# Previously they bypassed the dunder and treated every instance as truthy.
#
# Notes on coverage:
#   * `sorted(reverse=<instance>)` coerces via `__index__` (not `__bool__`)
#     per CPython.  Class with only `__bool__` raises TypeError; class with
#     `__index__` returning 0 / non-zero controls reversal.  See
#     `sorted-rev-*` cases below.
#   * `__repr__` is not yet dispatched inside `list.__repr__` (separate bug),
#     so we count filter results with `len()` rather than printing them.


class T:
    def __init__(self, b):
        self.b = b
    def __bool__(self):
        return self.b


# --- any ---
print("any-all-false", any([T(False), T(False)]))
print("any-mixed", any([T(False), T(True)]))
print("any-empty", any([]))
print("any-mixed-prim", any([0, T(False), False, T(True)]))
print("any-only-instance-true", any([T(True)]))

# --- all ---
print("all-all-true", all([T(True), T(True)]))
print("all-mixed", all([T(True), T(False)]))
print("all-empty", all([]))
print("all-mixed-prim", all([1, T(True), "x", T(True)]))
print("all-only-instance-false", all([T(False)]))

# --- filter(None, ...) — identity predicate keeps truthy elements ---
print("filter-none-count", len(list(filter(None, [T(False), T(True), T(False), T(True)]))))
print("filter-none-mixed-count", len(list(filter(None, [0, T(True), "", T(False), 1, T(True)]))))

# --- filter with predicate — predicate's return value dispatches __bool__ ---
print("filter-pred-identity-count", len(list(filter(lambda x: x, [T(False), T(True), T(False)]))))
print("filter-pred-returns-instance", list(filter(lambda x: T(x > 0), [-1, 0, 1, 2])))

# --- sorted(reverse=...) ---
# Plain bool / int — implicit __index__.
print("sorted-rev-true", sorted([1, 3, 2], reverse=True))
print("sorted-rev-false", sorted([1, 3, 2], reverse=False))
print("sorted-rev-int-1", sorted([1, 3, 2], reverse=1))
print("sorted-rev-int-0", sorted([1, 3, 2], reverse=0))


# User class with __index__ — non-zero / zero controls reversal.
class IdxOne:
    def __index__(self): return 1


class IdxZero:
    def __index__(self): return 0


print("sorted-rev-idx-one", sorted([1, 3, 2], reverse=IdxOne()))
print("sorted-rev-idx-zero", sorted([1, 3, 2], reverse=IdxZero()))


# User class with only __bool__ — TypeError per CPython.
class JustBool:
    def __bool__(self): return True


try:
    sorted([1, 3, 2], reverse=JustBool())
    print("sorted-rev-justbool: FAIL (no exception)")
except TypeError as e:
    print("sorted-rev-justbool: TypeError")


# User class with neither — TypeError.
class Nothing:
    pass


try:
    sorted([1, 3, 2], reverse=Nothing())
    print("sorted-rev-nothing: FAIL (no exception)")
except TypeError as e:
    print("sorted-rev-nothing: TypeError")


# --- class that uses __len__ as the truthiness fallback ---
class L:
    def __init__(self, n):
        self.n = n
    def __len__(self):
        return self.n


print("any-len", any([L(0), L(0), L(3)]))
print("all-len", all([L(1), L(2), L(0)]))
print("filter-len-count", len(list(filter(None, [L(0), L(1), L(0), L(2)]))))
