"""contextlib async surface: asynccontextmanager, AsyncExitStack, aclosing, ... (issue #3388)."""

import asyncio
import contextlib
from contextlib import (
    AbstractAsyncContextManager,
    AsyncContextDecorator,
    AsyncExitStack,
    ExitStack,
    aclosing,
    asynccontextmanager,
    nullcontext,
)

events = []


async def tick():
    await asyncio.sleep(0)


# ── AbstractAsyncContextManager ─────────────────────────────────────────────
class NoAexit(AbstractAsyncContextManager):
    pass


try:
    NoAexit()
except TypeError as exc:
    print("abstract:", exc)


class WithAexit(AbstractAsyncContextManager):
    async def __aexit__(self, *exc):
        return None


# ── asynccontextmanager ─────────────────────────────────────────────────────
@asynccontextmanager
async def managed(x, *, y=0):
    """managed doc"""
    events.append(("setup", x, y))
    await tick()
    try:
        yield x + y
    finally:
        await tick()
        events.append("teardown")


print(managed.__name__, managed.__doc__, managed.__wrapped__.__name__)


@asynccontextmanager
async def swallowing():
    try:
        yield
    except ValueError:
        events.append("swallowed")


@asynccontextmanager
async def never_yields():
    return
    yield


@asynccontextmanager
async def yields_twice():
    yield 1
    yield 2


@asynccontextmanager
async def yields_twice_after_throw():
    try:
        yield 1
    except ValueError:
        yield 2


@asynccontextmanager
async def replaces():
    try:
        yield
    except ValueError as exc:
        raise KeyError("replaced") from exc


class Holder:
    @asynccontextmanager
    async def ctx(self, tag):
        yield (type(self).__name__, tag)


class Decorating(AsyncContextDecorator):
    async def __aenter__(self):
        events.append("deco enter")
        return self

    async def __aexit__(self, *exc):
        events.append("deco exit")
        return False


@Decorating()
async def decorated(a, b=1):
    await tick()
    return a + b


class Obj:
    @Decorating()
    async def method(self, n):
        return (type(self).__name__, n)


