import asyncio
import builtins
import collections.abc


def show(label, fn):
    try:
        res = fn()
        print(f"{label}: OK {res!r}")
    except BaseException as exc:
        print(f"{label}: EXC {type(exc).__name__} {exc.args!r}")


# --- builtins registration & metadata ---
print("is builtins.anext:", anext is builtins.anext)
print("hasattr builtins:", hasattr(builtins, "anext"))
print("in dir builtins:", "anext" in dir(builtins))
print("repr:", repr(anext))
print("name:", anext.__name__)
print("module:", anext.__module__)


# --- arity, keyword, and type errors ---
show("0 args", lambda: anext())
show("3 args", lambda: anext(1, 2, 3))
show("kwarg", lambda: anext(x=1))
show("int 1-arg", lambda: anext(1))
show("int 2-arg", lambda: anext(1, 99))
show("list 1-arg", lambda: anext([1, 2]))
show("sync gen", lambda: anext(x for x in [1]))


async def plain_coro():
    return 1


pc = plain_coro()
show("coro 1-arg", lambda: anext(pc))
pc.close()


# --- instance-only __anext__ vs class-level __anext__ ---
class InstOnly:
    pass


io = InstOnly()


async def fake_anext():
    return 1


io.__anext__ = fake_anext
show("instance-only __anext__", lambda: anext(io))


class NonCallableAnext:
    __anext__ = 123


show("non-callable __anext__ 1-arg", lambda: anext(NonCallableAnext()))
show("non-callable __anext__ 2-arg", lambda: anext(NonCallableAnext(), "def"))


class NoneAnext:
    __anext__ = None


show("None __anext__ 1-arg", lambda: anext(NoneAnext()))


# --- metaclass-defined __anext__ ---
class MetaAI(type):
    async def __anext__(cls):
        return f"meta:{cls.__name__}"


class ClassAI(metaclass=MetaAI):
    pass


# --- raw non-awaitable return from __anext__ ---
class BadAnext:
    def __anext__(self):
        return 42


print("1-arg raw non-awaitable:", anext(BadAnext()))
aw_bad = anext(BadAnext(), "def")
print("2-arg bad type:", type(aw_bad).__name__)
show("bad send(None)", lambda: aw_bad.send(None))
show("bad next()", lambda: next(aw_bad))
show("bad throw(VE)", lambda: aw_bad.throw(ValueError("x")))
show("bad close()", lambda: aw_bad.close())


# --- async generator 1-arg and 2-arg ---
async def sample_agen():
    yield 10
    await asyncio.sleep(0)
    yield 20


class SubSAI(StopAsyncIteration):
    pass


class CustomAsyncIter:
    def __init__(self, items, err=None):
        self._items = list(items)
        self._err = err

    def __aiter__(self):
        return self

    async def __anext__(self):
        await asyncio.sleep(0)
        if self._items:
            return self._items.pop(0)
        if self._err is not None:
            raise self._err
        raise StopAsyncIteration()


async def run_async_tests():
    ag = sample_agen()
    print("1-arg first:", await anext(ag))
    print("1-arg second:", await anext(ag))
    try:
        await anext(ag)
    except StopAsyncIteration as exc:
        print("1-arg exhausted:", type(exc).__name__, exc.args)

    ag2 = sample_agen()
    print("2-arg first:", await anext(ag2, "DONE"))
    print("2-arg second:", await anext(ag2, "DONE"))
    print("2-arg default:", await anext(ag2, "DONE"))
    print("2-arg default None:", await anext(ag2, None))

    # Default is a coroutine — must be returned un-awaited!
    async def sentinel_coro():
        return "SHOULD_NOT_RUN"

    dc = sentinel_coro()
    got = await anext(ag2, dc)
    print("default coro unawaited:", got is dc)
    dc.close()

    # Custom async iterator + StopAsyncIteration subclass
    ci = CustomAsyncIter(["a", "b"], err=SubSAI("sub"))
    print("custom 1:", await anext(ci, "FB"))
    print("custom 2:", await anext(ci, "FB"))
    print("custom SubSAI:", await anext(ci, "FB"))

    # Non-StopAsyncIteration exception propagates through 2-arg anext
    ci_err = CustomAsyncIter([], err=ValueError("boom"))
    try:
        await anext(ci_err, "FB")
    except ValueError as exc:
        print("custom ValueError:", exc.args)

    # Metaclass __anext__
    print("meta 1-arg:", await anext(ClassAI))
    print("meta 2-arg:", await anext(ClassAI, "FB"))


asyncio.run(run_async_tests())


# --- anext_awaitable protocol details ---
ag_p = sample_agen()
aw = anext(ag_p, "DEF")
print("type name:", type(aw).__name__)
print("repr prefix:", repr(aw).startswith("<anext_awaitable object at 0x"), repr(aw).endswith(">"))
print("__await__ is self:", aw.__await__() is aw)
print("__iter__ is self:", iter(aw) is aw)
print("hasattr protocol:", [hasattr(aw, n) for n in ("__await__", "__iter__", "__next__", "send", "throw", "close", "gi_frame")])
print("abc Awaitable:", isinstance(aw, collections.abc.Awaitable))
print("abc Iterator:", isinstance(aw, collections.abc.Iterator))
print("abc Iterable:", isinstance(aw, collections.abc.Iterable))
print("abc Coroutine:", isinstance(aw, collections.abc.Coroutine))
show("aw.send(None)", lambda: aw.send(None))
show("aw.send(None) reuse", lambda: aw.send(None))


class SharedAw:
    def __init__(self):
        def g():
            v = yield "step1"
            if v == "sai":
                raise StopAsyncIteration()
            return ("ok", v)

        self.g = g()

    def __await__(self):
        return self.g


class AIWrap:
    def __init__(self, wrapped):
        self.wrapped = wrapped

    def __anext__(self):
        return self.wrapped


aw_s1 = anext(AIWrap(SharedAw()), "FB")
show("shared next", lambda: next(aw_s1))
show("shared send ok", lambda: aw_s1.send("val"))

aw_s2 = anext(AIWrap(SharedAw()), "FB")
show("shared2 send None", lambda: aw_s2.send(None))
show("shared2 send sai", lambda: aw_s2.send("sai"))

aw_s3 = anext(AIWrap(SharedAw()), "FB")
show("shared3 send None", lambda: aw_s3.send(None))
show("shared3 throw SAI", lambda: aw_s3.throw(StopAsyncIteration()))

aw_s4 = anext(AIWrap(SharedAw()), "FB")
show("shared4 send None", lambda: aw_s4.send(None))
show("shared4 throw VE", lambda: aw_s4.throw(ValueError("boom")))

aw_s5 = anext(AIWrap(SharedAw()), "FB")
show("shared5 pre-start throw SAI", lambda: aw_s5.throw(StopAsyncIteration()))

aw_s6 = anext(AIWrap(SharedAw()), "FB")
show("shared6 pre-start throw VE", lambda: aw_s6.throw(ValueError("v")))

aw_s7 = anext(AIWrap(SharedAw()), "FB")
show("shared7 send None", lambda: aw_s7.send(None))
show("shared7 close", lambda: aw_s7.close())

aw_s8 = anext(AIWrap(SharedAw()), "FB")
show("shared8 pre-start close", lambda: aw_s8.close())
