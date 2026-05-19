# CPython 3.12 parity: each factory call returns a distinct function object

def make_fn():
    def f():
        pass
    return f

f1 = make_fn()
f2 = make_fn()
print(f1 is f2)       # False
print(id(f1) == id(f2))  # False
print(f1 is f1)       # True  (same object compared to itself)

# Calling both still works
f1()
f2()
print("ok")           # ok

# Lambda factory
def make_lam():
    return lambda x: x + 1

l1 = make_lam()
l2 = make_lam()
print(l1 is l2)       # False
print(l1(5))          # 6
print(l2(10))         # 11

# Class factory
def make_class():
    class C:
        pass
    return C

C1 = make_class()
C2 = make_class()
print(C1 is C2)       # False
