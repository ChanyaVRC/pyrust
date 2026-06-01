# Class decorators must receive the constructed class object, not the
# decorator function, regardless of whether the class declares a base
# (issue #1889).  Previously a decorated class with any explicit base had its
# class register clobbered by the decorator value during code generation.

def deco(cls):
    print("deco got", cls.__name__)
    return cls


def deco_arg(tag):
    def inner(cls):
        print("deco_arg", tag, "->", cls.__name__)
        return cls
    return inner


class Wrapper:
    def __init__(self, wrapped):
        self.wrapped = wrapped
        print("wrapped", wrapped.__name__)


def wrap(cls):
    return Wrapper(cls)


def d1(cls):
    print("d1", cls.__name__)
    return cls


def d2(cls):
    print("d2", cls.__name__)
    return cls


class Base:
    pass


class A:
    pass


class B:
    pass


class Meta(type):
    pass


# Single base.
@deco
class Sub(Base):
    pass


# No base (must stay correct).
@deco
class Plain:
    pass


# Multiple bases.
@deco
class Multi(A, B):
    pass


# Keyword / metaclass with a base.
@deco
class WithMeta(Base, metaclass=Meta):
    x = 7


# Stacked decorators on a based class: applied innermost-first, d1(d2(C)).
@d1
@d2
class Stacked(Base):
    pass


# Decorator with arguments on a based class.
@deco_arg("hi")
class Argd(Base):
    pass


# Decorator that returns a different object: the name binds to the return value.
@wrap
class Wrapped(Base):
    pass


# Introspection + instantiation of the decorated subclass.
print(Sub.__name__, [b.__name__ for b in Sub.__bases__])
print(issubclass(Sub, Base), isinstance(Sub(), Base))
print(Multi.__bases__[0].__name__, Multi.__bases__[1].__name__)
print(WithMeta.x, type(WithMeta).__name__)
print(Stacked.__name__, issubclass(Stacked, Base))
print(isinstance(Wrapped, Wrapper), Wrapped.wrapped.__name__)
