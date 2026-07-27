# Augmented-assignment parity across primitive and container targets.
# BinOpInPlace remains an observable protocol operation: the runtime must retain
# __iadd__ / __isub__ / __imul__ behavior instead of assuming plain BinOp is
# equivalent from bytecode-only type facts.

# Integer augmented assignment in a tight loop
# The local target remains an in-place operation on every iteration.
s = 0
for i in range(100):
    s += i
print(s)        # 4950

# Float augmented assignment
f = 0.0
for i in range(10):
    f += 0.1
print(f > 0.99 and f < 1.01)  # True (float accumulation ~= 1.0)

# String concatenation (augmented assignment on str)
acc = ""
for ch in "hello":
    acc += ch
print(acc)      # hello

# Chained augmented assignments — multiple BinOpInPlace in one expression
x = 10
x += 5
x -= 3
x *= 2
print(x)        # (10+5-3)*2 = 24

# Augmented assignment inside a function
def sum_squares(n):
    total = 0
    for i in range(n):
        total += i * i
    return total

print(sum_squares(5))   # 0+1+4+9+16 = 30
print(sum_squares(0))   # 0

# Mixed int/float augmented assignment
v = 1
v += 1.5
v += 2
print(v)        # 4.5

# Augmented assignment with expressions on the right-hand side
a = 0
b = 3
for _ in range(4):
    a += b * 2
print(a)        # 4 * 6 = 24

# In-place with a list element must preserve target write-back semantics.
lst = [1, 2, 3]
lst[0] += 10
print(lst)      # [11, 2, 3]
