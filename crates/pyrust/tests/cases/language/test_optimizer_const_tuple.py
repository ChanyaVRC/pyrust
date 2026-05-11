# Constant tuple literal used in iteration — should fold to a single LoadConst
total = 0
for x in (1, 2, 3):
    total += x
print(total)       # 6

# Tuple used as a function argument
def f(t):
    return t[0] + t[1]
print(f((10, 20)))  # 30

# Nested constant tuple
t = ((1, 2), (3, 4))
print(t[0][1])     # 2
