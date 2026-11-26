# Parity fixture for `multiprocessing` core APIs: Process, Context, IPC
# (Pipe/Connection/Listener/Client/wait), Queue/SimpleQueue/JoinableQueue,
# Synchronization (Lock/RLock/Semaphore/Event/Condition/Barrier), Value/Array,
# and shared_memory (SharedMemory/ShareableList) (issue #3264).

import _multiprocessing
import ctypes
import multiprocessing as mp
import multiprocessing.connection as mp_conn
import multiprocessing.context as mp_ctx
import multiprocessing.heap as mp_heap
import multiprocessing.process as mp_proc
import multiprocessing.queues as mp_queues
import multiprocessing.reduction as mp_red
import multiprocessing.resource_sharer as mp_sharer
import multiprocessing.resource_tracker as mp_tracker
import multiprocessing.shared_memory as mp_shm
import multiprocessing.sharedctypes as mp_sc
import multiprocessing.spawn as mp_spawn
import multiprocessing.synchronize as mp_sync
import multiprocessing.util as mp_util
import os
import queue
import sys
import threading


# ── Module-level worker targets (required for 'spawn' on Windows & Unix) ─────

def worker_queue_square(q_in, q_out):
    pid = os.getpid()
    while True:
        val = q_in.get()
        if val is None:
            break
        q_out.put((val, val * val, pid != os.getppid()))


def worker_joinable(jq, out_q):
    while True:
        item = jq.get()
        if item is None:
            jq.task_done()
            break
        out_q.put(item * 10)
        jq.task_done()


def worker_simple_queue(sq):
    sq.put(("from_child", os.getpid() != os.getppid()))


def worker_pipe(conn):
    msg = conn.recv()
    conn.send(("echo", msg[0] + msg[1]))
    raw = conn.recv_bytes()
    conn.send_bytes(raw[::-1])
    conn.close()


def worker_unidirectional_pipe(send_conn):
    send_conn.send("one_way_ok")
    send_conn.close()


class CustomWorkerProcess(mp.Process):
    def __init__(self, factor, out_q):
        super().__init__(name="CustomChild")
        self.factor = factor
        self.out_q = out_q

    def run(self):
        cp = mp.current_process()
        pp = mp.parent_process()
        self.out_q.put((
            cp.name,
            pp is not None and pp.pid != os.getpid(),
            self.factor * 7,
        ))


def worker_sync_and_shared(val, arr, lock, ev_ready, ev_go):
    ev_ready.set()
    ev_go.wait(timeout=15.0)
    for _ in range(5):
        with lock:
            val.value += 1
    with arr.get_lock():
        for i in range(len(arr)):
            arr[i] = arr[i] + 10


def worker_barrier(bar, out_q, wid):
    idx = bar.wait(timeout=15.0)
    out_q.put((wid, idx in (0, 1)))


def worker_shm_write(shm_name, sl_name):
    shm = mp_shm.SharedMemory(name=shm_name, create=False)
    try:
        shm.buf[0:6] = b"PYRUST"
    finally:
        shm.close()

    sl = mp_shm.ShareableList(name=sl_name)
    try:
        sl[0] = 999
        sl[2] = "done"
    finally:
        sl.shm.close()


def worker_client_connect(addr, authkey, out_q):
    with mp_conn.Client(addr, authkey=authkey) as c:
        c.send({"hello": "server"})
        reply = c.recv()
    out_q.put(reply)


def worker_sleep_forever():
    import time
    time.sleep(60)


def worker_fork_test(q, val, ev):
    val.value = 321
    q.put(("forked", os.getpid() != os.getppid()))
    ev.set()


