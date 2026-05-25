# Parity fixture for min() / max() — default= kwarg and correct exception types.

# default= with empty sequence
print(min([], default=0))
print(max([], default=-1))
print(min([], default='x'))
print(min([], default=None))

# empty sequence without default raises ValueError
try:
    min([])
except ValueError as e:
    print("ValueError:", e)

try:
    max([])
except ValueError as e:
    print("ValueError:", e)

# zero positional arguments raises TypeError
try:
    min()
except TypeError as e:
    print("TypeError:", e)

try:
    max()
except TypeError as e:
    print("TypeError:", e)

# positional-arg forms still work
print(min(1, 2, 3))
print(max(1, 2, 3))
print(min(3, 1, 2))
print(max(3, 1, 2))

# key= kwarg
print(min([3, 1, 2], key=lambda x: -x))
print(max([3, 1, 2], key=lambda x: -x))

# default= with a non-empty sequence is ignored (returns the min/max)
print(min([5, 3, 7], default=99))
print(max([5, 3, 7], default=-99))

# default= with multiple positionals raises TypeError
try:
    min(1, 2, default=0)
except TypeError as e:
    print("TypeError:", e)

try:
    max(1, 2, default=0)
except TypeError as e:
    print("TypeError:", e)

# invalid keyword argument raises TypeError
try:
    min([], foo=1)
except TypeError as e:
    print("TypeError:", e)

try:
    max([1, 2], bar=1)
except TypeError as e:
    print("TypeError:", e)
