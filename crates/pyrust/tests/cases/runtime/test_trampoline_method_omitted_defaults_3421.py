# Bound-method call trampoline for calls that omit trailing defaulted positional
# parameters (#3421; plain functions are #3381).
#
# Each section pins one way the binding could diverge from CPython: a mutated or
# deleted `__defaults__`, a shared mutable default, a missing required argument,
# too many arguments, subclass overrides, descriptors, and exact-arity calls.


class K:
    def m(self, x, y=1):
        return (x, y)

    def two(self, x=5, y=6):
        return (x, y)

    def exact(self, x, y):
        return (x, y)


print("== omitted defaults (CallMethod and Call-on-bound) ==")


def drive(n):
    k = K()
    bound = k.m
    out = []
    for i in range(n):
        out.append(k.m(i))
        out.append(k.m(i, 2))
        out.append(bound(i))
        out.append(bound(i, 3))
        out.append(k.two())
        out.append(k.two(i))
        out.append(k.two(i, i))
        out.append(k.exact(i, 2))
    return out


print(drive(5))

print("== mutable defaults are shared across calls ==")


class Bag:
    def add(self, v, bag=[]):
        bag.append(v)
        return len(bag)


b = Bag()
print([b.add(i) for i in range(5)])
print(Bag.add.__defaults__)
shared = []
print(b.add(1, shared), b.add(2, shared), b.add(3), b.add(4))
print(Bag.add.__defaults__)
print(Bag().add(0))

print("== default is the identical object ==")
sentinel = object()


class S:
    def get(self, a, b=sentinel):
        return b


s = S()
print(all(s.get(i) is sentinel for i in range(10)))

print("== __defaults__ reassignment / deletion ==")


class R:
    def f(self, a, b=1, c=2):
        return (a, b, c)


r = R()
for i in range(3):
    print(r.f(i))
R.f.__defaults__ = (10,)
for i in range(3):
    try:
        print(r.f(i))
    except TypeError as e:
        print("TypeError", e)
    print(r.f(i, 5))
R.f.__defaults__ = (7, 8)
print(r.f(0), r.f(0, 1), r.f(0, 1, 2))
R.f.__defaults__ = (4, 5, 6)
print(r.f(), r.f(1), r.f(1, 2), r.f(1, 2, 3))
R.f.__defaults__ = ()
try:
    r.f(1)
except TypeError as e:
    print("TypeError", e)
R.f.__defaults__ = None
try:
    r.f(1)
except TypeError as e:
    print("TypeError", e)
print(r.f(1, 2, 3))
del R.f.__defaults__
print(R.f.__defaults__)
try:
    r.f(1, 2)
except TypeError as e:
    print("TypeError", e)

print("== reassigned after being trampolined ==")


class L:
    def late(self, a, b=1):
        return a + b


l = L()
print(sum(l.late(i) for i in range(5)))
L.late.__defaults__ = (100,)
print(sum(l.late(i) for i in range(5)))
L.late.__defaults__ = None
try:
    l.late(1)
except TypeError as e:
    print("TypeError", e)

print("== too few / too many arguments ==")


class N:
    def need2(self, a, b, c=3):
        return (a, b, c)


n = N()
for args in [(), (1,), (1, 2), (1, 2, 3), (1, 2, 3, 4)]:
    try:
        print(n.need2(*args))
    except TypeError as e:
        print("TypeError", e)


def direct_calls():
    out = []
    try:
        out.append(n.need2())
    except TypeError as e:
        out.append(str(e))
    try:
        out.append(n.need2(1))
    except TypeError as e:
        out.append(str(e))
    out.append(n.need2(1, 2))
    try:
        out.append(n.need2(1, 2, 3, 4))
    except TypeError as e:
        out.append(str(e))
    bound = n.need2
    try:
        out.append(bound())
    except TypeError as e:
        out.append(str(e))
    try:
        out.append(bound(1))
    except TypeError as e:
        out.append(str(e))
    out.append(bound(1, 2))
    return out


for line in direct_calls():
    print(line)

print("== subclass overrides ==")


class Base:
    def m(self, x, y=1):
        return ("base", x, y)


class Child(Base):
    def m(self, x, y=2, z=3):
        return ("child", x, y, z)


