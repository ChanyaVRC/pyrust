# Parity fixture for BigInt object identity (issue #523).

x = 2**64
y = +x
print(x is y)       # unary + preserves the same object

a = 2**64
b = a
print(a is b)       # assignment aliases the same BigInt object

x = -(2**64)
y = -x
print(y is x)       # unary - allocates a new object

left = 2**64
right = left + 0
print(left is right)  # equal BigInts from separate operations are distinct
