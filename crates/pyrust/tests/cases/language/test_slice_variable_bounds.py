# Parity fixture for slice operations where bounds are local variables (issue #862).
#
# compile_slice_key previously aliased the step register with the upper-bound
# register when the upper bound was a local variable and step was None.  The
# result was that a[:x] produced a[:x:x] (step was the upper bound value rather
# than None).

a = [0, 1, 2, 3, 4]

# None lower, local upper
x = 3
print(a[:x])       # [0, 1, 2]

# Local lower, None upper
print(a[x:])       # [3, 4]

# None lower, None upper, local step
s = 2
print(a[::s])      # [0, 2, 4]

# Local lower and upper, None step
lo = 1
hi = 4
print(a[lo:hi])    # [1, 2, 3]

# All three are locals
print(a[lo:hi:s])  # [1, 3]

# Same variable used for lower and step
m = 2
print(a[m::m])     # [2, 4]

# String slicing with variable bounds
t = "hello world"
n = 5
print(t[:n])       # hello
print(t[n + 1:])   # world

# Tuple slicing with variable bounds
tp = (10, 20, 30, 40, 50)
i = 1
j = 4
print(tp[i:j])     # (20, 30, 40)

# Negative step via variable
k = -1
print(a[::k])      # [4, 3, 2, 1, 0]

# Nested function scope — bounds captured from enclosing locals
def make_slicer(seq, stop):
    return seq[:stop]

print(make_slicer(a, 3))  # [0, 1, 2]

# Slice on the result of an expression
b = list(range(10))
step = 3
print(b[::step])   # [0, 3, 6, 9]

# Zero-length slice with local bounds
z = 2
print(a[z:z])      # []
