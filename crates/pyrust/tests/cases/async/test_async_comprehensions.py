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


async def get_list():
    await asyncio.sleep(0)
    return [1, 2, 3]


async def klist(x):
    await asyncio.sleep(0)
    return [x, x + 100]


async def await_no_async_for():
    # #2304: an `await` in the element / condition / non-outermost iterable
    # (without any `async for` clause) makes the comprehension asynchronous.
    xs = [1, 2, 3]

    # await in the element of list / set / dict comprehensions.
    print([await double(x) for x in xs])
    print(sorted({await double(x) for x in xs}))
    print({await double(x): x for x in xs})
    print({x: await double(x) for x in xs})

    # await inside an f-string interpolation in the element, including a
    # nested format spec — these are real sub-expressions in the comp scope.
    print([f"{await double(x)}" for x in xs])
    print([f"{x:>{await double(x)}}" for x in xs])

    # await in the condition.
    print([x for x in xs if await is_even(x)])
    print({x for x in xs if await is_even(x)})

    # await in a NON-outermost clause iterable.
    print([y for x in xs for y in await klist(x)])
    print({y: y for x in xs for y in await klist(x)})

    # await only in the OUTERMOST iterable is the enclosing scope's concern,
    # so this is a plain (sync) comprehension.
    print([x for x in await get_list()])

    # Lambda defaults execute in the comprehension scope. An await there makes
    # the synthesized comprehension function async even though the lambda body
    # itself remains a separate scope.
    print([(lambda value=(await double(x)): value)() for x in xs])

    # The same boundary applies when a lambda default contains a nested async
    # collection comprehension: its async-ness propagates to the outer comp.
    print([
        (lambda value=[await double(y) for y in range(x)]: value)()
        for x in xs
    ])

    # A nested comprehension's first iterable is evaluated in its enclosing
    # scope. Here that scope is the outer comprehension via a lambda default,
    # so the await must make the outer synthesized function async.
    print([
        (lambda value=[y for y in await get_list()]: value)()
        for _ in [0]
    ])

    # Format specs are recursively nested f-string parts. The async list comp
    # lives two specs deep; its result is paired with an empty string so the
    # resulting format spec remains valid ("3" for the outer integer).
    print(f"{42:{3:{([await double(y) for y in range(1)], '')[1]}}}")

    # await combined with an explicit `async for` clause.
    print([await double(x) async for x in arange(3)])

    # genexp with `await` but no `async for` is an async generator.
    agen = (await double(x) for x in xs)
    print(type(agen).__name__)
    print([v async for v in agen])


async def nested_comp_propagation():
    # #2312: a nested async COLLECTION comprehension (list/set/dict) in the
    # element / cond / non-outermost iterable makes the OUTER comprehension
    # async (the outer body must await the inner comp's coroutine). A nested
    # async genexp does NOT propagate (creating an async-gen object needs no
    # await). The outermost-iterable rule still holds.
    xs = [1, 2, 3]

    # inner async list comp in the element of a sync-source outer list comp.
    print([[await double(y) for y in range(x)] for x in xs])
    # inner async set comp.
    print([sorted({await double(y) for y in range(x)}) for x in xs])
    # inner async dict comp (as the outer element).
    print([{y: await double(y) for y in range(x)} for x in xs])

    # outer set / dict comprehension with a nested async list comp.
    print(sorted([tuple([await double(y) for y in range(x)]) for x in xs]))
    print({x: [await double(y) for y in range(x)] for x in xs})

    # depth-3 nesting.
    print([[[await double(z) for z in range(y)] for y in range(x)] for x in range(3)])

    # nested async list comp wrapped inside an intervening expression / call /
    # sync genexp still propagates outward.
    print([sum([await double(y) for y in range(x)]) for x in xs])
    print([list(v for v in [await double(y) for y in range(x)]) for x in xs])

    # nested async list comp in a NON-outermost iterable propagates.
    print([v for x in xs for v in [await double(y) for y in range(x)]])

    # nested async-for list comp propagates the same way.
    print([[y async for y in arange(x)] for x in xs])

    # a nested async GENEXP does NOT make the outer async: the outer stays a
    # plain list comp producing async_generator objects.
    gens = [(y async for y in arange(x)) for x in range(2)]
    print([type(g).__name__ for g in gens])

    # genexp-outer with a nested async list comp is itself an async generator.
    agen = ([await double(y) for y in range(x)] for x in range(3))
    print(type(agen).__name__)
    print([item async for item in agen])


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
asyncio.run(await_no_async_for())
asyncio.run(nested_comp_propagation())

# Sync comprehensions are unaffected.
print([i * i for i in range(4)])
print(sorted({i % 3 for i in range(6)}))
print({i: i + 1 for i in range(3)})
print(list(i for i in range(4)))
print([(a, b) for a in range(2) for b in range(2) if a != b])
