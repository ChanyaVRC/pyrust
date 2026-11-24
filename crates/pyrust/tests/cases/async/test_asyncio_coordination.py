# asyncio task naming, inspection, gather(return_exceptions), wait,
# as_completed, and shield parity tests (issue #3154).

import asyncio


async def val(x, delay=0):
    if delay > 0:
        await asyncio.sleep(delay)
    return x


async def boom(msg, delay=0):
    if delay > 0:
        await asyncio.sleep(delay)
    raise ValueError(msg)


async def test_naming_and_inspection():
    cur = asyncio.current_task()
    print("root name:", cur.get_name())

    t_named = asyncio.create_task(val(1), name="worker-alpha")
    t_auto = asyncio.create_task(val(2))
    print("names:", t_named.get_name(), t_auto.get_name())

    t_auto.set_name("renamed")
    print("after set_name:", t_auto.get_name())
    t_auto.set_name(123)
    print("set_name int:", t_auto.get_name(), type(t_auto.get_name()).__name__)

    tasks = asyncio.all_tasks()
    print("cur in all_tasks:", cur in tasks)
    print("t_named in all_tasks:", t_named in tasks)
    print("t_auto in all_tasks:", t_auto in tasks)
    print("all_tasks len:", len(tasks))

    await t_named
    await t_auto
    tasks_after = asyncio.all_tasks()
    print("after await len:", len(tasks_after), cur in tasks_after)


async def test_cancelling_and_uncancel():
    ev = asyncio.Event()

    async def waiter():
        t = asyncio.current_task()
        print("initial cancelling:", t.cancelling())
        try:
            ev.set()
            await asyncio.sleep(10)
        except asyncio.CancelledError:
            print("on cancel cancelling:", t.cancelling())
            print("uncancel 1:", t.uncancel())
            print("uncancel 2:", t.uncancel())
            print("uncancel floor:", t.uncancel())
            return "recovered"

    task = asyncio.create_task(waiter())
    await ev.wait()
    await asyncio.sleep(0)
    task.cancel()
    task.cancel()
    res = await task
    print("waiter result:", res, "cancelled:", task.cancelled())


async def test_gather_return_exceptions():
    # Return exceptions instead of raising
    t_cancel = asyncio.create_task(asyncio.sleep(10))
    await asyncio.sleep(0)
    t_cancel.cancel()

    res = await asyncio.gather(
        val("ok"),
        boom("err1"),
        t_cancel,
        val(42),
        return_exceptions=True,
    )
    print(
        "gather return_exceptions:",
        res[0],
        type(res[1]).__name__,
        str(res[1]),
        type(res[2]).__name__,
        res[3],
    )

    # Duplicate future in gather
    f = asyncio.Future()
    f.set_result("dup")
    print("gather dup:", await asyncio.gather(f, f))

    # Cancelling gather itself propagates CancelledError even with return_exceptions=True
    g = asyncio.gather(asyncio.sleep(10), return_exceptions=True)
    await asyncio.sleep(0)
    g.cancel()
    try:
        await g
    except asyncio.CancelledError:
        print("gather cancel with return_exceptions -> CancelledError")


async def test_wait():
    print("constants:", asyncio.FIRST_COMPLETED, asyncio.FIRST_EXCEPTION, asyncio.ALL_COMPLETED)

    # ALL_COMPLETED
    t1 = asyncio.create_task(val("a"))
    t2 = asyncio.create_task(val("b"))
    done, pending = await asyncio.wait([t1, t2])
    print("wait all:", sorted(t.result() for t in done), len(pending))

    # FIRST_COMPLETED using deterministic events
    e1 = asyncio.Event()
    e2 = asyncio.Event()

    async def wait_ev(e, label):
        await e.wait()
        return label

    w1 = asyncio.create_task(wait_ev(e1, "first"))
    w2 = asyncio.create_task(wait_ev(e2, "second"))
    e1.set()
    done, pending = await asyncio.wait({w1, w2}, return_when=asyncio.FIRST_COMPLETED)
    print("wait first:", [t.result() for t in done], len(pending), w2 in pending)
    e2.set()
    await w2

    # FIRST_EXCEPTION
    ef = asyncio.Event()
    es = asyncio.Event()

    async def fail_on_ev():
        await ef.wait()
        raise RuntimeError("first-exc")

    wf = asyncio.create_task(fail_on_ev())
    ws = asyncio.create_task(wait_ev(es, "still-waiting"))
    ef.set()
    done, pending = await asyncio.wait([wf, ws], return_when=asyncio.FIRST_EXCEPTION)
    print("wait first_exc:", len(done), len(pending), type(wf.exception()).__name__)
    es.set()
    await ws

    # wait with timeout
    wslow = asyncio.create_task(asyncio.sleep(10))
    done, pending = await asyncio.wait([wslow], timeout=0.01)
    print("wait timeout:", len(done), len(pending), wslow in pending)
    wslow.cancel()
    try:
        await wslow
    except asyncio.CancelledError:
        pass

    # Validation errors
    try:
        await asyncio.wait([])
    except ValueError as e:
        print("wait empty:", str(e))

    c = val(1)
    try:
        await asyncio.wait([c])
    except TypeError as e:
        print("wait coro in list:", "forbidden" in str(e))
    finally:
        c.close()

    f = asyncio.Future()
    try:
        await asyncio.wait(f)
    except TypeError as e:
        print("wait single future:", "expect a list" in str(e))

    t3 = asyncio.create_task(val(3))
    try:
        await asyncio.wait([t3], return_when="BAD")
    except ValueError as e:
        print("wait bad return_when:", "Invalid return_when" in str(e))
    await t3


