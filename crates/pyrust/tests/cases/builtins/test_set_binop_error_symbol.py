# Issue #1714: set binary ops with non-set RHS use the binary-op symbol (|, &, -, ^),
# not the augmented-assign symbol (|=, &=, -=, ^=).

try:
    {1, 2} | 42
except TypeError as e:
    print(e)

try:
    {1, 2} & 42
except TypeError as e:
    print(e)

try:
    {1, 2} - 42
except TypeError as e:
    print(e)

try:
    {1, 2} ^ 42
except TypeError as e:
    print(e)

# Augmented assign keeps the |= symbol.
s = {1, 2}
t = 42
try:
    s |= t
except TypeError as e:
    print(e)

# Valid set operations still work.
print({1, 2} | {3})
print({1, 2} & {2, 3})
