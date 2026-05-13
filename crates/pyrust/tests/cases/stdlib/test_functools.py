# functools.reduce — 2-arg, 3-arg, empty-iterable error path.

from functools import reduce

# --- two-arg form: fold from the left ---
print("sum", reduce(lambda a, b: a + b, [1, 2, 3, 4]))
print("product", reduce(lambda a, b: a * b, [1, 2, 3, 4]))

# Single-element iterable: first element is returned as-is, function isn't called.
print("single", reduce(lambda a, b: a + b, [42]))

# --- three-arg form: initializer is the seed ---
print("sum-init", reduce(lambda a, b: a + b, [1, 2, 3], 100))
print("concat-init", reduce(lambda a, b: a + b, ['a', 'b', 'c'], 'zero'))

# Empty iterable with initializer returns the initializer.
print("empty-init", reduce(lambda a, b: a + b, [], 'seed'))

# --- empty iterable WITHOUT initializer is a TypeError ---
# Don't print the full message — CPython says "reduce() of empty iterable..."
# while pyrust qualifies with "functools.reduce()".  Same exception, same
# semantics, different wording.  Assert just the substring that matters.
try:
    reduce(lambda a, b: a + b, [])
except TypeError as e:
    print("empty-noinit", "of empty iterable with no initial value" in str(e))
