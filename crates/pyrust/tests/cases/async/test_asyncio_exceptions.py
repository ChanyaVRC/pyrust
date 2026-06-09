# asyncio event loop: exception propagation through gather / tasks (issue
# #2281).  Exceptions are caught in-script so output is deterministic (no
# traceback frames, which differ between CPython and pyrust's native asyncio).

import asyncio


# --- 1. gather propagates the first exception (default return_exceptions) ---

async def ok():
    await asyncio.sleep(0.02)
    return "ok"


async def boom():
    await asyncio.sleep(0.01)
    raise ValueError("kaboom")


async def gather_exc():
    try:
        await asyncio.gather(ok(), boom())
    except ValueError as e:
        print("gather caught", type(e).__name__, e)


asyncio.run(gather_exc())


# --- 2. awaiting a failing task re-raises its exception ---

async def failing():
    await asyncio.sleep(0.01)
    raise KeyError("missing")


async def task_exc():
    t = asyncio.create_task(failing())
    try:
        await t
    except KeyError as e:
        print("task caught", type(e).__name__, e)


asyncio.run(task_exc())


# --- 3. an exception raised before the first await still propagates ---

async def immediate_fail():
    raise RuntimeError("right away")


async def immediate():
    try:
        await immediate_fail()
    except RuntimeError as e:
        print("immediate caught", type(e).__name__, e)


asyncio.run(immediate())


# --- 4. an exception escaping the top-level coro surfaces from asyncio.run ---

async def top_fail():
    await asyncio.sleep(0.01)
    raise ZeroDivisionError("top")


try:
    asyncio.run(top_fail())
except ZeroDivisionError as e:
    print("run caught", type(e).__name__, e)
