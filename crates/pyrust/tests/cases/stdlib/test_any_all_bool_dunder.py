# Regression for #432: any/all/filter must dispatch __bool__ on user classes.
# Previously they bypassed the dunder and treated every instance as truthy.
#
# Notes on coverage:
#   * `sorted(reverse=<instance>)` is also fixed at the source level, but
#     CPython coerces `reverse=` via `__index__`, not `__bool__`, and rejects
#     an instance that defines only `__bool__`.  Verifying that path with a
#     parity fixture would require an unrelated `__index__`/coercion fix,
#     so we exercise it via bool-typed reverse only.
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

# --- sorted(reverse=...) with regular bool — confirms no regression ---
print("sorted-rev-true", sorted([1, 3, 2], reverse=True))
print("sorted-rev-false", sorted([1, 3, 2], reverse=False))


# --- class that uses __len__ as the truthiness fallback ---
class L:
    def __init__(self, n):
        self.n = n
    def __len__(self):
        return self.n


print("any-len", any([L(0), L(0), L(3)]))
print("all-len", all([L(1), L(2), L(0)]))
print("filter-len-count", len(list(filter(None, [L(0), L(1), L(0), L(2)]))))
