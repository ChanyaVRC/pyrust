# Issue #2297: list/tuple.__iter__ usable as a callable dunder, and the
# list/tuple rich-comparison dunders return True/False (not NotImplemented)
# when called through the unbound class descriptor with compatible operands.
#
# Note: the *repr* / type() of the iterator object is deliberately not asserted
# here -- list iterators print as a placeholder in pyrust (the same as the
# builtin iter([...]) does), which is a separate pre-existing concern. What
# matters for #2297 is that the dunder is callable and yields a working
# iterator.

# --- list.__iter__ (bound and unbound) -------------------------------------
it = [1, 2, 3].__iter__()
print(next(it))
print(next(it))
print(next(it))

it2 = list.__iter__([10, 20, 30])
print(list(it2))

print([i for i in [1, 2, 3].__iter__()])
print(list(i for i in [4, 5, 6].__iter__()))

# Empty list iterator is still valid.
print(list([].__iter__()))

# --- tuple.__iter__ (bound and unbound) ------------------------------------
ti = (1, 2, 3).__iter__()
print(next(ti))
print(list(ti))

print(list(tuple.__iter__((7, 8, 9))))
print(list(().__iter__()))

# --- list rich-comparison dunders via the unbound class descriptor ---------
print(list.__lt__([1, 2], [3, 4]))   # True
print(list.__lt__([3, 4], [1, 2]))   # False
print(list.__le__([1, 2], [1, 2]))   # True
print(list.__le__([2], [1]))         # False
print(list.__gt__([3], [2]))         # True
print(list.__gt__([2], [3]))         # False
print(list.__ge__([3], [2]))         # True
print(list.__ge__([1], [2]))         # False
print(list.__eq__([1, 2], [1, 2]))   # True
print(list.__eq__([1, 2], [1, 3]))   # False
print(list.__ne__([1, 2], [1, 3]))   # True
print(list.__ne__([1, 2], [1, 2]))   # False

# Incompatible operand types still yield NotImplemented (correct behaviour).
print(list.__lt__([1, 2], "not a list"))
print(list.__eq__([1, 2], "not a list"))

# --- tuple rich-comparison dunders via the unbound class descriptor --------
print(tuple.__lt__((1, 2), (3, 4)))  # True
print(tuple.__eq__((1, 2), (1, 2)))  # True
print(tuple.__gt__((3,), (2,)))      # True
print(tuple.__eq__((1, 2), "nope"))  # NotImplemented
