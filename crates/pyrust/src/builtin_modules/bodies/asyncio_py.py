# Python-level members of the minimal `asyncio` module (issue #1039).
#
# These are exec'd once into a private namespace and copied onto the module by
# `asyncio::inject_python_members` (wired from `env.rs::load_module`).  They are
# defined in Python so that `sleep(...)` and `gather(...)` return real coroutine
# objects driven by the same await machinery as user `async def` functions.
#
# This is a deliberately minimal event-loop surface: there is no real timer or
# I/O scheduling.  `sleep(delay)` completes immediately (it does not actually
# wait `delay` seconds — see the PR follow-ups), and `gather(*coros)` awaits its
# coroutines sequentially, returning their results in order.


async def sleep(delay, result=None):
    """Minimal asyncio.sleep: completes immediately, returning `result`.

    Real time-based suspension is out of scope for the MVP; `sleep(0)` and
    `sleep(t)` both resolve right away.  `await asyncio.sleep(0, result=x)`
    yields `x`, matching CPython's return-value contract.
    """
    return result


async def gather(*coros):
    """Minimal asyncio.gather: await each awaitable in order, return a list.

    CPython runs the awaitables concurrently; the MVP drives them sequentially.
    For coroutines with no real I/O the observable results are identical.
    """
    results = []
    for c in coros:
        results.append(await c)
    return results
