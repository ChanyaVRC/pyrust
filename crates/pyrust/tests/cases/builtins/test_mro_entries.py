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


# PEP 560: __mro_entries__ resolves under an explicit metaclass= too, and the
# metaclass observes the resolved bases plus __orig_bases__ in its namespace.
class Meta(type):
    def __new__(mcs, name, bases, ns, **kw):
        print(bases == (Base,))                       # True
        print(ns.get('__orig_bases__') is not None)   # True
        return super().__new__(mcs, name, bases, ns)


p2 = Proxy()


class I(p2, metaclass=Meta):
    pass


print(type(I) is Meta)                # True
print(I.__bases__ == (Base,))         # True
print(I.__orig_bases__ == (p2,))      # True
print(isinstance(I(), Base))          # True


# PEP 560: a base carrying an inherited custom metaclass via __mro_entries__
# routes through that metaclass and still records __orig_bases__.
class MetaB(type):
    pass


class HasMetaB(metaclass=MetaB):
    pass


class ProxyMeta:
    def __mro_entries__(self, bases):
        return (HasMetaB,)


pm = ProxyMeta()


class J(pm):
    pass


print(type(J) is MetaB)               # True
print(J.__bases__ == (HasMetaB,))     # True
print(J.__orig_bases__ == (pm,))      # True
