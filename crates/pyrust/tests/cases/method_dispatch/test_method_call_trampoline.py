# Method-call trampoline (#2344): o.m() and bound-method f() calls loop in the
# VM dispatch loop with the receiver bound to `self`, instead of re-entering the
# native call machinery.  This fixture pins the observable semantics the
# trampoline must preserve: receiver binding, argument shifting, method
# resolution (shadowing, monkeypatching, classmethod/staticmethod, super(),
# inheritance, __slots__, property), exceptions escaping a trampolined frame,
# recursion, and the gate fallbacks (kwargs / *args / generators / coroutines).


class Acc:
    def __init__(self, base):
        self.base = base

    def m0(self):
        return self.base

    def m1(self, a):
        return self.base + a

    def m3(self, a, b, c):
        return self.base + a + b + c

    def me(self):
        return self


a = Acc(10)
# Zero/one/three-arg trampolined calls (receiver bound to self, args shifted).
print(a.m0(), a.m1(5), a.m3(1, 2, 3))
# Method that returns self — receiver identity must round-trip.
print(a.me() is a)

# Pre-bound method called through Insn::Call (the BoundMethod arm).
f = a.m1
print(f(7), f(8))

# Hot loop: exercises the trampoline repeatedly and checks no state leaks.
total = 0
for i in range(1000):
    total += a.m1(i)
print(total)


# Instance attribute shadowing a method: the shadow wins (cache must miss).
class Sh:
    def go(self):
        return "method"


s = Sh()
print(s.go())
s.go = lambda: "shadow"
print(s.go())


# Monkeypatched class method picked up on the next call (class version bump).
class Mp:
    def v(self):
        return 1


m = Mp()
print(m.v())
Mp.v = lambda self: 2
print(m.v())


# classmethod / staticmethod are not trampolined (gate falls back).
class Cs:
    @classmethod
    def cm(cls):
        return cls.__name__

    @staticmethod
    def sm(x):
        return x * 2


print(Cs.cm(), Cs().cm(), Cs.sm(21))


# super() chain.
class Base:
    def who(self):
        return "Base"


class Mid(Base):
    def who(self):
        return "Mid->" + super().who()


print(Mid().who())


# Inheritance depth 3: the method resolves on a distant ancestor.
class D1:
    def deep(self):
        return 99


class D2(D1):
    pass


class D3(D2):
    pass


print(D3().deep())


# __slots__ class.
class Slotted:
    __slots__ = ("v",)

    def __init__(self):
        self.v = 7

    def get(self):
        return self.v


print(Slotted().get())


# property accessed inside a method body.
class Prop:
    def __init__(self):
        self._n = 3

    @property
    def n(self):
        return self._n

    def use(self):
        return self.n + 1


print(Prop().use(), Prop().n)


# Exception raised inside a trampolined method escapes to the caller's handler.
class Boom:
    def boom(self):
        raise ValueError("kaboom")


try:
    Boom().boom()
except ValueError as e:
    print("caught", e)


# Recursion through a method (depth limit / frame accounting).
class Fib:
    def fib(self, n):
        if n < 2:
            return n
        return self.fib(n - 1) + self.fib(n - 2)


print(Fib().fib(20))


# Method calling another method on self (nested trampolined frames).
class Chain:
    def a(self):
        return self.b() + 1

    def b(self):
        return self.c() + 1

    def c(self):
        return 40


print(Chain().a())


# Gate fallbacks: kwargs, defaults, *args still bind correctly.
class Variadic:
    def kw(self, a, b=100):
        return a + b

    def star(self, *args):
        return sum(args)


v = Variadic()
print(v.kw(1), v.kw(1, b=2), v.kw(1, 3), v.star(1, 2, 3, 4))


# A method that is itself a generator must NOT be trampolined (builds a gen).
class Gen:
    def count(self, n):
        i = 0
        while i < n:
            yield i
            i += 1


print(list(Gen().count(4)))


# Wrong arity through a method still raises the CPython-matching TypeError.
try:
    a.m1(1, 2)
except TypeError as e:
    print(type(e).__name__)
