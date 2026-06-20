class Base:
    pass


class Proxy:
    def __mro_entries__(self, bases):
        return (Base,)


# Non-class base with __mro_entries__
class C(Proxy()):
    pass


print(C.__bases__ == (Base,))        # True
print(hasattr(C, '__orig_bases__'))  # True
print(isinstance(C(), Base))         # True


# Existing class bases still work, no __orig_bases__
class D(Base):
    pass


print(D.__bases__ == (Base,))        # True
print(hasattr(D, '__orig_bases__'))  # False


# Multiple bases, some with __mro_entries__
class Other:
    pass


class E(Proxy(), Other):
    pass


print(Base in E.__mro__)             # True
print(E.__bases__ == (Base, Other))  # True


# __mro_entries__ returning empty tuple drops the entry
class Empty:
    def __mro_entries__(self, bases):
        return ()


class F(Empty(), Base):
    pass


print(F.__bases__ == (Base,))        # True


# __orig_bases__ holds the original (non-class) bases
proxy = Proxy()


class G(proxy):
    pass


print(G.__orig_bases__ == (proxy,))  # True


# __mro_entries__ receives the full original bases tuple
class Recorder:
    seen = None

    def __mro_entries__(self, bases):
        Recorder.seen = bases
        return (Base,)


r = Recorder()


class H(r, Other):
    pass


print(Recorder.seen == (r, Other))   # True
