# asyncio.timeout / timeout_at / Timeout / wait_for parity tests (issue #3154).

import asyncio


async def test_timeout_succeeds():
    async with asyncio.timeout(5) as cm:
        await asyncio.sleep(0)
        print("active expired:", cm.expired(), "when is float:", isinstance(cm.when(), float))
    print("after ok expired:", cm.expired())


async def test_timeout_expires():
    saw_cancel = False
    task = asyncio.current_task()
    print("cancelling before:", task.cancelling())
    try:
        async with asyncio.timeout(0.01) as cm:
            try:
                await asyncio.sleep(10)
            except asyncio.CancelledError:
                saw_cancel = True
                print("during cancel expired:", cm.expired())
                raise
    except TimeoutError as e:
        print("caught TimeoutError:", type(e) is TimeoutError)
    print("saw_cancel:", saw_cancel, "expired:", cm.expired(), "cancelling after:", task.cancelling())


async def test_timeout_none():
    async with asyncio.timeout(None) as cm:
        print("none when:", cm.when(), "expired:", cm.expired())
        await asyncio.sleep(0)
    print("none after:", cm.expired())


async def test_timeout_zero():
    try:
        async with asyncio.timeout(0) as cm:
            await asyncio.sleep(0)
    except TimeoutError:
        print("timeout(0) expired:", cm.expired())


async def test_timeout_at():
    loop = asyncio.get_running_loop()
    async with asyncio.timeout_at(loop.time() + 5) as cm:
        await asyncio.sleep(0)
    print("timeout_at ok:", cm.expired())

    try:
        async with asyncio.timeout_at(loop.time() + 0.01) as cm2:
            await asyncio.sleep(10)
    except TimeoutError:
        print("timeout_at expired:", cm2.expired())


async def test_reschedule():
    loop = asyncio.get_running_loop()

    # Extend deadline so task succeeds
    async with asyncio.timeout(0.01) as cm:
        cm.reschedule(loop.time() + 5)
        await asyncio.sleep(0.02)
    print("extended ok:", cm.expired())

    # Disable via reschedule(None)
    async with asyncio.timeout(0.01) as cm2:
        cm2.reschedule(None)
        print("disabled when:", cm2.when())
        await asyncio.sleep(0.02)
    print("disabled ok:", cm2.expired())

    # Shorten to past deadline -> immediate timeout
    try:
        async with asyncio.timeout(5) as cm3:
            cm3.reschedule(loop.time() - 1)
            await asyncio.sleep(5)
    except TimeoutError:
        print("shortened expired:", cm3.expired())

    # Invalid reschedule states
    cm4 = asyncio.timeout(5)
    try:
        cm4.reschedule(None)
    except RuntimeError as e:
        print("reschedule before enter:", str(e))

    async with cm4:
        pass
    try:
        cm4.reschedule(None)
    except RuntimeError as e:
        print("reschedule after exit:", str(e))
    try:
        async with cm4:
            pass
    except RuntimeError as e:
        print("re-enter timeout:", str(e))


async def test_nested_timeouts():
    # Outer expires before inner: outer raises TimeoutError, inner is not expired.
    try:
        async with asyncio.timeout(0.01) as outer:
            async with asyncio.timeout(5) as inner:
                await asyncio.sleep(10)
    except TimeoutError:
        print("nested outer expired:", outer.expired(), "inner expired:", inner.expired())


async def test_external_cancel_not_timeout():
    async def worker():
        async with asyncio.timeout(5) as cm:
            await asyncio.sleep(10)

    t = asyncio.create_task(worker())
    await asyncio.sleep(0)
    t.cancel()
    try:
        await t
    except asyncio.CancelledError:
        print("external cancel -> CancelledError, not TimeoutError")


async def test_wait_for_zero_and_shield():
    started = False

    async def work():
        nonlocal started
        started = True
        return "done"

    try:
        await asyncio.wait_for(work(), 0)
    except TimeoutError:
        print("wait_for(0) started:", started)

    # wait_for with shield: inner completes even when wait_for times out
    ev = asyncio.Event()
    finished = False

    async def shielded_work():
        nonlocal finished
        await ev.wait()
        finished = True
        return "shielded-ok"

    t = asyncio.create_task(shielded_work())
    try:
        await asyncio.wait_for(asyncio.shield(t), timeout=0.02)
    except TimeoutError:
        print("wait_for shield timed out, task done yet:", t.done())
    ev.set()
    res = await t
    print("shielded task finished:", finished, res)


async def main():
    await test_timeout_succeeds()
    await test_timeout_expires()
    await test_timeout_none()
    await test_timeout_zero()
    await test_timeout_at()
    await test_reschedule()
    await test_nested_timeouts()
    await test_external_cancel_not_timeout()
    await test_wait_for_zero_and_shield()


asyncio.run(main())
