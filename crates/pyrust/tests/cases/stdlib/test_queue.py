# Parity fixture for `queue` (issue #3208).

import queue
from queue import Empty, Full, LifoQueue, PriorityQueue, Queue, SimpleQueue
import types


def catch_error(fn):
    try:
        fn()
    except Exception as exc:
        return (type(exc).__name__, str(exc))
    return ("OK", None)


# ── 1. Module metadata, exceptions, GenericAlias ──────────────────────────────

print("all:", queue.__all__)
print("Empty subclass:", issubclass(Empty, Exception))
print("Full subclass:", issubclass(Full, Exception))
for cls in (Queue, LifoQueue, PriorityQueue, SimpleQueue):
    alias = cls[int]
    assert type(alias) is types.GenericAlias
    assert alias.__origin__ is cls
    assert alias.__args__ == (int,)
print("generic aliases: ok")

# ── 2. FIFO Queue (unbounded & bounded) ───────────────────────────────────────

q = Queue()
print("init:", q.qsize(), q.empty(), q.full(), q.maxsize)
for x in ("a", "b", "c"):
    q.put(x)
print("after puts:", q.qsize(), q.empty(), q.full())
out = [q.get(), q.get_nowait(), q.get(block=False)]
print("fifo out:", out, q.qsize(), q.empty())

# Empty errors
print("get_nowait empty:", catch_error(lambda: q.get_nowait()))
print("get(block=False) empty:", catch_error(lambda: q.get(block=False)))
print("get(timeout=0) empty:", catch_error(lambda: q.get(timeout=0)))
print("get(timeout=0.005) empty:", catch_error(lambda: q.get(timeout=0.005)))
print("get(timeout=-1) empty:", catch_error(lambda: q.get(timeout=-1)))
# block=False ignores negative timeout
print(
    "get(block=False, timeout=-1):",
    catch_error(lambda: q.get(block=False, timeout=-1)),
)

# Bounded Queue (maxsize=2)
bq = Queue(maxsize=2)
bq.put(10)
print("bq 1/2:", bq.qsize(), bq.empty(), bq.full())
bq.put_nowait(20)
print("bq 2/2:", bq.qsize(), bq.empty(), bq.full())
print("put_nowait full:", catch_error(lambda: bq.put_nowait(30)))
print("put(block=False) full:", catch_error(lambda: bq.put(30, block=False)))
print("put(timeout=0) full:", catch_error(lambda: bq.put(30, timeout=0)))
print("put(timeout=0.005) full:", catch_error(lambda: bq.put(30, timeout=0.005)))
print("put(timeout=-1) full:", catch_error(lambda: bq.put(30, timeout=-1)))
print(
    "put(block=False, timeout=-1):",
    catch_error(lambda: bq.put(30, block=False, timeout=-1)),
)
print("bq pop:", bq.get(), bq.full(), bq.qsize())
bq.put(30)
print("bq drain:", [bq.get(), bq.get()], bq.empty())

# Negative maxsize means unbounded (matches CPython: maxsize <= 0)
uq = Queue(maxsize=-5)
for i in range(10):
    uq.put(i)
print("negative maxsize:", uq.qsize(), uq.full())

# ── 3. task_done() and join() ─────────────────────────────────────────────────

tq = Queue()
# join() on empty queue returns immediately
tq.join()
tq.put("job1")
tq.put("job2")
assert tq.unfinished_tasks == 2
assert tq.get() == "job1"
tq.task_done()
assert tq.unfinished_tasks == 1
assert tq.get() == "job2"
tq.task_done()
assert tq.unfinished_tasks == 0
tq.join()
print("task_done + join: ok")
print("task_done too many:", catch_error(lambda: tq.task_done()))

# ── 4. LifoQueue ──────────────────────────────────────────────────────────────

lq = LifoQueue(maxsize=3)
for i in (1, 2, 3):
    lq.put(i)
print("lifo full:", lq.full(), catch_error(lambda: lq.put_nowait(4)))
print("lifo order:", [lq.get(), lq.get(), lq.get()], lq.empty())
print("lifo empty:", catch_error(lambda: lq.get_nowait()))

# ── 5. PriorityQueue ──────────────────────────────────────────────────────────

pq = PriorityQueue(maxsize=4)
for item in ((3, "low"), (1, "high"), (2, "mid"), (1, "alpha")):
    pq.put(item)
print("pq full:", pq.full(), catch_error(lambda: pq.put_nowait((0, "x"))))
print("pq order:", [pq.get() for _ in range(4)], pq.empty())
print("pq empty:", catch_error(lambda: pq.get_nowait()))

# ── 6. SimpleQueue ────────────────────────────────────────────────────────────

sq = SimpleQueue()
print("sq init:", sq.qsize(), sq.empty())
sq.put("first")
sq.put_nowait("second")
sq.put("third", block=False, timeout=-1)
print("sq after puts:", sq.qsize(), sq.empty())
print("sq order:", [sq.get(), sq.get_nowait(), sq.get(block=False)])
print("sq empty:", sq.qsize(), sq.empty())
print("sq get_nowait empty:", catch_error(lambda: sq.get_nowait()))
print("sq get(block=False) empty:", catch_error(lambda: sq.get(block=False)))
print(
    "sq get(block=False, timeout=-1):",
    catch_error(lambda: sq.get(block=False, timeout=-1)),
)
print("sq get(timeout=0) empty:", catch_error(lambda: sq.get(timeout=0)))
print("sq get(timeout=0.005) empty:", catch_error(lambda: sq.get(timeout=0.005)))
print("sq get(timeout=-1) empty:", catch_error(lambda: sq.get(timeout=-1)))

# ── 7. Subclassing Queue hooks (_init/_qsize/_put/_get) ───────────────────────

class UniqueQueue(Queue):
    def _init(self, maxsize):
        self.queue = []
        self._seen = set()

    def _put(self, item):
        if item not in self._seen:
            self._seen.add(item)
            self.queue.append(item)

    def _get(self):
        item = self.queue.pop(0)
        self._seen.remove(item)
        return item


uq2 = UniqueQueue()
for v in (1, 2, 1, 3, 2):
    uq2.put(v)
print("subclass queue:", uq2.qsize(), [uq2.get() for _ in range(uq2.qsize())])

print("queue tests ok")
