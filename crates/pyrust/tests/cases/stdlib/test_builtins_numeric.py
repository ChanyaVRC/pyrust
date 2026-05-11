# abs, min, max, sum

# --- abs ---
print("abs-neg-int", abs(-7))
print("abs-pos-int", abs(5))
print("abs-zero", abs(0))
print("abs-neg-float", abs(-3.14))
print("abs-pos-float", abs(2.5))

# --- min ---

# multiple args
print("min-args", min(3, 1, 4, 1, 5))

# single iterable
print("min-list", min([9, 2, 7, 4]))

# strings
print("min-str-args", min('banana', 'apple', 'cherry'))

# one-element
print("min-one", min([42]))

# --- max ---

# multiple args
print("max-args", max(3, 1, 4, 1, 5, 9, 2, 6))

# single iterable
print("max-list", max([9, 2, 7, 4]))

# strings
print("max-str-args", max('banana', 'apple', 'cherry'))

# one-element
print("max-one", max([99]))

# --- sum ---

# integers
print("sum-int", sum([1, 2, 3, 4, 5]))

# with start value
print("sum-start", sum([1, 2, 3], 10))

# empty with start
print("sum-empty", sum([], 7))

# floats
print("sum-float", sum([0.5, 1.5, 2.0]))

# range
print("sum-range", sum(range(1, 6)))