if __name__ == "__main__":
    mp.freeze_support()

    # ── 1. Module surface, context, exceptions, and process metadata ─────────

    print(
        "exc_hierarchy:",
        issubclass(mp.ProcessError, Exception),
        issubclass(mp.BufferTooShort, mp.ProcessError),
        issubclass(mp.TimeoutError, mp.ProcessError),
        issubclass(mp.AuthenticationError, mp.ProcessError),
        mp.TimeoutError is not TimeoutError,
        mp.ProcessError is mp_ctx.ProcessError,
    )

    methods = mp.get_all_start_methods()
    default_sm = mp.get_start_method()
    print(
        "start_methods:",
        "spawn" in methods,
        default_sm in methods,
        mp.cpu_count() >= 1,
        isinstance(_multiprocessing.flags, dict),
    )

    cur = mp.current_process()
    print(
        "main_proc:",
        cur.name,
        cur.pid == os.getpid(),
        cur.ident == os.getpid(),
        cur.daemon,
        cur.is_alive(),
        cur.exitcode,
        mp.parent_process() is None,
    )

    ctx = mp.get_context("spawn")
    print("spawn_ctx:", ctx.get_start_method())

    try:
        mp.get_context("nonexistent_method")
        print("bad_ctx: UNEXPECTED")
    except ValueError:
        print("bad_ctx: ValueError")

    # ── 2. Process + Queue + SimpleQueue + JoinableQueue ─────────────────────

    q_in = ctx.Queue()
    q_out = ctx.Queue()
    p = ctx.Process(target=worker_queue_square, args=(q_in, q_out), name="SqWorker")
    print("pre_start:", p.name, p.pid, p.exitcode, p.is_alive())
    p.start()
    for n in [2, 5, 9]:
        q_in.put(n)
    q_in.put(None)
    results = sorted(q_out.get(timeout=20.0) for _ in range(3))
    p.join(timeout=20.0)
    print("queue_results:", results, p.exitcode, p.is_alive(), p.pid is not None)
    p.close()

    # Bounded Queue + Empty/Full exceptions
    bq = ctx.Queue(maxsize=2)
    bq.put_nowait("a")
    bq.put_nowait("b")
    print("bq_state:", bq.full(), bq.qsize())
    try:
        bq.put_nowait("c")
        print("bq_full: UNEXPECTED")
    except queue.Full:
        print("bq_full: Full")
    print("bq_pop:", bq.get(timeout=5.0), bq.get(timeout=5.0), bq.empty())
    try:
        bq.get_nowait()
        print("bq_empty: UNEXPECTED")
    except queue.Empty:
        print("bq_empty: Empty")

    # SimpleQueue across process
    sq = ctx.SimpleQueue()
    psq = ctx.Process(target=worker_simple_queue, args=(sq,))
    psq.start()
    print("simple_queue:", sq.get())
    psq.join(timeout=20.0)
    sq.close()

    # JoinableQueue across process
    jq = ctx.JoinableQueue()
    jq_out = ctx.Queue()
    pjq = ctx.Process(target=worker_joinable, args=(jq, jq_out))
    pjq.start()
    for x in [1, 2, 3]:
        jq.put(x)
    jq.put(None)
    jq.join()
    jq_vals = sorted(jq_out.get(timeout=20.0) for _ in range(3))
    pjq.join(timeout=20.0)
    print("joinable_queue:", jq_vals)

    # Subclassing Process + overriding run()
    sub_q = ctx.Queue()
    sp = CustomWorkerProcess(6, sub_q)
    sp.start()
    print("subclass_proc:", sub_q.get(timeout=20.0))
    sp.join(timeout=20.0)
    print("subclass_exit:", sp.exitcode)

    # Terminate a sleeping child process
    pt = ctx.Process(target=worker_sleep_forever)
    pt.start()
    pt.terminate()
    pt.join(timeout=20.0)
    print("terminated:", not pt.is_alive(), pt.exitcode is not None and pt.exitcode != 0)
    pt.close()

    # ── 3. Pipe, Connection, wait(), Listener & Client ───────────────────────

    parent_c, child_c = ctx.Pipe(duplex=True)
    pp = ctx.Process(target=worker_pipe, args=(child_c,))
    pp.start()
    child_c.close()
    parent_c.send((13, 29))
    print("pipe_poll_recv:", parent_c.poll(timeout=20.0), parent_c.recv())
    parent_c.send_bytes(b"hello")
    buf = bytearray(5)
    n_read = parent_c.recv_bytes_into(buf)
    print("pipe_bytes:", n_read, bytes(buf))
    try:
        parent_c.recv()
        print("pipe_eof: UNEXPECTED")
    except EOFError:
        print("pipe_eof: EOFError")
    parent_c.close()
    pp.join(timeout=20.0)

    # Unidirectional Pipe + BufferTooShort + connection.wait()
    r_conn, w_conn = ctx.Pipe(duplex=False)
    print("pipe_rw:", r_conn.readable, r_conn.writable, w_conn.readable, w_conn.writable)
    pu = ctx.Process(target=worker_unidirectional_pipe, args=(w_conn,))
    pu.start()
    w_conn.close()
    ready_list = mp_conn.wait([r_conn], timeout=20.0)
    print("wait_pipe:", len(ready_list) == 1 and ready_list[0] is r_conn, r_conn.recv())
    r_conn.close()
    pu.join(timeout=20.0)

    # BufferTooShort in-process
    c1, c2 = mp.Pipe()
    c1.send_bytes(b"0123456789")
    small = bytearray(4)
    try:
        c2.recv_bytes_into(small)
        print("buf_short: UNEXPECTED")
    except mp.BufferTooShort as exc:
        print("buf_short:", exc.args[0])
    c1.close()
    c2.close()

    # Listener & Client loopback with authkey (cross-process)
    auth = b"secret_auth_key_3264"
    cli_q = ctx.Queue()
    with mp_conn.Listener(("127.0.0.1", 0), authkey=auth) as listener:
        addr = listener.address
        pc = ctx.Process(target=worker_client_connect, args=(addr, auth, cli_q))
        pc.start()
        with listener.accept() as srv_conn:
            req = srv_conn.recv()
            srv_conn.send({"ack": req["hello"]})
        pc.join(timeout=20.0)
    print("listener_client:", cli_q.get(timeout=20.0))

    # Wrong authkey raises AuthenticationError
    err_box = []
    with mp_conn.Listener(("127.0.0.1", 0), authkey=b"right_key") as listener2:
        addr2 = listener2.address

        def bad_client():
            try:
                with mp_conn.Client(addr2, authkey=b"wrong_key"):
                    pass
            except mp.AuthenticationError:
                err_box.append("client_auth_err")

        t_bad = threading.Thread(target=bad_client)
        t_bad.start()
        try:
            listener2.accept()
        except mp.AuthenticationError:
            err_box.append("server_auth_err")
        t_bad.join(timeout=10.0)
    print("auth_reject:", sorted(err_box))

    # ── 4. Synchronization + Value & Array across processes ──────────────────

    shared_val = ctx.Value("i", 100)
    shared_arr = ctx.Array("d", [1.5, 2.5, 3.5])
    shared_lock = ctx.Lock()
    ev_ready = ctx.Event()
    ev_go = ctx.Event()

    ps1 = ctx.Process(
        target=worker_sync_and_shared,
        args=(shared_val, shared_arr, shared_lock, ev_ready, ev_go),
    )
    ps1.start()
    print("ev_ready:", ev_ready.wait(timeout=20.0), ev_ready.is_set())
    ev_go.set()
    ps1.join(timeout=20.0)
    print("shared_val_arr:", shared_val.value, list(shared_arr), len(shared_arr))

    # RawValue & RawArray & ctypes type support
    rv = ctx.RawValue(ctypes.c_int, 77)
    ra = ctx.RawArray("i", [10, 20, 30])
    print("raw_val_arr:", rv.value, list(ra), ra[1:3])

    # RLock, Semaphore, BoundedSemaphore, Barrier across processes
    rl = ctx.RLock()
    with rl:
        with rl:
            pass
    bsem = ctx.BoundedSemaphore(2)
    bsem.acquire()
    bsem.release()
    try:
        bsem.release()
        print("bsem_overflow: UNEXPECTED")
    except ValueError:
        print("bsem_overflow: ValueError")

    bar = ctx.Barrier(2)
    bar_q = ctx.Queue()
    pb1 = ctx.Process(target=worker_barrier, args=(bar, bar_q, 1))
    pb2 = ctx.Process(target=worker_barrier, args=(bar, bar_q, 2))
    pb1.start()
    pb2.start()
    bar_res = sorted(bar_q.get(timeout=20.0) for _ in range(2))
    pb1.join(timeout=20.0)
    pb2.join(timeout=20.0)
    print("barrier_cross_proc:", bar_res, bar.parties, bar.broken)

    # ── 5. shared_memory.SharedMemory & ShareableList across processes ───────

    shm = mp_shm.SharedMemory(create=True, size=32)
    sl = mp_shm.ShareableList([10, 2.5, "init", b"ab", True, None])
    try:
        shm.buf[0:6] = b"before"
        pshm = ctx.Process(target=worker_shm_write, args=(shm.name, sl.shm.name))
        pshm.start()
        pshm.join(timeout=20.0)
        print("shm_cross_proc:", bytes(shm.buf[0:6]), shm.size == 32)
        print("sl_cross_proc:", list(sl), len(sl), sl.count(True), sl.index("done"))

        # FileExistsError on duplicate create
        try:
            dup = mp_shm.SharedMemory(name=shm.name, create=True, size=16)
            dup.close()
            print("shm_exists: UNEXPECTED")
        except FileExistsError:
            print("shm_exists: FileExistsError")

        # FileNotFoundError on missing attach
        try:
            mp_shm.SharedMemory(name="pyrust_nonexistent_shm_3264", create=False)
            print("shm_missing: UNEXPECTED")
        except FileNotFoundError:
            print("shm_missing: FileNotFoundError")
    finally:
        sl.shm.close()
        sl.shm.unlink()
        shm.close()
        shm.unlink()

    # ── 6. Unix 'fork' context test ──────────────────────────────────────────

    if sys.platform != "win32":
        fctx = mp.get_context("fork")
        fq = fctx.Queue()
        fv = fctx.Value("i", 0)
        fev = fctx.Event()
        fp = fctx.Process(target=worker_fork_test, args=(fq, fv, fev))
        fp.start()
        assert fev.wait(timeout=15.0)
        fres = fq.get(timeout=15.0)
        fp.join(timeout=15.0)
        assert fres == ("forked", True)
        assert fv.value == 321
        assert fp.exitcode == 0

    print("multiprocessing_core_ok")
