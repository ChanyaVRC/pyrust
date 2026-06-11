# Error behaviour for async/await (issue #1039).
#
# Exceptions are caught in-script so the output is the exception class + message
# only — deterministic across CPython and pyrust (the surrounding traceback
# frames differ because pyrust's asyncio is native rather than Python).

import asyncio


# --- 1. asyncio.run on a non-coroutine raises ValueError ---

for bad in (42, "x", [1, 2], None):
    try:
        asyncio.run(bad)
    except ValueError as e:
        print(type(e).__name__, e)


# --- 2. awaiting a non-awaitable raises TypeError ---

async def await_int():
    return await 5


try:
    asyncio.run(await_int())
except TypeError as e:
    print(type(e).__name__, e)


# --- 3. a coroutine is not iterable ---

async def coro():
    return 1


c = coro()
try:
    for _ in c:
        pass
except TypeError as e:
    print(type(e).__name__, e)
c.close()

c2 = coro()
try:
    iter(c2)
except TypeError as e:
    print(type(e).__name__, e)
c2.close()

c3 = coro()
try:
    list(c3)
except TypeError as e:
    print(type(e).__name__, e)
c3.close()


# --- 4. an exception raised inside a coroutine propagates out of run() ---

async def boom():
    raise RuntimeError("kaboom")


try:
    asyncio.run(boom())
except RuntimeError as e:
    print(type(e).__name__, e)
