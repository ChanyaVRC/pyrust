"""contextlib ABCs / decorators / async support (issue #2795)."""

import asyncio
from contextlib import (
    AbstractContextManager,
    AbstractAsyncContextManager,
    ContextDecorator,
    asynccontextmanager,
    aclosing,
    AsyncExitStack,
)


# ── AbstractContextManager ──────────────────────────────────────────────────
class MyCtx(AbstractContextManager):
    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False


with MyCtx() as ctx:
    print(isinstance(ctx, AbstractContextManager))

# default __enter__ returns self
class OnlyExit(AbstractContextManager):
    def __exit__(self, *args):
        return False


oe = OnlyExit()
with oe as got:
    print(got is oe)


# structural __subclasshook__ — virtual subclass without inheritance
class SimpleCtx:
    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False


print(isinstance(SimpleCtx(), AbstractContextManager))
print(issubclass(SimpleCtx, AbstractContextManager))


class NotACtx:
    pass


print(isinstance(NotACtx(), AbstractContextManager))


# ── AbstractAsyncContextManager ─────────────────────────────────────────────
class AsyncSimple:
    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return False


print(issubclass(AsyncSimple, AbstractAsyncContextManager))
print(issubclass(SimpleCtx, AbstractAsyncContextManager))


# ── ContextDecorator ────────────────────────────────────────────────────────
log = []


class Tracking(ContextDecorator):
    def __enter__(self):
        log.append("enter")
        return self

    def __exit__(self, *args):
        log.append("exit")
        return False


@Tracking()
def decorated(x):
    log.append("body")
    return x * 2


print(decorated(21))
print(log)

log.clear()
with Tracking():
    log.append("ctx")
print(log)


# ── asynccontextmanager ─────────────────────────────────────────────────────
events = []


@asynccontextmanager
async def managed(x):
    events.append("setup")
    try:
        yield x * 2
    finally:
        events.append("cleanup")


@asynccontextmanager
async def suppressing():
    try:
        yield
    except ValueError:
        pass


async def main():
    async with managed(5) as v:
        print(v)
    print(events)

    # exception forwarded into the async generator and suppressed
    async with suppressing():
        raise ValueError("boom")
    print("suppressed in async cm")

    # AsyncExitStack with sync + async context managers
    order = []

    class SyncCM:
        def __enter__(self):
            order.append("sync enter")
            return "S"

        def __exit__(self, *a):
            order.append("sync exit")
            return False

    class AsyncCM:
        async def __aenter__(self):
            order.append("async enter")
            return "A"

        async def __aexit__(self, *a):
            order.append("async exit")
            return False

    async with AsyncExitStack() as stack:
        a = stack.enter_context(SyncCM())
        b = await stack.enter_async_context(AsyncCM())
        order.append(f"body {a} {b}")
    print(order)

    # callback ordering (LIFO)
    cbs = []
    async with AsyncExitStack() as stack:
        stack.callback(cbs.append, "first")
        stack.push_async_callback(_async_append, cbs, "second")
    print(cbs)

    # aclosing on an async generator
    async def agen():
        try:
            yield 1
            yield 2
        finally:
            events.append("agen closed")

    g = agen()
    async with aclosing(g) as ag:
        print(await ag.__anext__())
    print(events[-1])


async def _async_append(lst, val):
    lst.append(val)


asyncio.run(main())
print("contextlib abc ok")
