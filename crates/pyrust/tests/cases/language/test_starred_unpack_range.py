# Regression test for issue #1358:
# A second starred unpack of range() returned wrong values when a prior
# UnpackEx had written over temp registers that the CSE table still thought
# held compile-time constants from list element loads.

# Basic case: prior starred unpack with n_after=1, then *x, last = range(n)
a, *b, c = [1, 2, 3, 4, 5]
*x, last = range(4)
print(x)
print(last)

# prior unpack with n_before=1, then starred unpack of range
first, *middle, end = [10, 20, 30]
*p, q = range(3)
print(p)
print(q)

# Multiple starred unpacks with range in sequence
_, *r1, _ = [1, 2, 3, 4]
*r2, _ = range(5)
print(r1)
print(r2)

# Starred unpack of range without a prior starred unpack (regression guard)
*y, z = range(6)
print(y)
print(z)

# Starred unpack at the start
*head, tail = range(2)
print(head)
print(tail)

# Range unpacked fully as starred (single element after)
a2, *b2, c2 = range(5)
print(a2)
print(b2)
print(c2)
