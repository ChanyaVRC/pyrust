# Async comprehensions (PEP 530, issue #2283).
#
# `[x async for x in ait]` / set / dict comprehensions run the implicit
# comprehension function as a coroutine that the enclosing async frame awaits;
# `(x async for x in ait)` is an async generator expression. Builds on the
# async-for drive (#2279) and async generators (#2280).

import asyncio


async def arange(n):
    for i in range(n):
        await asyncio.sleep(0)
        yield i


async def is_even(x):
    return x % 2 == 0


async def double(x):
    return x * 2


async def main():
    # list / set / dict comprehensions over an async iterator.
    print([x async for x in arange(4)])
    print(sorted({x async for x in arange(3)}))
    print({x: x * 10 async for x in arange(3)})

    # filter clause (sync condition) and an `await` inside the condition.
    print([x async for x in arange(5) if x % 2 == 0])
    print([x async for x in arange(5) if await is_even(x)])

    # `await` in the produced element.
    print([await double(x) async for x in arange(4)])

    # nested: an async-for clause followed by a plain-for clause.
    print([(a, b) async for a in arange(2) for b in range(2)])

    # walrus target leaks to the enclosing scope (PEP 572).
    vals = [y := x + 1 async for x in arange(3)]
    print(vals, y)

    # empty async iterator.
    print([x async for x in arange(0)])

    # async generator expression — produces an async_generator, not awaited.
    agen = (x + 100 async for x in arange(3))
    print(type(agen).__name__)
    print([v async for v in agen])


asyncio.run(main())

# Sync comprehensions are unaffected.
print([i * i for i in range(4)])
print(sorted({i % 3 for i in range(6)}))
print({i: i + 1 for i in range(3)})
print(list(i for i in range(4)))
print([(a, b) for a in range(2) for b in range(2) if a != b])
