# Parity fixture for real multi-threaded `threading`, `_thread`, and `queue`
# execution (issue #3263).

import _thread
import queue
import threading
import time


# ── 1. Real concurrency proof: Event ping-pong between 2 threads ──────────────

ev_ping = threading.Event()
ev_pong = threading.Event()
ping_log = []


def ponger():
    assert ev_ping.wait(timeout=10.0)
    ping_log.append("pong")
    ev_pong.set()


t_pong = threading.Thread(target=ponger, name="PongWorker")
t_pong.start()
ping_log.append("ping")
ev_ping.set()
assert ev_pong.wait(timeout=10.0)
t_pong.join(timeout=10.0)
print("ping_pong:", ping_log, t_pong.is_alive())


# ── 2. Thread metadata: ident, native_id, active_count, enumerate ─────────────

meta_ready = threading.Event()
meta_release = threading.Event()
meta_info = {}


def meta_worker():
    cur = threading.current_thread()
    meta_info["name"] = cur.name
    meta_info["is_main"] = cur is threading.main_thread()
    meta_info["ident_diff"] = threading.get_ident() != threading.main_thread().ident
    meta_info["ident_match"] = cur.ident == threading.get_ident()
    meta_info["native_match"] = cur.native_id == threading.get_native_id()
    meta_info["alive_inside"] = cur.is_alive()
    meta_ready.set()
    meta_release.wait(timeout=10.0)


t_meta = threading.Thread(target=meta_worker, name="MetaWorker")
t_meta.start()
assert meta_ready.wait(timeout=10.0)
mid_count = threading.active_count()
mid_names = sorted(th.name for th in threading.enumerate())
t_meta.join(timeout=0.01)
still_alive = t_meta.is_alive()
meta_release.set()
t_meta.join(timeout=10.0)
print(
    "meta:",
    meta_info["name"],
    meta_info["is_main"],
    meta_info["ident_diff"],
    meta_info["ident_match"],
    meta_info["native_match"],
    meta_info["alive_inside"],
    mid_count == 2,
    mid_names,
    still_alive,
    t_meta.is_alive(),
    threading.active_count(),
)


# ── 3. Lock & RLock mutual exclusion across concurrent threads ────────────────

lk = threading.Lock()
rlk = threading.RLock()
shared = {"n": 0, "rn": 0}


def lock_worker():
    for _ in range(100):
        with lk:
            shared["n"] += 1
        with rlk:
            with rlk:
                shared["rn"] += 1


lock_threads = [threading.Thread(target=lock_worker) for _ in range(4)]
for t in lock_threads:
    t.start()
for t in lock_threads:
    t.join(timeout=10.0)
print("locks:", shared["n"], shared["rn"])


# ── 4. Condition wait / wait_for / notify_all across threads ──────────────────

cond = threading.Condition()
state = {"ready": False, "seen": []}


def cond_waiter(wid):
    with cond:
        ok = cond.wait_for(lambda: state["ready"], timeout=10.0)
        state["seen"].append((wid, ok))


waiters = [threading.Thread(target=cond_waiter, args=(i,)) for i in range(3)]
for t in waiters:
    t.start()
time.sleep(0.02)
with cond:
    state["ready"] = True
    cond.notify_all()
for t in waiters:
    t.join(timeout=10.0)
print("condition:", sorted(state["seen"]))


# ── 5. Semaphore & BoundedSemaphore across threads ────────────────────────────

bsem = threading.BoundedSemaphore(2)
sem_lock = threading.Lock()
sem_stats = {"cur": 0, "max": 0, "done": 0}


def sem_worker():
    with bsem:
        with sem_lock:
            sem_stats["cur"] += 1
            if sem_stats["cur"] > sem_stats["max"]:
                sem_stats["max"] = sem_stats["cur"]
        time.sleep(0.02)
        with sem_lock:
            sem_stats["cur"] -= 1
            sem_stats["done"] += 1


sem_threads = [threading.Thread(target=sem_worker) for _ in range(4)]
for t in sem_threads:
    t.start()
for t in sem_threads:
    t.join(timeout=10.0)
print("semaphore:", sem_stats["max"] <= 2, sem_stats["cur"], sem_stats["done"])


# ── 6. Barrier across 3 concurrent threads (2 reusable phases) ────────────────

barrier_actions = []
bar = threading.Barrier(3, action=lambda: barrier_actions.append("action"))
bar_results = []
bar_lock = threading.Lock()


def bar_worker(wid):
    r1 = bar.wait(timeout=10.0)
    r2 = bar.wait(timeout=10.0)
    with bar_lock:
        bar_results.append((wid, r1, r2))


bar_threads = [threading.Thread(target=bar_worker, args=(i,)) for i in range(3)]
for t in bar_threads:
    t.start()
for t in bar_threads:
    t.join(timeout=10.0)
print(
    "barrier:",
    len(barrier_actions),
    sorted(r[1] for r in bar_results),
    sorted(r[2] for r in bar_results),
    bar.broken,
)


