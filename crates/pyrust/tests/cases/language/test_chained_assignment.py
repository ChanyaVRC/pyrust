a = b = c = 10
print(a, b, c)              # 10 10 10

a = b = []
b.append(1)
print(a, b)                 # [1] [1]   — same object since RHS evaluated once
print(a is b)               # True

# Three-way with side-effect on RHS — RHS evaluated only once
def f():
    print("rhs-called")
    return 42

x = y = f()
print(x, y)
# Output: rhs-called; 42 42

# Mixed targets — subscripts + bare names
d = {}
d["k"] = z = "value"
print(d["k"], z)            # value value
