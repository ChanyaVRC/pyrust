"""Async generators: `async def` containing `yield` (issue #2280).

An `async def` whose body contains a bare `yield` is an *async generator*
(`type(g).__name__ == 'async_generator'`), driven by the asynchronous-iterator
protocol (`__aiter__` / `__anext__`) plus `asend` / `athrow` / `aclose`.  A bare
`yield v` surfaces as the item from `__anext__`; an inner `await` propagates its
scheduling point to the event loop.  Exercises the happy path, empty / return /
mixed-await bodies, nested `async for`, the introspection surface, and the
synchronous-iteration / `yield from` errors — all checked against CPython 3.12.
"""
import asyncio


# --- basic: items collected by async for ---------------------------------

async def squares(n):
    for i in range(n):
        yield i * i


async def collect_basic():
    out = []
    async for x in squares(5):
        out.append(x)
    print("basic:", out)


# --- mixing await and yield ------------------------------------------------

async def mixed(n):
    for i in range(n):
        await asyncio.sleep(0)
        yield i
        await asyncio.sleep(0)


async def collect_mixed():
    out = []
    async for x in mixed(4):
        out.append(x)
    print("mixed:", out)


# --- empty async generator -------------------------------------------------

async def empty():
    if False:
        yield 1


async def collect_empty():
    out = []
    async for x in empty():
        out.append(x)
    print("empty:", out)


# --- async generator with a bare `return` (→ StopAsyncIteration, no value) -

async def with_return(n):
    for i in range(n):
        if i == 2:
            return
        yield i


async def collect_return():
    out = []
    async for x in with_return(5):
        out.append(x)
    print("return:", out)


# --- nested async for ------------------------------------------------------

async def inner(n):
    for i in range(n):
        yield i


async def outer(n):
    async for x in inner(n):
        yield x * 10


async def collect_nested():
    out = []
    async for x in outer(4):
        out.append(x)
    print("nested:", out)


# --- introspection: type name, repr shape, attribute surface ---------------

async def introspect():
    g = squares(3)
    print("typename:", type(g).__name__)
    r = repr(g)
    print(
        "repr_ok:",
        r.startswith("<async_generator object squares at 0x") and r.endswith(">"),
    )
    for m in ("__aiter__", "__anext__", "asend", "athrow", "aclose"):
        print(m, hasattr(g, m))
    # async generators are NOT synchronous iterators.
    print("has __next__:", hasattr(g, "__next__"))
    print("has send:", hasattr(g, "send"))
    await g.aclose()


# --- asend: drive the generator manually, feeding values back in -----------

async def echo():
    received = yield 0
    while True:
        received = yield received * 2


async def drive_asend():
    g = echo()
    print("asend0:", await g.asend(None))   # primes -> 0
    print("asend1:", await g.asend(5))      # -> 10
    print("asend2:", await g.asend(7))      # -> 14
    await g.aclose()


# --- athrow: inject an exception the body catches, then continues ----------

async def catcher():
    try:
        yield 1
        yield 2
    except ValueError:
        yield 99
    yield 3


async def drive_athrow():
    g = catcher()
    print("athrow0:", await g.asend(None))         # 1
    print("athrow1:", await g.athrow(ValueError))  # 99 (caught)
    print("athrow2:", await g.asend(None))         # 3
    try:
        await g.asend(None)
    except StopAsyncIteration:
        print("athrow exhausted")

    # An uncaught athrow propagates.
    g2 = catcher()
    print("athrow3:", await g2.asend(None))        # 1
    try:
        await g2.athrow(KeyError("boom"))
    except KeyError as e:
        print("athrow uncaught:", e)


# --- aclose / asend error semantics ---------------------------------------


async def swallows_genexit():
    try:
        yield 1
    except GeneratorExit:
        # Ignoring GeneratorExit (yielding again during close) is a RuntimeError.
        yield 2


async def drive_close_errors():
    # asend(non-None) to a just-started async generator is a TypeError.
    g = squares(3)
    try:
        await g.asend(5)
    except TypeError as e:
        print("asend_just_started:", e)
    await g.aclose()

    # awaiting the async generator object itself (not __anext__) is a TypeError.
    g = squares(3)
    try:
        await g
    except TypeError as e:
        print("await_agen:", e)
    await g.aclose()

    # aclose on a never-started generator, then a second aclose: both no-ops.
    g = squares(3)
    await g.aclose()
    await g.aclose()
    print("double_aclose: ok")

    # A generator that swallows GeneratorExit and yields again → RuntimeError.
    g = swallows_genexit()
    await g.asend(None)
    try:
        await g.aclose()
    except RuntimeError as e:
        print("ignored_genexit:", e)


async def main():
    await collect_basic()
    await collect_mixed()
    await collect_empty()
    await collect_return()
    await collect_nested()
    await introspect()
    await drive_asend()
    await drive_athrow()
    await drive_close_errors()


asyncio.run(main())


# --- synchronous iteration of an async generator is a TypeError ------------

def sync_iteration_errors():
    g = squares(3)
    try:
        next(g)
    except TypeError as e:
        print("next:", e)
    try:
        for _ in g:
            pass
    except TypeError as e:
        print("for:", e)
    try:
        iter(g)
    except TypeError as e:
        print("iter:", e)


sync_iteration_errors()
