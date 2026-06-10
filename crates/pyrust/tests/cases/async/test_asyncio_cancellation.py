# asyncio task cancellation, wait_for, and timeouts (issue #2307).
#
# Builds on the #2281 / #2305 event loop. Verifies CancelledError inheritance,
# Task.cancel()/Future.cancel() semantics, absorbed cancellation, wait_for
# success/timeout, and gather cancellation, all byte-identical to CPython 3.12.

import asyncio


# --- CancelledError inheritance (BaseException, NOT Exception, since 3.8) ---
print("Cancelled<BaseException:", issubclass(asyncio.CancelledError, BaseException))
print("Cancelled<Exception:", issubclass(asyncio.CancelledError, Exception))
print("TimeoutError is builtin:", asyncio.TimeoutError is TimeoutError)


async def slow():
    try:
        await asyncio.sleep(10)
    except asyncio.CancelledError:
        print("slow: got CancelledError")
        raise
    return "never"


async def absorb():
    try:
        await asyncio.sleep(10)
    except asyncio.CancelledError:
        print("absorb: caught, returning normally")
        return "absorbed"


async def caught_by_exc():
    try:
        await asyncio.sleep(10)
    except Exception:
        print("WRONG: except Exception caught CancelledError")
        return "wrong"
    return "no"


async def quick():
    return "q"


async def fast():
    await asyncio.sleep(0)
    return "fast-result"


async def main():
    # Task.cancel injects CancelledError at the await point; coroutine re-raises.
    t = asyncio.create_task(slow())
    await asyncio.sleep(0)
    print("cancel returned:", t.cancel())
    try:
        await t
    except asyncio.CancelledError:
        print("await cancelled task -> CancelledError")
    print("t.cancelled():", t.cancelled(), "t.done():", t.done())

    # Absorbed cancellation: caught + returned normally -> not cancelled.
    a = asyncio.create_task(absorb())
    await asyncio.sleep(0)
    a.cancel()
    r = await a
    print("absorb result:", r, "cancelled:", a.cancelled())

    # `except Exception` does NOT catch CancelledError; task ends cancelled.
    c = asyncio.create_task(caught_by_exc())
    await asyncio.sleep(0)
    c.cancel()
    try:
        await c
    except asyncio.CancelledError:
        print("caught_by_exc -> cancelled:", c.cancelled())

    # Cancel a task before it starts running.
    n = asyncio.create_task(quick())
    n.cancel()
    try:
        await n
    except asyncio.CancelledError:
        print("not-started cancelled:", n.cancelled())

    # Future.cancel: pending -> True, done -> False.
    f = asyncio.Future()
    f.set_result(5)
    print("cancel done future:", f.cancel())
    f2 = asyncio.Future()
    print("cancel pending future:", f2.cancel(), "cancelled:", f2.cancelled())

    # wait_for: completes in time, returns the result.
    print("wait_for in time:", await asyncio.wait_for(fast(), timeout=5))

    # wait_for: timeout -> TimeoutError (the builtin).
    try:
        await asyncio.wait_for(slow(), timeout=0.01)
    except TimeoutError as e:
        print("wait_for timeout -> TimeoutError:", type(e).__name__)

    # wait_for: timeout=None is a plain await.
    print("wait_for None:", await asyncio.wait_for(fast(), timeout=None))

    # gather cancellation: cancelling the gather cancels its children.
    g = asyncio.gather(slow(), slow())
    await asyncio.sleep(0)
    print("gather cancel:", g.cancel())
    try:
        await g
    except asyncio.CancelledError:
        print("gather await -> CancelledError")


asyncio.run(main())
print("done")
