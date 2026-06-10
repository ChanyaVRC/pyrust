# Re-entrancy ("already executing/running") error parity (issue #2285).
#
# CPython 3.12 wording matrix:
#   - coroutine awaited while executing (self-await chain)
#       -> ValueError: coroutine already executing
#   - sync generator re-entered (next/list/yield-from while running)
#       -> ValueError: generator already executing
#   - async generator stepped while running (__anext__/asend)
#       -> RuntimeError: anext(): asynchronous generator is already running
#
# pyrust additionally drove some of these through panics ("interpreter thread
# panicked") or "invalid generator state" before #2285: a re-entrant next()
# on a natively-resumed generator panicked on the RefCell re-borrow, and the
# gen-drive trampoline (#2253) left a placeholder the send/next/collect paths
# misreported. Every case below prints the exception type and message.

import asyncio


def show(fn):
    try:
        fn()
        print("no error")
    except BaseException as e:
        print(type(e).__name__, "-", e)


# -- coroutine self-await ----------------------------------------------------

async def self_await():
    await c


c = self_await()
show(lambda: asyncio.run(c))


# -- coroutine self-await via a helper chain ---------------------------------

async def helper():
    await c2


async def outer():
    await helper()


c2 = outer()
show(lambda: asyncio.run(c2))


# -- sync generator: re-entrant next() while natively resumed ----------------

def g_next():
    next(it_next)
    yield 1


it_next = g_next()
show(lambda: next(it_next))


# -- sync generator: re-entrant next() while trampoline-driven (for-loop) ----

def g_tramp():
    next(it_tramp)
    yield 1


it_tramp = g_tramp()


def drive_tramp():
    for _ in it_tramp:
        pass


show(drive_tramp)


# -- sync generator: re-entrant send() while natively resumed ----------------

def g_send():
    it_send.send(None)
    yield 1


it_send = g_send()
show(lambda: next(it_send))


# -- sync generator: re-entrant list() collect while natively resumed --------

def g_collect():
    list(it_collect)
    yield 1


it_collect = g_collect()
show(lambda: next(it_collect))


# -- sync generator: re-entrant throw() while trampoline-driven --------------

it_throw = None


def g_throw():
    it_throw.throw(ValueError("x"))
    yield 1


it_throw = g_throw()


def drive_throw():
    for _ in it_throw:
        pass


show(drive_throw)


# -- sync generator: re-entrant close() while trampoline-driven --------------

it_close = None


def g_close():
    it_close.close()
    yield 1


it_close = g_close()


def drive_close():
    for _ in it_close:
        pass


show(drive_close)


# -- sync generator: re-entrant consume via genexpr sum() while driven -------

def g_sum():
    sum(x for x in it_sum)
    yield 1


it_sum = g_sum()


def drive_sum():
    for _ in it_sum:
        pass


show(drive_sum)


# -- async generator: re-entrant __anext__ / asend while running -------------

a1 = None


async def anext_helper():
    return await a1.__anext__()


async def ag1():
    await anext_helper()
    yield 1


async def drive_anext():
    global a1
    a1 = ag1()
    await a1.__anext__()


show(lambda: asyncio.run(drive_anext()))

a2 = None


async def asend_helper():
    return await a2.asend(None)


async def ag2():
    await asend_helper()
    yield 1


async def drive_asend():
    global a2
    a2 = ag2()
    await a2.asend(None)


show(lambda: asyncio.run(drive_asend()))
