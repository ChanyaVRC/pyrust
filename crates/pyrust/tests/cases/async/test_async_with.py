"""`async with` statement: async context-manager protocol + suppression.

Exercises `await __aenter__()` / `await __aexit__(...)`, the exception
suppression contract, and left-to-right nesting of multiple items (issue #2279).
"""
import asyncio


class CM:
    def __init__(self, name):
        self.name = name

    async def __aenter__(self):
        print("enter", self.name)
        return self.name

    async def __aexit__(self, et, ev, tb):
        print("exit", self.name)
        return False


class Suppress:
    async def __aenter__(self):
        return self

    async def __aexit__(self, et, ev, tb):
        print("aexit:", et.__name__ if et else None, ev if ev else None)
        return True  # suppress


class NoSuppress:
    async def __aenter__(self):
        return self

    async def __aexit__(self, et, ev, tb):
        print("aexit2:", et.__name__ if et else None)
        return False


async def main():
    # Basic enter/body/exit.
    async with CM("a") as v:
        print("body", v)

    # Multiple items nest left-to-right (exit b then exit a).
    async with CM("a") as x, CM("b") as y:
        print("multi", x, y)

    # Exception suppressed by __aexit__ returning truthy.
    async with Suppress():
        raise ValueError("boom")
    print("after suppress")

    # Exception NOT suppressed -> propagates out.
    try:
        async with NoSuppress():
            raise KeyError("k")
    except KeyError as e:
        print("caught:", e)

    # Clean exit calls __aexit__ with all-None.
    async with NoSuppress():
        print("clean body")

    # await inside the body.
    async with CM("z") as v:
        await asyncio.sleep(0)
        print("awaited in body", v)


asyncio.run(main())
