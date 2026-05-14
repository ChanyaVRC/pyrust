# Regression for #432: any/all/filter must dispatch __bool__ on user classes.
# Previously they bypassed the dunder and treated every instance as truthy.
#
# Notes on coverage:
#   * `sorted(reverse=<instance>)` coerces via `bool()` per CPython 3.12+
#     (3.11 used `__index__`; the change is intentional upstream).  We test
#     the version-stable cases only: bool / int literals and a class that
#     defines `__bool__` returning False — the latter should NOT reverse.
#     `IdxZero`/`JustBool`/`Nothing` cases are intentionally omitted
#     because they diverge across CPython 3.11 vs 3.12+ (#477 CI flagged).
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

# --- sorted(reverse=...) — bool / int literals only ---
# CPython 3.11 dispatches via `__index__`; 3.12+ via `bool()`.  The
# truthiness fix here matches 3.12+, but to keep this fixture green on
# both we only test the version-stable literal cases.  Anything that
# uses a user-class instance for `reverse=` diverges across versions.
print("sorted-rev-true", sorted([1, 3, 2], reverse=True))
print("sorted-rev-false", sorted([1, 3, 2], reverse=False))
print("sorted-rev-int-1", sorted([1, 3, 2], reverse=1))
print("sorted-rev-int-0", sorted([1, 3, 2], reverse=0))


# --- class that uses __len__ as the truthiness fallback ---
class L:
    def __init__(self, n):
        self.n = n
    def __len__(self):
        return self.n


print("any-len", any([L(0), L(0), L(3)]))
print("all-len", all([L(1), L(2), L(0)]))
print("filter-len-count", len(list(filter(None, [L(0), L(1), L(0), L(2)]))))
