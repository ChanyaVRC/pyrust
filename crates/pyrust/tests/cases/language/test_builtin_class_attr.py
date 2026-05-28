class A:
    pass

A.f = len
a = A()

print(type(a.f).__name__)   # builtin_function_or_method
print(a.f([1, 2, 3]))       # 3
print(a.f("hello"))          # 5

A.g = abs
print(a.g(-5))               # 5
print(a.g(3.14))             # 3.14

# Builtin still callable directly via class
print(A.f([1, 2]))           # 2

# User functions still bind correctly (no regression)
def greet(self, name):
    return f"hello {name}"

A.greet = greet
print(a.greet("world"))      # hello world
