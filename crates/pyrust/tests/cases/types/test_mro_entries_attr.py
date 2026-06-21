# PEP 560: __mro_entries__ is resolved via a regular attribute access on the
# base object (CPython's _PyObject_LookupAttr), so it is honored when set as an
# instance attribute or served through __getattr__ — not only as a type slot.


class Base:
    pass


# Case 1: __mro_entries__ as an instance attribute.
class Proxy:
    pass


p = Proxy()
p.__mro_entries__ = lambda bases: (Base,)


class FromInstanceAttr(p):
    pass


print(FromInstanceAttr.__bases__)
print(FromInstanceAttr.__orig_bases__ == (p,))


# Case 2: __mro_entries__ served through __getattr__.
class GetattrProxy:
    def __getattr__(self, name):
        if name == "__mro_entries__":
            return lambda bases: (Base,)
        raise AttributeError(name)


class FromGetattr(GetattrProxy()):
    pass


print(FromGetattr.__bases__)


# The resolver receives the *full* original bases tuple.
class Recorder:
    def __mro_entries__(self, bases):
        print("got", len(bases), "bases")
        return (Base,)


r = Recorder()
proxy = Proxy()
proxy.__mro_entries__ = lambda bases: ()


class MultiBase(r, proxy):
    pass


print(MultiBase.__bases__)


# A genuine non-class base with no __mro_entries__ anywhere still fails.
try:

    class Bad(5):
        pass

except Exception as e:
    print(type(e).__name__ in ("TypeError", "RuntimeError"))


# A non-AttributeError raised while looking up __mro_entries__ propagates.
class Boom:
    def __getattr__(self, name):
        raise ValueError("boom")


try:

    class FromBoom(Boom()):
        pass

except ValueError as e:
    print("propagated", e)
