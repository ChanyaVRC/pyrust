import asyncio
import builtins


def show(label, fn):
    try:
        res = fn()
        print(f"{label}: OK {res!r}")
    except BaseException as exc:
        print(f"{label}: EXC {type(exc).__name__} {exc.args!r}")


# --- builtins registration & metadata ---
print("is builtins.aiter:", aiter is builtins.aiter)
print("hasattr builtins:", hasattr(builtins, "aiter"))
print("in dir builtins:", "aiter" in dir(builtins))
print("repr:", repr(aiter))
print("name:", aiter.__name__)
print("qualname:", aiter.__qualname__)
print("module:", aiter.__module__)
print("self is builtins:", aiter.__self__ is builtins)


# --- arity, keyword, and non-async-iterable errors ---
show("0 args", lambda: aiter())
show("2 args", lambda: aiter(1, 2))
show("kwarg x", lambda: aiter(x=1))
show("kwarg async_iterable", lambda: aiter(async_iterable=1))
show("int", lambda: aiter(1))
show("str", lambda: aiter("abc"))
show("list", lambda: aiter([1, 2]))
show("dict", lambda: aiter({"a": 1}))
show("None", lambda: aiter(None))
show("sync gen", lambda: aiter(x for x in [1]))


async def plain_coro():
    return 1


pc = plain_coro()
show("coro", lambda: aiter(pc))
pc.close()


class PlainObj:
    pass


show("plain instance", lambda: aiter(PlainObj()))
show("plain class", lambda: aiter(PlainObj))


class MetaNoAI(type):
    pass


class ClsNoAI(metaclass=MetaNoAI):
    pass


show("metaclass no __aiter__", lambda: aiter(ClsNoAI))


# --- instance-only __aiter__ must be ignored ---
class InstOnly:
    pass


io_obj = InstOnly()
io_obj.__aiter__ = lambda: io_obj
show("instance-only __aiter__", lambda: aiter(io_obj))


# --- non-callable / None __aiter__ ---
class NonCallableAiter:
    __aiter__ = 123


show("non-callable __aiter__", lambda: aiter(NonCallableAiter()))


class NoneAiter:
    __aiter__ = None


show("None __aiter__", lambda: aiter(NoneAiter()))


# --- __aiter__ raising exception ---
class RaisingAiter:
    def __aiter__(self):
        raise ValueError("aiter boom")


show("raising __aiter__", lambda: aiter(RaisingAiter()))


# --- invalid return types (PyAIter_Check) ---
class BadRetInt:
    def __aiter__(self):
        return 42


show("bad ret int", lambda: aiter(BadRetInt()))


class NoAnext:
    def __aiter__(self):
        return self


show("bad ret no __anext__", lambda: aiter(NoAnext()))


class InstAnextOnly:
    def __init__(self):
        self.__anext__ = lambda: 1


class ReturnsInstAnextOnly:
    def __aiter__(self):
        return InstAnextOnly()


show("bad ret instance-only __anext__", lambda: aiter(ReturnsInstAnextOnly()))


class RetMeta(type):
    pass


class RetCls(metaclass=RetMeta):
    pass


class ReturnsMetaCls:
    def __aiter__(self):
        return RetCls


show("bad ret class with custom metaclass", lambda: aiter(ReturnsMetaCls()))


# --- PyAIter_Check accepts any class-level __anext__ slot (even None / non-callable) ---
class NoneAnextRet:
    __anext__ = None


class ReturnsNoneAnext:
    def __aiter__(self):
        return NoneAnextRet()


print("ret __anext__=None type:", type(aiter(ReturnsNoneAnext())).__name__)


class IntAnextRet:
    __anext__ = 123


class ReturnsIntAnext:
    def __aiter__(self):
        return IntAnextRet()


print("ret __anext__=123 type:", type(aiter(ReturnsIntAnext())).__name__)


# --- descriptor, staticmethod, classmethod, and metaclass __aiter__ ---
class GoodAsyncIter:
    def __init__(self, items):
        self._items = list(items)

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self._items:
            return self._items.pop(0)
        raise StopAsyncIteration


class AiterDescriptor:
    def __get__(self, obj, owner):
        return lambda: GoodAsyncIter([100])


class WithDescriptor:
    __aiter__ = AiterDescriptor()


class WithStatic:
    @staticmethod
    def __aiter__():
        return GoodAsyncIter([200])


class WithClassMethod:
    @classmethod
    def __aiter__(cls):
        return GoodAsyncIter([cls.__name__])


class MetaBadAI(type):
    def __aiter__(cls):
        return "not-an-async-iter"


class ClsMetaBadAI(metaclass=MetaBadAI):
    pass


show("metaclass bad ret", lambda: aiter(ClsMetaBadAI))


class MetaGoodAI(type):
    def __aiter__(cls):
        return cls

    async def __anext__(cls):
        return f"meta:{cls.__name__}"


class ClsMetaGoodAI(metaclass=MetaGoodAI):
    pass


print("metaclass good aiter is cls:", aiter(ClsMetaGoodAI) is ClsMetaGoodAI)


# --- async generator & custom iterable/iterator end-to-end ---
async def sample_agen():
    yield "x"
    await asyncio.sleep(0)
    yield "y"


class CustomAsyncIterable:
    def __init__(self, items):
        self._items = items

    def __aiter__(self):
        return GoodAsyncIter(self._items)


async def run_async_tests():
    ag = sample_agen()
    it = aiter(ag)
    print("agen identity:", it is ag)
    print("agen 1:", await anext(it))
    print("agen 2:", await anext(it))
    print("agen done:", await anext(it, "END"))

    ci = CustomAsyncIterable([1, 2, 3])
    cit = aiter(ci)
    collected = []
    async for v in cit:
        collected.append(v)
    print("custom async for:", collected)

    print("descriptor item:", await anext(aiter(WithDescriptor())))
    print("staticmethod item:", await anext(aiter(WithStatic())))
    print("classmethod item:", await anext(aiter(WithClassMethod())))
    print("metaclass item:", await anext(aiter(ClsMetaGoodAI)))


asyncio.run(run_async_tests())
