def make_adder(n):
    return lambda x: x + n

add5 = make_adder(5)
print(add5(3))
print(add5(10))

def outer(n):
    f = lambda x: x + n
    return f(1)

print(outer(10))

def make_greeter(greeting):
    greet = lambda name: greeting + ", " + name
    return greet

hello = make_greeter("Hello")
print(hello("world"))