async def test_as_completed():
    # Deterministic completion order driven by pre-created futures
    loop = asyncio.get_running_loop()
    f1 = loop.create_future()
    f2 = loop.create_future()
    f3 = loop.create_future()

    async def resolve_in_order():
        await asyncio.sleep(0)
        f2.set_result("second-arg-first-done")
        await asyncio.sleep(0)
        f3.set_result("third-arg-second-done")
        await asyncio.sleep(0)
        f1.set_result("first-arg-last-done")

    driver = asyncio.create_task(resolve_in_order())
    order = []
    for coro in asyncio.as_completed([f1, f2, f3]):
        order.append(await coro)
    await driver
    print("as_completed order:", order)

    # Exception propagation in as_completed
    f_ok = loop.create_future()
    f_err = loop.create_future()
    f_err.set_exception(ValueError("ac-boom"))
    f_ok.set_result("ac-ok")
    got = []
    for coro in asyncio.as_completed([f_ok, f_err]):
        try:
            got.append(("ok", await coro))
        except ValueError as e:
            got.append(("err", str(e)))
    print("as_completed exc:", sorted(got))

    # Timeout in as_completed: already-done future yields its value, pending times out
    slow = asyncio.create_task(asyncio.sleep(10))
    fast = loop.create_future()
    fast.set_result("quick")
    out = []
    for coro in asyncio.as_completed([slow, fast], timeout=0.05):
        try:
            out.append(await coro)
        except TimeoutError:
            out.append("TIMEOUT")
    print("as_completed timeout:", out)
    slow.cancel()
    try:
        await slow
    except asyncio.CancelledError:
        pass


async def test_shield():
    # Normal completion
    print("shield normal:", await asyncio.shield(val("plain")))

    # Outer cancellation does NOT cancel inner task
    ev = asyncio.Event()
    inner_completed = False

    async def inner_coro():
        nonlocal inner_completed
        await ev.wait()
        inner_completed = True
        return "survived"

    inner_task = asyncio.create_task(inner_coro())

    async def outer_coro():
        return await asyncio.shield(inner_task)

    outer_task = asyncio.create_task(outer_coro())
    await asyncio.sleep(0)
    outer_task.cancel()
    try:
        await outer_task
    except asyncio.CancelledError:
        print("shield outer cancelled:", outer_task.cancelled(), "inner done:", inner_task.done())
    ev.set()
    print("shield inner result:", await inner_task, "completed:", inner_completed)

    # Inner cancelled directly -> shield awaiter sees CancelledError
    t2 = asyncio.create_task(asyncio.sleep(10))
    sh = asyncio.shield(t2)
    await asyncio.sleep(0)
    t2.cancel()
    try:
        await sh
    except asyncio.CancelledError:
        print("shield inner cancelled -> CancelledError")

    # Inner exception propagates
    try:
        await asyncio.shield(boom("inner-err"))
    except ValueError as e:
        print("shield inner exc:", str(e))


async def main():
    await test_naming_and_inspection()
    await test_cancelling_and_uncancel()
    await test_gather_return_exceptions()
    await test_wait()
    await test_as_completed()
    await test_shield()


asyncio.run(main())
