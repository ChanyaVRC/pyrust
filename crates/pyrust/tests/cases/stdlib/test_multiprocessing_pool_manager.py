# Parity fixture for `multiprocessing.pool` (`Pool`, `ThreadPool`,
# `AsyncResult`), `multiprocessing.managers` (`Manager`/`SyncManager`,
# `SharedMemoryManager`), and `multiprocessing.dummy` (issue #3264).

import multiprocessing as mp
import multiprocessing.dummy as mp_dummy
import multiprocessing.dummy.connection as mp_dummy_conn
import multiprocessing.managers as mp_mgr
import multiprocessing.pool as mp_pool
import os
import threading
import time


# ── Module-level functions for Pool & Manager cross-process tests ────────────

_POOL_OFFSET = 0


def pool_init(offset):
    global _POOL_OFFSET
    _POOL_OFFSET = offset


def pool_square(x):
    return x * x


def pool_add(a, b, mult=1):
    return (a + b) * mult


def pool_with_offset(x):
    return x + _POOL_OFFSET


def pool_get_pid(_x):
    return os.getpid()


def pool_fail(x):
    if x == 0:
        raise ValueError("zero is invalid")
    return 100 // x


def pool_slow(x):
    time.sleep(0.25)
    return x


def manager_worker(d, lst, ns, val, q, lock, wid):
    with lock:
        d[f"k{wid}"] = wid * 10
        lst.append(wid)
        val.value = val.value + wid
    if wid == 1:
        ns.label = "updated_by_child"
        ns.count = 42
    q.put(("done", wid))


