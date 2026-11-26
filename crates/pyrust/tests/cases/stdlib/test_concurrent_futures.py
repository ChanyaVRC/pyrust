# Parity fixture for `concurrent` and `concurrent.futures` (issue #3263).

import concurrent
import concurrent.futures as cf
import concurrent.futures._base as cf_base
import concurrent.futures.process as cf_process
import concurrent.futures.thread as cf_thread
import logging
import math
import os
import threading
import time


# Module-level functions for ProcessPoolExecutor (required for spawn / Windows)
_PROC_INIT_VALUE = 0


def proc_init(val):
    global _PROC_INIT_VALUE
    _PROC_INIT_VALUE = val


def proc_square(x):
    return x * x


def proc_add(a, b, mult=1):
    return (a + b) * mult


def proc_use_init(x):
    return x + _PROC_INIT_VALUE


def proc_fail(x):
    if x == 0:
        raise ZeroDivisionError("cannot divide by zero")
    return 100 // x


def proc_get_pid(_x):
    return os.getpid()


if __name__ == "__main__":
    # ── 1. Module surface, submodule identities, and exception hierarchy ──────

    print(
        "identities:",
        cf.Future is cf_base.Future,
        cf.Executor is cf_base.Executor,
        cf.ThreadPoolExecutor is cf_thread.ThreadPoolExecutor,
        cf.ProcessPoolExecutor is cf_process.ProcessPoolExecutor,
        cf.TimeoutError is TimeoutError,
        cf.FIRST_COMPLETED,
        cf.FIRST_EXCEPTION,
        cf.ALL_COMPLETED,
    )
    print(
        "exceptions:",
        issubclass(cf.CancelledError, Exception),
        issubclass(cf.InvalidStateError, Exception),
        issubclass(cf.BrokenExecutor, RuntimeError),
        issubclass(cf_thread.BrokenThreadPool, cf.BrokenExecutor),
        issubclass(cf_process.BrokenProcessPool, cf.BrokenExecutor),
    )

    # ── 2. Future lifecycle, callbacks, cancellation, exceptions ──────────────

    f = cf.Future()
    cb_log = []
    f.add_done_callback(lambda fut: cb_log.append(("cb1", fut.result())))
    print("fut_pending:", f.done(), f.running(), f.cancelled())
    try:
        f.result(timeout=0.005)
        print("fut_timeout: UNEXPECTED")
    except TimeoutError:
        print("fut_timeout: ok")
    print("fut_set_running:", f.set_running_or_notify_cancel(), f.running())
    f.set_result(42)
    f.add_done_callback(lambda fut: cb_log.append(("cb2", fut.result())))
    print("fut_finished:", f.done(), f.running(), f.cancelled(), f.result(), f.exception(), cb_log)
    try:
        f.set_result(99)
    except cf.InvalidStateError:
        print("fut_double_set: InvalidStateError")

    fc = cf.Future()
    cb_c = []
    fc.add_done_callback(lambda fut: cb_c.append(fut.cancelled()))
    print("fut_cancel:", fc.cancel(), fc.cancelled(), fc.done(), cb_c)
    print("fut_notify_cancel:", fc.set_running_or_notify_cancel())
    try:
        fc.result()
    except cf.CancelledError:
        print("fut_cancel_result: CancelledError")

    fe = cf.Future()
    fe.set_exception(ValueError("bad value"))
    print("fut_exc_obj:", type(fe.exception()).__name__, str(fe.exception()))
    try:
        fe.result()
    except ValueError as e:
        print("fut_exc_raised:", str(e))

    # ── 3. ThreadPoolExecutor: submit, map, initializer, prefix, exceptions ───

    tls = threading.local()

    def thread_init(offset):
        tls.offset = offset

    def thread_work(x, scale=1):
        return (x + tls.offset) * scale, threading.current_thread().name.startswith("MyPool_")

    with cf.ThreadPoolExecutor(
        max_workers=3,
        thread_name_prefix="MyPool",
        initializer=thread_init,
        initargs=(10,),
    ) as pool:
        f1 = pool.submit(thread_work, 5, scale=2)
        f2 = pool.submit(thread_work, 7, scale=3)
        print("tpe_submit:", f1.result(timeout=10), f2.result(timeout=10))
        mapped = list(pool.map(lambda a, b: a * b, [1, 2, 3, 4], [10, 20, 30, 40]))
        print("tpe_map:", mapped)

        f_bad = pool.submit(lambda: 1 // 0)
        try:
            f_bad.result(timeout=10)
        except ZeroDivisionError:
            print("tpe_exc:", type(f_bad.exception()).__name__)

    # Submit after shutdown raises RuntimeError
    try:
        pool.submit(lambda: 1)
    except RuntimeError:
        print("tpe_after_shutdown: RuntimeError")

    # Invalid max_workers
    try:
        cf.ThreadPoolExecutor(max_workers=0)
    except ValueError:
        print("tpe_bad_workers: ValueError")

    # cancel_futures=True cancels queued tasks
    gate = threading.Event()
    entered = threading.Event()

    def hold_gate():
        entered.set()
        gate.wait(timeout=10.0)
        return "held"

    p_cancel = cf.ThreadPoolExecutor(max_workers=1)
    fa = p_cancel.submit(hold_gate)
    assert entered.wait(timeout=10.0)
    fb = p_cancel.submit(lambda: "queued1")
    fc2 = p_cancel.submit(lambda: "queued2")
    p_cancel.shutdown(wait=False, cancel_futures=True)
    gate.set()
    p_cancel.shutdown(wait=True)
    print("tpe_cancel_futures:", fa.result(timeout=10), fb.cancelled(), fc2.cancelled())

    # BrokenThreadPool on failing initializer
    def bad_init():
        raise RuntimeError("init boom")

    cf_logger = logging.getLogger("concurrent.futures")
    prev_disabled = cf_logger.disabled
    cf_logger.disabled = True
    try:
        with cf.ThreadPoolExecutor(max_workers=1, initializer=bad_init) as bad_pool:
            f_broken = bad_pool.submit(lambda: 123)
            try:
                f_broken.result(timeout=10)
            except cf_thread.BrokenThreadPool:
                print("tpe_broken: BrokenThreadPool")
    finally:
        cf_logger.disabled = prev_disabled

    # ── 4. wait() and as_completed() ──────────────────────────────────────────

    ev_slow = threading.Event()
    with cf.ThreadPoolExecutor(max_workers=3) as wpool:
        fslow = wpool.submit(lambda: (ev_slow.wait(timeout=10.0), "slow")[1])
        ffast = wpool.submit(lambda: "fast")
        res_first = cf.wait([fslow, ffast], timeout=10.0, return_when=cf.FIRST_COMPLETED)
        done1, not_done1 = res_first
        print(
            "wait_first:",
            isinstance(res_first, tuple),
            len(res_first.done),
            len(res_first.not_done),
            ffast in done1,
            fslow in not_done1,
        )

        # FIRST_EXCEPTION
        fexc = wpool.submit(lambda: (_ for _ in ()).throw(KeyError("k")))
        done_e, not_done_e = cf.wait([fslow, fexc], timeout=10.0, return_when=cf.FIRST_EXCEPTION)
        print("wait_exc:", fexc in done_e, fslow in not_done_e)

        # as_completed yields fast before slow
        order = []
        ac_futs = [fslow, ffast]
        ac_iter = cf.as_completed(ac_futs, timeout=10.0)
        first_done = next(ac_iter)
        order.append(first_done.result())
        ev_slow.set()
        second_done = next(ac_iter)
        order.append(second_done.result())
        print("as_completed_order:", order)

        # ALL_COMPLETED
        done_all, not_done_all = cf.wait([fslow, ffast], timeout=10.0, return_when=cf.ALL_COMPLETED)
        print("wait_all:", len(done_all), len(not_done_all))

    # as_completed timeout
    never_done = cf.Future()
    try:
        for _ in cf.as_completed([never_done], timeout=0.01):
            pass
    except TimeoutError:
        print("as_completed_timeout: ok")

    # wait invalid return_when (with a pending future, matching CPython)
    try:
        cf.wait([cf.Future()], return_when="BOGUS")
    except ValueError:
        print("wait_invalid: ValueError")

    # ── 5. ProcessPoolExecutor ────────────────────────────────────────────────

    with cf.ProcessPoolExecutor(max_workers=2) as ppool:
        pf1 = ppool.submit(proc_square, 7)
        pf2 = ppool.submit(proc_add, 3, 4, mult=5)
        pf3 = ppool.submit(math.isqrt, 81)
        print("ppe_submit:", pf1.result(timeout=30), pf2.result(timeout=30), pf3.result(timeout=30))

        pmap = list(ppool.map(proc_square, [1, 2, 3, 4, 5]))
        print("ppe_map:", pmap)

        pmap2 = list(ppool.map(proc_add, [1, 2, 3], [10, 20, 30]))
        print("ppe_map2:", pmap2)

        # Child process actually has a different PID than parent
        child_pid = ppool.submit(proc_get_pid, 0).result(timeout=30)
        print("ppe_real_proc:", isinstance(child_pid, int) and child_pid != os.getpid())

        # Exception propagation across process boundary
        pf_err = ppool.submit(proc_fail, 0)
        try:
            pf_err.result(timeout=30)
        except ZeroDivisionError as exc:
            print("ppe_exc:", type(exc).__name__, str(exc))

        # wait & as_completed on ProcessPoolExecutor futures
        pfuts = [ppool.submit(proc_square, i) for i in range(4)]
        pdone, pnot = cf.wait(pfuts, timeout=30)
        print("ppe_wait:", len(pdone), len(pnot), sorted(x.result() for x in cf.as_completed(pfuts, timeout=30)))

    # ProcessPoolExecutor with initializer & initargs
    with cf.ProcessPoolExecutor(
        max_workers=2,
        initializer=proc_init,
        initargs=(100,),
    ) as ipool:
        print("ppe_init:", list(ipool.map(proc_use_init, [1, 2, 3])))

    print("concurrent_futures_ok")