# ── 7. Producer / Consumer with Queue, LifoQueue, PriorityQueue, SimpleQueue ──

q = queue.Queue(maxsize=3)
consumed = []
cons_lock = threading.Lock()


def producer():
    for i in range(10):
        q.put(i, timeout=10.0)
    q.put(None, timeout=10.0)
    q.put(None, timeout=10.0)


def consumer():
    while True:
        item = q.get(timeout=10.0)
        try:
            if item is None:
                return
            with cons_lock:
                consumed.append(item)
        finally:
            q.task_done()


tp = threading.Thread(target=producer)
tc1 = threading.Thread(target=consumer)
tc2 = threading.Thread(target=consumer)
tp.start()
tc1.start()
tc2.start()
tp.join(timeout=10.0)
tc1.join(timeout=10.0)
tc2.join(timeout=10.0)
q.join()
print("queue_pc:", sorted(consumed), q.empty())

sq = queue.SimpleQueue()


def sq_producer():
    time.sleep(0.02)
    sq.put("hello")
    sq.put("world")


tsq = threading.Thread(target=sq_producer)
tsq.start()
sq_out = [sq.get(timeout=10.0), sq.get(timeout=10.0)]
tsq.join(timeout=10.0)
print("simple_queue:", sq_out)

pq = queue.PriorityQueue()


def pq_producer():
    for pair in [(3, "c"), (1, "a"), (2, "b")]:
        pq.put(pair)


tpq = threading.Thread(target=pq_producer)
tpq.start()
tpq.join(timeout=10.0)
print("priority_queue:", [pq.get(timeout=5.0) for _ in range(3)])

lq = queue.LifoQueue()
for v in [10, 20, 30]:
    lq.put(v)
print("lifo_queue:", [lq.get(timeout=5.0) for _ in range(3)])


# ── 8. threading.local isolation across threads ───────────────────────────────


class ScopedLocal(threading.local):
    def __init__(self, tag="default"):
        self.tag = tag


tls = ScopedLocal(tag="init")
tls.owner = "main"
tls_out = []
tls_bar = threading.Barrier(3)


def tls_worker(idx):
    initial_tag = tls.tag
    has_main_attr = hasattr(tls, "owner")
    tls.owner = f"worker-{idx}"
    tls_bar.wait(timeout=10.0)
    tls_out.append((idx, initial_tag, has_main_attr, tls.owner))


tls_threads = [threading.Thread(target=tls_worker, args=(i,)) for i in range(3)]
for t in tls_threads:
    t.start()
for t in tls_threads:
    t.join(timeout=10.0)
print("local_isolation:", sorted(tls_out), tls.owner, tls.tag)


# ── 9. Timer firing and cancellation ──────────────────────────────────────────

timer_fired = threading.Event()
timer_vals = []
tmr = threading.Timer(0.02, lambda v: (timer_vals.append(v), timer_fired.set()), args=("ok",))
tmr.start()
assert timer_fired.wait(timeout=10.0)
tmr.join(timeout=10.0)

tmr_c = threading.Timer(5.0, lambda: timer_vals.append("bad"))
tmr_c.start()
tmr_c.cancel()
tmr_c.join(timeout=10.0)
print("timer:", timer_vals)


# ── 10. threading.excepthook and SystemExit handling ──────────────────────────

caught_hooks = []
orig_hook = threading.excepthook


def custom_excepthook(args):
    caught_hooks.append((args.exc_type.__name__, str(args.exc_value), args.thread.name))


threading.excepthook = custom_excepthook
try:
    def boom():
        raise ValueError("worker failed")

    def quiet_exit():
        raise SystemExit(0)

    t_err = threading.Thread(target=boom, name="BoomThread")
    t_err.start()
    t_err.join(timeout=10.0)

    t_sys = threading.Thread(target=quiet_exit, name="ExitThread")
    t_sys.start()
    t_sys.join(timeout=10.0)
finally:
    threading.excepthook = orig_hook

print("excepthook:", caught_hooks)


# ── 11. _thread.start_new_thread ──────────────────────────────────────────────

raw_done = threading.Event()
raw_out = []


def raw_worker(a, b=0):
    raw_out.append(a + b)
    raw_done.set()


raw_tid = _thread.start_new_thread(raw_worker, (10,), {"b": 32})
assert raw_done.wait(timeout=10.0)
print("raw_thread:", isinstance(raw_tid, int) and raw_tid > 0, raw_out)


# ── 12. Daemon thread inheritance & non-hanging exit ──────────────────────────

child_daemon_flag = []


def daemon_parent():
    child = threading.Thread(target=lambda: None)
    child_daemon_flag.append(child.daemon)


t_dp = threading.Thread(target=daemon_parent, daemon=True)
t_dp.start()
t_dp.join(timeout=10.0)
print("daemon_inherit:", child_daemon_flag)

# Start a long-sleeping daemon thread; process exit must not hang!
t_bg = threading.Thread(target=lambda: time.sleep(60.0), daemon=True)
t_bg.start()
print("threading_concurrent_ok")
