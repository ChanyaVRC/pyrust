"""`async for` statement: async-iterator protocol, else, break/continue, nesting.

Exercises the asynchronous iterator protocol (`__aiter__` / `await __anext__()`)
driven through the same suspend/resume machinery as `await` (issue #2279).
"""
import asyncio


class Arange:
    def __init__(self, n):
        self.n = n
        self.i = 0

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self.i >= self.n:
            raise StopAsyncIteration
        self.i += 1
        return self.i - 1


async def helper(v):
    # An awaitable that itself suspends, to confirm the await drive resumes.
    await asyncio.sleep(0)
    return v * 2


class APairs:
    def __init__(self):
        self.i = 0

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self.i >= 3:
            raise StopAsyncIteration
        self.i += 1
        return (self.i - 1, (self.i - 1) * 10)


class ANested:
    def __init__(self):
        self.i = 0

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self.i >= 2:
            raise StopAsyncIteration
        self.i += 1
        return await helper(self.i - 1)


async def main():
    # Basic iteration.
    out = []
    async for x in Arange(3):
        out.append(x)
    print("basic:", out)

    # else runs on clean (StopAsyncIteration) exit.
    out = []
    async for x in Arange(3):
        out.append(x)
    else:
        out.append("else")
    print("forelse:", out)

    # break skips else.
    out = []
    async for x in Arange(5):
        if x == 2:
            break
        out.append(x)
    else:
        out.append("else")
    print("break:", out)

    # continue.
    out = []
    async for x in Arange(5):
        if x % 2 == 0:
            continue
        out.append(x)
    print("continue:", out)

    # Tuple-unpacking target.
    out = []
    async for k, v in APairs():
        out.append((k, v))
    print("tuple:", out)

    # await inside __anext__ and inside the body.
    out = []
    async for x in ANested():
        out.append(x)
        await asyncio.sleep(0)
    print("nested-await:", out)

    # Empty async iterator.
    out = []
    async for x in Arange(0):
        out.append(x)
    else:
        out.append("empty-else")
    print("empty:", out)


asyncio.run(main())
