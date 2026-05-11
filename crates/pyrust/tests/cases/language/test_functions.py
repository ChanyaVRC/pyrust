# Functions: definitions, defaults, keywords, lambda, *args, closures, global, nonlocal

# Basic functions
def add(a, b):
    return a + b


def sum_list(values):
    total = 0
    for value in values:
        total = total + value
    return total


def fact(n):
    if n <= 1:
        return 1
    return n * fact(n - 1)


print("fn-add", add(2, 5))
print("fn-sum-list", sum_list([4, 5, 6]))
print("fn-fact", fact(5))

# Default and keyword arguments
def greet(name, suffix="!"):
    return name + suffix


def power(base, exp=2):
    result = 1
    i = 0
    while i < exp:
        result = result * base
        i = i + 1
    return result


print("fn-default", greet("hi"), greet("hi", "?"))
print("fn-default-exp", power(3), power(2, 3))
print("fn-keyword", greet(name="hi", suffix="?"), greet("hi", suffix="."))
print("fn-keyword-default", power(base=4), power(exp=3, base=2))

# Closures (variable rebinding)
def outer():
    x = 1

    def inner():
        return x

    x = 2
    return inner()


print("fn-closure-rebind", outer())

# global
g_counter = 10


def use_global():
    global g_counter
    before = g_counter
    g_counter = g_counter + 5
    return before


print("global", use_global(), g_counter)

# Nonlocal variables
def outer_nonlocal():
    x = 1

    def inner_nonlocal():
        nonlocal x
        x = x + 4
        return x

    return [inner_nonlocal(), x]


print("nonlocal", outer_nonlocal())

# lambda
sq = lambda x: x * x
print("lambda", sq(7))
add2 = lambda a, b: a + b
print("lambda2", add2(3, 4))

# Default arguments (alternate example)
def greet2(name, msg="hello"):
    return msg + " " + name


print("default", greet2("world"))
print("default-kw", greet2("world", msg="hi"))

# *args (variadic positional arguments)
def my_sum(*args):
    total = 0
    for v in args:
        total += v
    return total


print("varargs", my_sum(1, 2, 3, 4))


# Call-site *args/**kwargs
vals = [1, 2, 3]
print("call-splat", my_sum(*vals, 4))


def greet3(name, msg="hi"):
    return msg + " " + name


kw = {"name": "world", "msg": "hello"}
print("call-doublesplat", greet3(**kw))


# Decorators
def deco(fn):
    def wrapped(x):
        return fn(x) + 1
    return wrapped


@deco
def inc(x):
    return x + 1


print("decorator", inc(10))


# Unexpected keyword argument should error  
def simple(a, b):
    return a + b


try:
    simple(1, 2, c=3)
except Exception as e:
    print("unexpected-kw", "got unexpected keyword argument")
