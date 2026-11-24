# asyncio synchronization primitives parity tests: Lock, Event, Semaphore,
# BoundedSemaphore, Condition, Barrier, BrokenBarrierError (issue #3154).

import asyncio

# Construction outside a running loop
_outer_lock = asyncio.Lock()
_outer_event = asyncio.Event()
_outer_sem = asyncio.Semaphore(2)
_outer_bsem = asyncio.BoundedSemaphore(2)
_outer_cond = asyncio.Condition()
_outer_barrier = asyncio.Barrier(2)
print("outer constructed ok:", not _outer_lock.locked(), not _outer_event.is_set(), _outer_barrier.parties)


async def test_lock():
    lock = asyncio.Lock()
    print("lock init:", lock.locked())
    try:
        lock.release()
    except RuntimeError as e:
        print("unlocked release:", str(e))

    order = []

    async def worker(name):
        async with lock:
            order.append(f"enter-{name}")
            await asyncio.sleep(0)
            order.append(f"exit-{name}")

    t1 = asyncio.create_task(worker("A"))
    t2 = asyncio.create_task(worker("B"))
    await asyncio.gather(t1, t2)
    print("lock order:", order, "locked after:", lock.locked())

    # Cancellation while waiting wakes next waiter
    await lock.acquire()
    w1 = asyncio.create_task(lock.acquire())
    w2 = asyncio.create_task(lock.acquire())
    await asyncio.sleep(0)
    w1.cancel()
    lock.release()
    try:
        await w1
    except asyncio.CancelledError:
        print("lock waiter w1 cancelled")
    print("lock waiter w2 acquired:", await w2, lock.locked())
    lock.release()


async def test_event():
    ev = asyncio.Event()
    print("event init:", ev.is_set())
    woken = []

    async def waiter(name):
        res = await ev.wait()
        woken.append((name, res))

    t1 = asyncio.create_task(waiter("E1"))
    t2 = asyncio.create_task(waiter("E2"))
    await asyncio.sleep(0)
    print("before set woken:", woken)
    ev.set()
    await asyncio.gather(t1, t2)
    print("after set woken:", woken, "is_set:", ev.is_set())
    print("wait when set:", await ev.wait())
    ev.clear()
    print("after clear:", ev.is_set())


async def test_semaphore_and_bounded():
    try:
        asyncio.Semaphore(-1)
    except ValueError as e:
        print("neg sem:", str(e))

    sem = asyncio.Semaphore(2)
    active = 0
    max_active = 0

    async def job():
        nonlocal active, max_active
        async with sem:
            active += 1
            if active > max_active:
                max_active = active
            await asyncio.sleep(0)
            active -= 1

    await asyncio.gather(job(), job(), job(), job())
    print("sem max_active:", max_active, "locked:", sem.locked())

    # BoundedSemaphore raises ValueError on excess release
    bsem = asyncio.BoundedSemaphore(1)
    async with bsem:
        print("bsem inside locked:", bsem.locked())
    try:
        bsem.release()
    except ValueError as e:
        print("bsem excess release:", str(e))


async def test_condition():
    cond = asyncio.Condition()
    try:
        await cond.wait()
    except RuntimeError as e:
        print("cond wait unlocked:", str(e))
    try:
        cond.notify()
    except RuntimeError as e:
        print("cond notify unlocked:", str(e))

    state = {"ready": False, "val": 0}

    async def waiter_pred():
        async with cond:
            res = await cond.wait_for(lambda: state["ready"])
            return (res, state["val"])

    async def notifier():
        await asyncio.sleep(0)
        async with cond:
            state["val"] = 99
            state["ready"] = True
            cond.notify_all()

    wt = asyncio.create_task(waiter_pred())
    nt = asyncio.create_task(notifier())
    print("cond wait_for:", await wt)
    await nt

    # notify(n) wakes n waiters
    woken = []

    async def wait_n(i):
        async with cond:
            await cond.wait()
            woken.append(i)

    tasks = [asyncio.create_task(wait_n(i)) for i in range(3)]
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    async with cond:
        cond.notify(1)
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    print("after notify(1):", woken)
    async with cond:
        cond.notify_all()
    await asyncio.gather(*tasks)
    print("after notify_all:", woken)

    # Cancelled wait still reacquires lock
    async def cancelled_wait():
        async with cond:
            try:
                await cond.wait()
            except asyncio.CancelledError:
                print("cond cancelled still locked:", cond.locked())
                raise

    ct = asyncio.create_task(cancelled_wait())
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    ct.cancel()
    try:
        await ct
    except asyncio.CancelledError:
        print("cond cancel done, unlocked after aexit:", not cond.locked())


async def test_barrier():
    try:
        asyncio.Barrier(0)
    except ValueError as e:
        print("barrier 0:", str(e))

    print(
        "BrokenBarrierError < RuntimeError:",
        issubclass(asyncio.BrokenBarrierError, RuntimeError),
    )

    b = asyncio.Barrier(3)
    print("barrier init:", b.parties, b.n_waiting, b.broken)

    async def participant():
        async with b as idx:
            return idx

    # Wave 1
    idxs1 = await asyncio.gather(participant(), participant(), participant())
    print("barrier wave 1:", sorted(idxs1), "n_waiting:", b.n_waiting, "broken:", b.broken)

    # Wave 2 (reusable)
    idxs2 = await asyncio.gather(b.wait(), b.wait(), b.wait())
    print("barrier wave 2:", sorted(idxs2))

    # reset() wakes waiting task with BrokenBarrierError and returns to filling
    w = asyncio.create_task(b.wait())
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    print("before reset n_waiting:", b.n_waiting)
    await b.reset()
    try:
        await w
    except asyncio.BrokenBarrierError:
        print("reset -> BrokenBarrierError, broken:", b.broken)

    # abort() sets broken=True and rejects future waits
    w2 = asyncio.create_task(b.wait())
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    await b.abort()
    try:
        await w2
    except asyncio.BrokenBarrierError:
        print("abort waiter -> BrokenBarrierError, broken:", b.broken)
    try:
        await b.wait()
    except asyncio.BrokenBarrierError:
        print("wait on broken -> BrokenBarrierError")


async def main():
    await test_lock()
    await test_event()
    await test_semaphore_and_bounded()
    await test_condition()
    await test_barrier()


asyncio.run(main())