if __name__ == "__main__":
    mp.freeze_support()

    # ── 1. Pool: apply, apply_async, map, map_async, starmap, imap ───────────

    print("pool_timeout_id:", mp_pool.TimeoutError is mp.TimeoutError)

    with mp.Pool(processes=2) as pool:
        # apply & apply_async
        print("apply:", pool.apply(pool_add, (3, 4), {"mult": 5}))

        cb_results = []
        cb_done = threading.Event()

        def on_ok(res):
            cb_results.append(("ok", res))
            cb_done.set()

        ar = pool.apply_async(pool_square, (9,), callback=on_ok)
        print("apply_async:", ar.get(timeout=30.0), ar.ready(), ar.successful())
        assert cb_done.wait(timeout=10.0)
        print("apply_cb:", cb_results)

        # map & map_async
        print("map:", pool.map(pool_square, [1, 2, 3, 4, 5]))
        mr = pool.map_async(pool_square, [6, 7, 8])
        print("map_async:", mr.get(timeout=30.0), mr.successful())

        # starmap & starmap_async
        print("starmap:", pool.starmap(pool_add, [(1, 2), (3, 4), (5, 6)]))
        smr = pool.starmap_async(pool_add, [(10, 20), (30, 40)])
        print("starmap_async:", smr.get(timeout=30.0))

        # imap & imap_unordered
        print("imap:", list(pool.imap(pool_square, [2, 3, 4])))
        print("imap_unordered:", sorted(pool.imap_unordered(pool_square, [2, 3, 4])))

        # Real child process PID differs from parent
        cpid = pool.apply(pool_get_pid, (0,))
        print("pool_real_proc:", isinstance(cpid, int) and cpid != os.getpid())

        # Exception propagation + error_callback
        err_log = []
        err_done = threading.Event()

        def on_err(exc):
            err_log.append((type(exc).__name__, str(exc)))
            err_done.set()

        ar_err = pool.apply_async(pool_fail, (0,), error_callback=on_err)
        try:
            ar_err.get(timeout=30.0)
            print("pool_err: UNEXPECTED")
        except ValueError as e:
            print("pool_err:", type(e).__name__, str(e), ar_err.ready(), ar_err.successful())
        assert err_done.wait(timeout=10.0)
        print("pool_err_cb:", err_log)

        # AsyncResult.get timeout raises multiprocessing.TimeoutError
        ar_slow = pool.apply_async(pool_slow, (123,))
        try:
            ar_slow.get(timeout=0.01)
            print("pool_timeout: UNEXPECTED")
        except mp.TimeoutError:
            print("pool_timeout: TimeoutError")
        print("pool_slow_res:", ar_slow.get(timeout=30.0))

    # Pool with initializer, initargs, maxtasksperchild, and explicit close/join
    p2 = mp.Pool(processes=2, initializer=pool_init, initargs=(100,), maxtasksperchild=2)
    try:
        print("pool_init:", p2.map(pool_with_offset, [1, 2, 3, 4]))
    finally:
        p2.close()
        p2.join()

    # ── 2. ThreadPool ────────────────────────────────────────────────────────

    with mp_pool.ThreadPool(processes=3) as tpool:
        print("tpool_apply:", tpool.apply(lambda x, y=2: x * y, (7,), {"y": 3}))
        print("tpool_map:", tpool.map(lambda x: x + 1, [10, 20, 30]))
        print("tpool_starmap:", tpool.starmap(lambda a, b: a - b, [(10, 3), (20, 5)]))
        print("tpool_imap:", list(tpool.imap(lambda x: x * 2, [1, 2, 3])))

    # ── 3. Manager (SyncManager) across real child processes ─────────────────

    ctx = mp.get_context("spawn")
    with mp.Manager() as mgr:
        d = mgr.dict({"init": 0})
        lst = mgr.list([0])
        ns = mgr.Namespace()
        ns.label = "initial"
        val = mgr.Value("i", 10)
        q = mgr.Queue()
        lock = mgr.Lock()

        p_a = ctx.Process(target=manager_worker, args=(d, lst, ns, val, q, lock, 1))
        p_b = ctx.Process(target=manager_worker, args=(d, lst, ns, val, q, lock, 2))
        p_a.start()
        p_b.start()
        p_a.join(timeout=30.0)
        p_b.join(timeout=30.0)

        q_items = sorted(q.get(timeout=10.0) for _ in range(2))
        print(
            "manager_dict:",
            sorted(d.items()),
            "k1" in d,
            len(d),
            d.get("missing", -1),
        )
        print("manager_list:", sorted(list(lst)), len(lst))
        print("manager_ns:", ns.label, ns.count)
        print("manager_val:", val.value)
        print("manager_q:", q_items)

    # SharedMemoryManager context manager
    with mp_mgr.SharedMemoryManager() as smm:
        sm = smm.SharedMemory(16)
        sm.buf[:4] = b"ABCD"
        sl = smm.ShareableList([1, "two", 3.5])
        print("smm:", bytes(sm.buf[:4]), list(sl))

    # ── 4. multiprocessing.dummy ─────────────────────────────────────────────

    dq = mp_dummy.Queue()
    dl = mp_dummy.Lock()
    dval = mp_dummy.Value("i", 5)
    darr = mp_dummy.Array("i", [1, 2, 3])
    dns = mp_dummy.Namespace(x=10)
    dp1, dp2 = mp_dummy.Pipe()

    def dummy_worker(q, lock, v, conn):
        with lock:
            v.value += 10
        q.put("from_dummy")
        conn.send("pipe_dummy")

    dt = mp_dummy.Process(target=dummy_worker, args=(dq, dl, dval, dp2))
    dt.start()
    dt.join(timeout=10.0)
    print(
        "dummy_proc:",
        dq.get(timeout=5.0),
        dp1.recv(),
        dval.value,
        list(darr),
        dns.x,
        dt.exitcode,
        mp_dummy.Manager() is mp_dummy,
    )
    dp1.close()
    dp2.close()

    with mp_dummy.Pool(2) as dpool:
        print("dummy_pool:", dpool.map(lambda x: x * 3, [1, 2, 3, 4]))

    # dummy.connection Listener / Client
    with mp_dummy_conn.Listener() as dlist:
        def dummy_cli():
            with mp_dummy_conn.Client(dlist.address) as dc:
                dc.send("hi_dummy_srv")

        t_dc = threading.Thread(target=dummy_cli)
        t_dc.start()
        with dlist.accept() as d_srv:
            print("dummy_listener:", d_srv.recv())
        t_dc.join(timeout=10.0)

    print("multiprocessing_pool_manager_ok")