async def main():
    async with managed(1, y=2) as value:
        print(value)
    print(events)
    cm = managed(5)
    print(type(cm).__name__, cm.__doc__, isinstance(cm, AbstractAsyncContextManager))

    events.clear()
    async with swallowing():
        raise ValueError("x")
    print(events)

    try:
        async with never_yields():
            pass
    except RuntimeError as exc:
        print(exc, exc.__cause__, exc.__suppress_context__)

    try:
        async with yields_twice():
            pass
    except RuntimeError as exc:
        print(exc)

    try:
        async with yields_twice_after_throw():
            raise ValueError("x")
    except RuntimeError as exc:
        print(exc)

    try:
        async with replaces():
            raise ValueError("orig")
    except KeyError as exc:
        print("replaced", repr(exc.__cause__))

    try:
        async with managed(1):
            raise StopAsyncIteration("body")
    except StopAsyncIteration as exc:
        print("StopAsyncIteration propagated", exc)

    async with Holder().ctx("t") as tag:
        print(tag)

    # as a decorator
    @managed(3)
    async def deco_target(n):
        return n * 2

    events.clear()
    print(await deco_target(1), await deco_target(2), events)

    events.clear()
    print(await decorated(1), await decorated(2, b=5), events)
    print(decorated.__name__, await Obj().method(4))

    # re-entering
    cm = managed(1)
    async with cm:
        pass
    try:
        async with cm:
            pass
    except AttributeError as exc:
        print("AttributeError", exc)

    # ── aclosing ────────────────────────────────────────────────────────────
    async def agen():
        try:
            yield 1
            yield 2
        finally:
            events.append("agen closed")

    events.clear()
    async with aclosing(agen()) as g:
        print(await g.__anext__())
    print(events)
    print(aclosing.__mro__[1] is AbstractAsyncContextManager)

    # ── nullcontext ─────────────────────────────────────────────────────────
    async with nullcontext(9) as n:
        print(n)
    print(await nullcontext(3).__aenter__(), await nullcontext().__aexit__(None, None, None))
    with nullcontext(4) as n:
        print(n)

    # ── AsyncExitStack ──────────────────────────────────────────────────────
    order = []

    class SyncCM:
        def __enter__(self):
            order.append("sync enter")
            return "S"

        def __exit__(self, *a):
            order.append("sync exit")
            return False

    class AsyncCM:
        def __init__(self, name, suppress_exc=False):
            self.name = name
            self.suppress_exc = suppress_exc

        async def __aenter__(self):
            await tick()
            order.append("enter " + self.name)
            return self.name

        async def __aexit__(self, typ, exc, tb):
            await tick()
            order.append(("exit " + self.name, typ.__name__ if typ else None))
            return self.suppress_exc

    async def async_cb(*a, **k):
        await tick()
        order.append(("async_cb", a, k))
        return "ignored"

    async with AsyncExitStack() as stack:
        print(stack.enter_context(SyncCM()))
        print(await stack.enter_async_context(AsyncCM("one")))
        print(stack.push_async_callback(async_cb, 1, k=2) is async_cb)
        stack.callback(order.append, "sync cb")
        print(len(stack._exit_callbacks), [flag for flag, _ in stack._exit_callbacks])
    print(order)

    # suppression
    order.clear()
    async with AsyncExitStack() as stack:
        await stack.enter_async_context(AsyncCM("suppress", True))
        raise ValueError("gone")
    print("suppressed", order)

    # push / push_async_exit with exit functions and CM objects
    order.clear()

    async def aexit_fn(typ, exc, tb):
        order.append(("aexit_fn", typ.__name__ if typ else None))
        return True

    async with AsyncExitStack() as stack:
        print(stack.push_async_exit(aexit_fn) is aexit_fn)
        cm = AsyncCM("pushed")
        print(stack.push_async_exit(cm) is cm)
        print(stack.push(SyncCM()) is not None)
        raise ValueError("boom")
    print(order)

    # pop_all / aclose
    order.clear()
    async with AsyncExitStack() as stack:
        stack.callback(order.append, "kept")
        moved = stack.pop_all()
        print(len(stack._exit_callbacks), len(moved._exit_callbacks), type(moved) is AsyncExitStack)
    print(order)
    await moved.aclose()
    print(order)
    await moved.aclose()

    # errors
    try:
        await AsyncExitStack().enter_async_context(object())
    except TypeError as exc:
        print(exc)
    try:
        await AsyncExitStack().enter_async_context(SyncCM())
    except TypeError as exc:
        print(exc)

    # exception chaining
    def raise_exit(label):
        async def inner(typ, exc, tb):
            raise KeyError(label)

        return inner

    try:
        async with AsyncExitStack() as stack:
            stack.push_async_exit(raise_exit("outer"))
            stack.push_async_exit(raise_exit("inner"))
            raise ValueError("body")
    except KeyError as exc:
        chain = []
        cur = exc
        while cur is not None:
            chain.append((type(cur).__name__, cur.args))
            cur = cur.__context__
        print(chain)

    # every callback runs even if one raises
    order.clear()
    try:
        async with AsyncExitStack() as stack:
            stack.callback(order.append, "first")
            stack.push_async_callback(async_cb, "second")
            stack.callback(lambda: 1 / 0)
            stack.callback(order.append, "last")
    except ZeroDivisionError:
        print("zde", order)

    # the coroutines are real awaitables for asyncio
    async def aexit_value():
        return 42

    stack = AsyncExitStack()
    stack.push_async_callback(aexit_value)
    task = asyncio.ensure_future(stack.aclose())
    print(await task)
    print(await asyncio.gather(nullcontext(1).__aenter__(), nullcontext(2).__aenter__()))
    print(await asyncio.wait_for(AsyncExitStack().__aenter__(), 1) is not None)

    # a coroutine can only be awaited once
    coro = nullcontext(1).__aenter__()
    print(await coro)
    try:
        await coro
    except RuntimeError as exc:
        print(exc)

    # cancellation unwinds the stack
    order.clear()

    async def slow_body():
        async with AsyncExitStack() as stack:
            await stack.enter_async_context(AsyncCM("slow"))
            await asyncio.sleep(10)

    t = asyncio.ensure_future(slow_body())
    await asyncio.sleep(0.01)
    t.cancel()
    try:
        await t
    except asyncio.CancelledError:
        print("cancelled", order)


asyncio.run(main())
print("contextlib async ok")
