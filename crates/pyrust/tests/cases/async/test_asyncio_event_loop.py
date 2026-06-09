# asyncio real event loop: sleep ordering, concurrent gather / create_task,
# sleep(0) fairness (issue #2281).
#
# Determinism: ordering is driven by *distinct* sleep deltas (0.01 / 0.02 /
# 0.03 s) — large enough that wake order is stable across machines, small
# enough to keep the test fast.  Output asserts the interleave order via
# prints, never absolute timings.

import asyncio


# --- 1. sleep ordering: tasks ordered by wake time, not start order ---

async def timed(name, delay):
    await asyncio.sleep(delay)
    print("woke", name)
    return name


async def ordering():
    # Spawn in A,B,C order but with descending delays: they must finish in
    # wake-time order C, B, A.
    res = await asyncio.gather(
        timed("A", 0.03),
        timed("B", 0.01),
        timed("C", 0.02),
    )
    print("ordering results", res)


asyncio.run(ordering())


# --- 2. gather concurrency: a print before/after each await interleaves ---

async def work(n):
    print("work start", n)
    await asyncio.sleep(0.01 * n)
    print("work end", n)
    return n * 10


async def concurrency():
    res = await asyncio.gather(work(1), work(2), work(3))
    print("gather results", res)


asyncio.run(concurrency())


# --- 3. create_task runs concurrently with the awaiting coroutine ---

async def background():
    print("bg start")
    await asyncio.sleep(0.02)
    print("bg end")
    return "bg-done"


async def with_task():
    t = asyncio.create_task(background())
    print("main running")
    await asyncio.sleep(0.01)
    print("main mid")
    result = await t
    print("task result", result)


asyncio.run(with_task())


# --- 4. sleep(0) fairness: two spinners interleave turn-by-turn ---

async def spinner(name, count):
    for i in range(count):
        print("spin", name, i)
        await asyncio.sleep(0)


async def fairness():
    await asyncio.gather(spinner("X", 3), spinner("Y", 3))


asyncio.run(fairness())


# --- 5. nested gather: a gather inside a task ---

async def leaf(v):
    await asyncio.sleep(0.01)
    return v


async def branch():
    parts = await asyncio.gather(leaf(1), leaf(2))
    return sum(parts)


async def nested():
    res = await asyncio.gather(branch(), leaf(10))
    print("nested results", res)


asyncio.run(nested())
