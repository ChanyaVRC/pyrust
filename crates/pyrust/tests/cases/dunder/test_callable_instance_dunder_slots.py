# Issue #2054: a protocol / rich-cmp / numeric dunder set to a *callable
# instance* (an object whose class defines `__call__`) is invoked, matching
# CPython.  A callable instance is not a descriptor, so it does NOT receive the
# receiver as `self`: `__len__ = Caller()` calls `Caller()()` with no implicit
# self, `__add__ = Caller()` calls `Caller()(other)`.


class CallerLen:
    def __call__(self, *a):
        return 42


class D:
    __len__ = CallerLen()


print(len(D()))


class CallerAdd:
    def __call__(self, *a):
        return ("called", a)


class A:
    __add__ = CallerAdd()


print(A() + 99)


class CallerIter:
    def __call__(self, *a):
        return iter([7, 8])


class E:
    __iter__ = CallerIter()


print(list(E()))


class CallerEq:
    def __call__(self, *a):
        return True


class C:
    __eq__ = CallerEq()


print(C() == C())


class CallerHash:
    def __call__(self, *a):
        return 1234


class H:
    __hash__ = CallerHash()


print(hash(H()))


class CallerLt:
    def __call__(self, *a):
        return True


class Cmp:
    __lt__ = CallerLt()


print(Cmp() < Cmp())
