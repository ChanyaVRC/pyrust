# asyncio.sleep / asyncio.gather (issue #1039).
#
# These are the minimal building blocks: `sleep` resolves immediately (the MVP
# has no real timer), `gather` awaits its coroutines in order and returns a list
# of results.  For coroutines with no real I/O the observable results match
# CPython.

import asyncio


async def double(x):
    return x * 2


async def main():
    # sleep(0) is a no-op scheduling point that yields None.
    r = await asyncio.sleep(0)
    print(r)  # None

    # sleep(delay, result=...) resolves to `result`.
    print(await asyncio.sleep(0, result="done"))  # done

    # gather runs several coroutines and collects their results in order.
    results = await asyncio.gather(double(1), double(2), double(3))
    print(results)  # [2, 4, 6]

    # gather with a single coroutine.
    print(await asyncio.gather(double(10)))  # [20]

    # gather with no coroutines.
    print(await asyncio.gather())  # []


asyncio.run(main())
