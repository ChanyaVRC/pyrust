# Re-using an already-completed coroutine raises RuntimeError (issue #2282).
#
# A coroutine that has run to completion cannot be awaited / run again: CPython
# raises `RuntimeError: cannot reuse already awaited coroutine`.  Generators are
# unaffected (re-iterating an exhausted generator yields nothing, not an error).
#
# Exceptions are caught in-script so the output is the exception class + message
# only — deterministic across CPython and pyrust (the surrounding traceback
# frames differ because pyrust's asyncio is native).

import asyncio


async def f():
    return 1


# --- 1. awaiting the same coroutine twice ---

async def await_twice():
    c = f()
    await c
    return await c


try:
    asyncio.run(await_twice())
except RuntimeError as e:
    print(type(e).__name__, e)


# --- 2. asyncio.run on a coroutine that already ran ---

c = f()
print(asyncio.run(c))
try:
    asyncio.run(c)
except RuntimeError as e:
    print(type(e).__name__, e)


# --- 3. await after a separate asyncio.run completed it ---

c2 = f()
print(asyncio.run(c2))


async def reuse_completed(coro):
    return await coro


try:
    asyncio.run(reuse_completed(c2))
except RuntimeError as e:
    print(type(e).__name__, e)


# --- 4. a coroutine with several awaits, re-awaited after completion ---

async def chained():
    a = await f()
    b = await f()
    return a + b


async def reuse_chained():
    c = chained()
    first = await c
    print(first)
    return await c


try:
    asyncio.run(reuse_chained())
except RuntimeError as e:
    print(type(e).__name__, e)


# --- 5. generators are NOT affected: re-iterating an exhausted generator
#        yields an empty list, it does not raise. ---

def g():
    yield 1
    yield 2


gen = g()
print(list(gen))
print(list(gen))
