# `await` is only valid inside an `async def` (issue #1039).
#
# Using `compile(...)` so the SyntaxError is caught and its `.msg` printed,
# keeping the output deterministic across CPython and pyrust.

cases = [
    # await inside a plain (non-async) function → "outside async function"
    "def f():\n    await 5",
    # await at module scope → "outside function"
    "await 5",
    # await inside a nested non-async function (the async enclosing scope does
    # not make a nested plain def async).
    "async def f():\n    def g():\n        await 5",
    # #2304: an `await` in a comprehension element/condition (without an
    # `async for`) makes it an asynchronous comprehension, which is illegal
    # outside an async function.
    "def f():\n    return [await g(x) for x in xs]",
    "def f():\n    return {await g(x) for x in xs}",
    "def f():\n    return {x: await g(x) for x in xs}",
    "def f():\n    return [x for x in xs if await p(x)]",
    # #2312: a nested async COLLECTION comprehension propagates async-ness to
    # the outer comprehension, so these are illegal outside an async function.
    "def f():\n    return [[await g(y) for y in range(x)] for x in xs]",
    "def f():\n    return [{await g(y) for y in range(x)} for x in xs]",
    "def f():\n    return [{y: await g(y) for y in range(x)} for x in xs]",
    # nested async comp wrapped in an intervening expression still propagates.
    "def f():\n    return [sum([await g(y) for y in range(x)]) for x in xs]",
    # nested async comp in a NON-outermost iterable propagates.
    "def f():\n    return [v for x in xs for v in [await g(y) for y in range(x)]]",
    # async comp in the OUTERMOST iterable makes the ENCLOSING scope async.
    "def f():\n    return [x for x in [await g(y) for y in range(2)]]",
    # a nested async GENEXP does NOT propagate — this outer comp stays sync.
    "def f():\n    return [(y async for y in g(x)) for x in xs]",
]

for src in cases:
    try:
        compile(src, "<test>", "exec")
        print("no error")
    except SyntaxError as e:
        print("SyntaxError:", e.msg)

# Valid await inside async def compiles cleanly.
compile("async def f():\n    await g()", "<test>", "exec")
print("valid await compiles")
