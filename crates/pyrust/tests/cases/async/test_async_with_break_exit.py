"""`async with` cleanup on control-flow early exit (issue #2295).

A `break`/`continue`/`return` leaving an `async with` body must still
`await __aexit__(None, None, None)`, in reverse order for multiple managers,
with the normal-exit and exception (suppression) paths unchanged.
"""
import asyncio


class ACM:
    def __init__(self, name):
        self.name = name

    async def __aenter__(self):
        print("enter", self.name)
        return self.name

    async def __aexit__(self, et, ev, tb):
        print("exit", self.name, et)
        return False


async def main():
    print("== break ==")
    for i in range(3):
        async with ACM("A"):
            print("body", i)
            if i == 1:
                break
    print("after break")

    print("== continue ==")
    for i in range(3):
        async with ACM("A"):
            if i == 1:
                continue
            print("body", i)
    print("after continue")

    print("== return ==")

    async def f():
        async with ACM("A"):
            return 7

    print(await f())

    print("== return binds alias ==")

    async def g():
        async with ACM("V") as v:
            return v

    print(await g())

    print("== nested break (reverse order) ==")
    for i in range(2):
        async with ACM("A"), ACM("B"):
            if i == 0:
                break
            print("body", i)
    print("after nested break")

    print("== nested return ==")

    async def h():
        async with ACM("A"), ACM("B"):
            return "done"

    print(await h())

    print("== normal fall-through still works ==")
    for i in range(2):
        async with ACM("A"):
            print("body", i)

    print("== exception path still works ==")
    try:
        async with ACM("X"), ACM("Y"):
            raise KeyError("k")
    except KeyError:
        print("caught")

    print("== exception suppression unchanged ==")

    class Sup:
        async def __aenter__(self):
            return self

        async def __aexit__(self, et, ev, tb):
            print("sup exit", et)
            return True

    async with Sup():
        raise ValueError("boom")
    print("after suppress")


asyncio.run(main())
