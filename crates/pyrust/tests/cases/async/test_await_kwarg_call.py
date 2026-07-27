# Awaiting a coroutine constructed with keyword / splat arguments (issue #2298).
#
# `await f(x, kw=v)` used to lower through the old variadic helper path,
# which emits an arg `Move` into the register slot that subsequently becomes the
# await drive's iterator.  Copy-propagation did not treat `GetAwaitable` as
# writing its destination, so the stale `Move` alias was substituted into the
# following `YieldFrom.iter_reg`, raising a spurious `object is not iterable`.
# Positional-only `await f(x, y)` was unaffected (no such Move).

import asyncio


async def add(x, y, z=0):
    return x + y + z


async def main():
    # The bug: a keyword arg in the awaited call.
    print(await add(1, y=2))
    # Inline as the whole return / nested in another expression.
    print((await add(1, y=2)) + (await add(10, y=20)))
    # Positional + keyword, splat, and double-splat all drive correctly.
    print(await add(1, 2, z=3))
    print(await add(*[4, 5], z=6))
    print(await add(**{"x": 7, "y": 8}))
    # Two kwarg-awaits in sequence (the original "subsequent await" corruption).
    a = await add(1, y=1)
    b = await add(2, y=2)
    print(a, b)


class ACM:
    def __init__(self, v):
        self.v = v

    async def __aenter__(self):
        return self.v

    async def __aexit__(self, *exc):
        return False


class Arange:
    def __init__(self, n):
        self.n = n

    def __aiter__(self):
        self.i = 0
        return self

    async def __anext__(self):
        if self.i >= self.n:
            raise StopAsyncIteration
        self.i += 1
        return self.i


async def managers():
    # async with / async for over a *keyword-constructed* manager / iterable.
    async with ACM(v=42) as a:
        print(a)
    total = 0
    async for n in Arange(n=3):
        total += n
    print(total)


asyncio.run(main())
asyncio.run(managers())
