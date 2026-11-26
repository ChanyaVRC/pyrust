# Call trampoline for calls that omit trailing defaulted positional parameters
# (#3381).
#
# The trampoline binds the omitted parameters from the callee's defaults at
# call time.  Each section pins one way that binding can diverge from CPython:
# a stale or aliased default, a reassigned / deleted `__defaults__`, a call
# with too few or too many arguments, or a default that is read mid-call.


def add3(x, y=1, z=2):
    return x + y + z


print("== omitted defaults ==")


def drive_omitted(n):
    acc = 0
    for i in range(n):
        acc += add3(i)
        acc += add3(i, 10)
        acc += add3(i, 10, 20)
    return acc


print(drive_omitted(50))

print("== mutable defaults are shared across calls ==")


def push(v, bag=[]):
    bag.append(v)
    return len(bag)


print([push(i) for i in range(5)])
print(push.__defaults__)
shared = []
print(push(1, shared), push(2, shared), push(3), push(4))
print(push.__defaults__)


def dict_default(k, d={}):
    d[k] = len(d)
    return d


for i in range(3):
    dict_default(i)
print(dict_default(99))

print("== default is the identical object ==")
sentinel = object()


def ident(a, b=sentinel):
    return b


print(all(ident(i) is sentinel for i in range(10)))

print("== __defaults__ reassignment ==")


def re(a, b=1, c=2):
    return (a, b, c)


for i in range(3):
    print(re(i))
re.__defaults__ = (10,)
for i in range(3):
    try:
        print(re(i))
    except TypeError as e:
        print("TypeError", e)
    print(re(i, 5))
re.__defaults__ = (7, 8)
print(re(0), re(0, 1), re(0, 1, 2))
re.__defaults__ = (4, 5, 6)
print(re(), re(1), re(1, 2), re(1, 2, 3))
re.__defaults__ = (1, 2, 3, 4)
print(re(), re(0))
re.__defaults__ = ()
try:
    re(1)
except TypeError as e:
    print("TypeError", e)
re.__defaults__ = None
try:
    re(1)
except TypeError as e:
    print("TypeError", e)
print(re(1, 2, 3))
del re.__defaults__
print(re.__defaults__)
try:
    re(1, 2)
except TypeError as e:
    print("TypeError", e)

print("== reassigned after being trampolined ==")


def late(a, b=1):
    return a + b


print(sum(late(i) for i in range(5)))
late.__defaults__ = (100,)
print(sum(late(i) for i in range(5)))
late.__defaults__ = None
try:
    late(1)
except TypeError as e:
    print("TypeError", e)

print("== too few / too many arguments ==")


def need2(a, b, c=3):
    return (a, b, c)


for args in [(), (1,), (1, 2), (1, 2, 3), (1, 2, 3, 4)]:
    try:
        print(need2(*args))
    except TypeError as e:
        print("TypeError", e)


def direct_calls():
    out = []
    try:
        out.append(need2())
    except TypeError as e:
        out.append(str(e))
    try:
        out.append(need2(1))
    except TypeError as e:
        out.append(str(e))
    out.append(need2(1, 2))
    try:
        out.append(need2(1, 2, 3, 4))
    except TypeError as e:
        out.append(str(e))
    return out


for line in direct_calls():
    print(line)

print("== defaults only, no required parameter ==")


def all_default(a=1, b=2):
    return (a, b)


def drive_all_default():
    return [all_default(), all_default(5), all_default(5, 6)]


print(drive_all_default())

print("== default bound to a name used by the callee body ==")


def uses_default(n, scale=3):
    s = 0
    for i in range(n):
        s += i * scale
    return s


print([uses_default(k) for k in range(6)])
print([uses_default(k, 2) for k in range(6)])

print("== recursion with omitted defaults ==")


def fact(n, acc=1):
    if n <= 1:
        return acc
    return fact(n - 1, acc * n)


print(fact(10), fact(1), fact(5, 2))


def count_down(n, depth=0):
    if n == 0:
        return depth
    return count_down(n - 1)  # `depth` omitted at every level but the first


print(count_down(20), count_down(20, 5))

print("== exceptions raised from a defaulted callee ==")


def boom(a, b=0):
    return a // b


for i in range(3):
    try:
        boom(i)
    except ZeroDivisionError as e:
        print("ZeroDivisionError", e)
print(boom(10, 5))

print("== default captured by a closure ==")


def make(a, b=5):
    def inner():
        return a + b

    return inner


print(make(1)(), make(1, 2)())

print("== methods ==")


class K:
    def m(self, a, b=10):
        return (a, b)

    def n(self, a=1, b=2):
        return (a, b)

    @classmethod
    def c(cls, a, b=3):
        return (cls.__name__, a, b)

    @staticmethod
    def s(a, b=4):
        return (a, b)


k = K()
print(k.m(1), k.m(1, 2), k.n(), k.n(7), k.n(7, 8))
print(K.c(1), K.c(1, 2), K.s(1), K.s(1, 2))
bound = k.m
print(bound(1), bound(1, 2))
K.m.__defaults__ = (99,)
print(k.m(1), bound(2))
try:
    k.n.__func__.__defaults__ = None
    k.n()
except TypeError as e:
    print("TypeError", e)

print("== introspection inside a defaulted callee ==")

import sys


def frame_locals(a, b=2):
    return sorted(sys._getframe().f_locals.items())


print(frame_locals(1))
print(frame_locals(1, 3))

print("== callee that is also a lambda / nested def ==")
lam = lambda a, b=2, c=3: a * b * c
print(lam(1), lam(1, 5), lam(1, 5, 7))


def outer(x):
    def inner(y, z=100):
        return x + y + z

    return inner(1), inner(1, 2)


print(outer(10))
