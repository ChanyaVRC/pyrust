# Minimum-viable async/await (issue #1039).
#
# An `async def` declares a coroutine function; calling it returns a coroutine
# object (NOT a generator, NOT iterable).  `await coro` drives the awaited
# coroutine to its return value.  `asyncio.run(coro)` runs a top-level coroutine
# to completion.

import asyncio


# --- 1. The issue's repro ---

async def greet(name):
    return f"hello {name}"


async def main():
    result = await greet("world")
    print(result)


asyncio.run(main())


# --- 2. A coroutine object's identity ---

c = greet("x")
print(type(c).__name__)        # coroutine
print(c.__class__.__name__)    # coroutine
c.close()                      # avoid "never awaited" warning


# --- 3. Awaiting chains of coroutines ---

async def add(a, b):
    return a + b


async def chained():
    x = await add(1, 2)
    y = await add(x, 39)
    return y


print(asyncio.run(chained()))  # 42


# --- 4. asyncio.run returns the coroutine's value ---

print(asyncio.run(add(5, 6)))  # 11


# --- 5. An async def with no await is still a coroutine ---

async def no_await():
    return 7


na = no_await()
print(type(na).__name__)       # coroutine
na.close()                     # avoid "never awaited" warning
print(asyncio.run(no_await()))  # 7
