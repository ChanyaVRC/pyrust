def f(a, *args, b):
    pass

try:
    f(1)
except TypeError as e:
    print(e)   # f() missing 1 required keyword-only argument: 'b'

def g(*, x, y):
    pass

try:
    g(x=1)
except TypeError as e:
    print(e)   # g() missing 1 required keyword-only argument: 'y'

def h(a, b, *, c):
    return a + b + c

print(h(1, 2, c=3))   # 6
