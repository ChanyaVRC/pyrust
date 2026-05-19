# Regression test for nonlocal inside a class method reaching the enclosing
# function scope.  CPython 3.12 allows this; pyrust previously raised
# "no binding for nonlocal 'x' found" (#633).

# Basic case: single method mutates enclosing variable.
def outer():
    x = 1
    class C:
        def method(self):
            nonlocal x
            x = 2
    C().method()
    return x

print(outer())   # 2

# Counter: method called multiple times accumulates mutations.
def outer2():
    count = 0
    class Counter:
        def increment(self):
            nonlocal count
            count += 1
    c = Counter()
    c.increment()
    c.increment()
    return count

print(outer2())  # 2

# Nested class: method inside inner class still reaches the outermost function.
def outer3():
    y = "original"
    class Outer:
        class Inner:
            def mutate(self):
                nonlocal y
                y = "mutated"
    Outer.Inner().mutate()
    return y

print(outer3())  # mutated

# Regression: nonlocal in a plain nested function (no class) must still work.
def outer_plain():
    x = 10
    def inner():
        nonlocal x
        x = 20
    inner()
    return x

print(outer_plain())  # 20

# Regression: global declaration in a method must still work.
_g = 100
def outer_global():
    class C:
        def method(self):
            global _g
            _g = 200
    C().method()

outer_global()
print(_g)  # 200