class GrandChild(Child):
    def m(self, x):
        return ("grand", x)


for obj in (Base(), Child(), GrandChild()):
    print(obj.m(1))
    try:
        print(obj.m(1, 9))
    except TypeError as e:
        print("TypeError", e)

print("== super() with omitted defaults ==")


class P:
    def m(self, a, b=10):
        return (a, b)


class Q(P):
    def m(self, a, b=20):
        return super().m(a)


print(P().m(1), Q().m(1), Q().m(1, 2))

print("== classmethod / staticmethod / property / __call__ ==")


class D:
    @classmethod
    def c(cls, a, b=3):
        return (cls.__name__, a, b)

    @staticmethod
    def s(a, b=4):
        return (a, b)

    def __call__(self, a, b=5):
        return (a, b)

    def plain(self, a, b=6):
        return (a, b)


d = D()
print(d.c(1), D.c(1), d.s(1), D.s(1), d(1), d(1, 2), d.plain(1), D.plain(d, 1))

print("== instance attribute shadows method ==")
d.plain = lambda a, b=7: ("inst", a, b)
print(d.plain(1), D.plain(d, 1))
del d.plain
print(d.plain(1))

print("== method replaced on class after warm-up ==")


class Rep:
    def m(self, a, b=1):
        return ("old", a, b)


rep = Rep()
for i in range(3):
    print(rep.m(i))
Rep.m = lambda self, a, b=2: ("new", a, b)
for i in range(3):
    print(rep.m(i))

print("== defaults-only method (no required parameter) ==")


class A:
    def f(self, a=1, b=2):
        return (a, b)


a = A()


def drive_all_default():
    return [a.f(), a.f(5), a.f(5, 6)]


print(drive_all_default())

print("== keyword-only / variadic callees are unaffected ==")


class V:
    def kw(self, a, b=1, *, c=2):
        return (a, b, c)

    def va(self, a, b=1, *rest):
        return (a, b, rest)

    def kws(self, a, b=1, **kw):
        return (a, b, kw)


v = V()
print(v.kw(1), v.kw(1, 2), v.kw(1, c=5))
print(v.va(1), v.va(1, 2), v.va(1, 2, 3))
print(v.kws(1), v.kws(1, 2), v.kws(1, z=3))

print("== recursion with omitted defaults ==")


class Rec:
    def fact(self, n, acc=1):
        if n <= 1:
            return acc
        return self.fact(n - 1, acc * n)

    def depth(self, n, d=0):
        if n == 0:
            return d
        return self.depth(n - 1)


rc = Rec()
print(rc.fact(10), rc.fact(1), rc.fact(5, 2))
print(rc.depth(20), rc.depth(20, 5))

print("== exceptions raised from a defaulted callee ==")


class Z:
    def boom(self, a, b=0):
        return a // b


z = Z()
for i in range(3):
    try:
        z.boom(i)
    except ZeroDivisionError as e:
        print("ZeroDivisionError", e)
print(z.boom(10, 5))

print("== omitted-default call inside try / except ==")


def guarded():
    out = []
    for i in range(3):
        try:
            out.append(z.boom(i))
        except ZeroDivisionError:
            out.append("zde")
    return out


print(guarded())

print("== introspection inside a defaulted callee ==")

import sys


class F:
    def frame_locals(self, a, b=2):
        return sorted((k, v) for k, v in sys._getframe().f_locals.items() if k != "self")


f = F()
print(f.frame_locals(1))
print(f.frame_locals(1, 3))

print("== default captured by a closure ==")


class C:
    def make(self, a, b=5):
        def inner():
            return a + b

        return inner


print(C().make(1)(), C().make(1, 2)())

print("== __slots__ and __getattr__ receivers ==")


class Slotted:
    __slots__ = ("v",)

    def __init__(self):
        self.v = 3

    def m(self, a, b=4):
        return (self.v, a, b)


print(Slotted().m(1), Slotted().m(1, 2))


class Dyn:
    def __getattr__(self, name):
        return lambda a, b=8: (name, a, b)

    def real(self, a, b=9):
        return (a, b)


dy = Dyn()
print(dy.real(1), dy.virt(1), dy.virt(1, 2))
