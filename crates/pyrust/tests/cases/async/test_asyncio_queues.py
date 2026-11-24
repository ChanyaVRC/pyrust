# asyncio.Queue, LifoQueue, PriorityQueue, QueueEmpty, QueueFull parity tests
# (issue #3154).

import asyncio

# Construction outside a running loop (Python 3.10+ lazy loop binding)
q_outer = asyncio.Queue(maxsize=2)
print("outer maxsize:", q_outer.maxsize, "empty:", q_outer.empty(), "full:", q_outer.full())


async def test_fifo_queue():
    q = asyncio.Queue(maxsize=2)
    print("init:", q.maxsize, q.qsize(), q.empty(), q.full())

    try:
        q.get_nowait()
    except asyncio.QueueEmpty:
        print("empty -> QueueEmpty")

    q.put_nowait("a")
    q.put_nowait("b")
    print("full state:", q.qsize(), q.empty(), q.full())

    try:
        q.put_nowait("c")
    except asyncio.QueueFull:
        print("full -> QueueFull")

    # Blocking put unblocked by get
    put_done = False

    async def producer():
        nonlocal put_done
        await q.put("c")
        put_done = True

    p = asyncio.create_task(producer())
    await asyncio.sleep(0)
    print("producer blocked:", not put_done)
    print("get 1:", q.get_nowait())
    await p
    print("producer unblocked:", put_done, "qsize:", q.qsize())
    print("get 2,3:", await q.get(), await q.get())

    # task_done / join
    q.task_done()
    q.task_done()
    q.task_done()
    await q.join()
    print("join done")

    try:
        q.task_done()
    except ValueError as e:
        print("extra task_done:", str(e))


async def test_producer_consumer_join():
    q = asyncio.Queue()
    processed = []

    async def consumer():
        while True:
            item = await q.get()
            processed.append(item)
            q.task_done()

    c = asyncio.create_task(consumer())
    for i in range(4):
        await q.put(i * 10)
    await q.join()
    c.cancel()
    try:
        await c
    except asyncio.CancelledError:
        pass
    print("processed:", processed)


async def test_cancelled_getter_wakes_next():
    q = asyncio.Queue()

    async def getter(name):
        return (name, await q.get())

    g1 = asyncio.create_task(getter("g1"))
    g2 = asyncio.create_task(getter("g2"))
    await asyncio.sleep(0)
    g1.cancel()
    q.put_nowait("payload")
    try:
        await g1
    except asyncio.CancelledError:
        print("g1 cancelled")
    print("g2 got:", await g2)


async def test_lifo_queue():
    q = asyncio.LifoQueue(maxsize=3)
    for x in [1, 2, 3]:
        await q.put(x)
    print("lifo full:", q.full())
    items = [await q.get(), await q.get(), await q.get()]
    print("lifo order:", items)
    for _ in range(3):
        q.task_done()
    await q.join()
    print("lifo joined")


async def test_priority_queue():
    q = asyncio.PriorityQueue()
    for item in [(3, "low"), (1, "high"), (2, "mid"), (1, "alpha")]:
        await q.put(item)
    out = []
    while not q.empty():
        out.append(q.get_nowait())
        q.task_done()
    await q.join()
    print("priority order:", out)


async def test_outer_queue_usable():
    await q_outer.put("hello")
    print("outer got:", await q_outer.get())
    q_outer.task_done()
    await q_outer.join()


async def main():
    await test_fifo_queue()
    await test_producer_consumer_join()
    await test_cancelled_getter_wakes_next()
    await test_lifo_queue()
    await test_priority_queue()
    await test_outer_queue_usable()


asyncio.run(main())
