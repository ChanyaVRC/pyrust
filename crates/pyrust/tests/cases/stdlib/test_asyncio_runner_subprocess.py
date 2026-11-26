"""Offline deterministic CPython 3.12 parity tests for asyncio.Runner,
subprocesses (exec/shell/Process/SubprocessProtocol), to_thread, wrap_future,
eager_task_factory, and event loop scheduling/handlers (issue #3254).
"""

import asyncio
import contextvars
import sys


# ---------------------------------------------------------------------------
# 1. asyncio.Runner context manager & shared context/loop
# ---------------------------------------------------------------------------
def test_runner():
    var = contextvars.ContextVar("runner_var", default="unset")

    async def set_val(v):
        var.set(v)
        return asyncio.get_running_loop()

    async def get_val():
        return (var.get(), asyncio.get_running_loop())

    with asyncio.Runner(debug=True) as runner:
        lp = runner.get_loop()
        assert lp.get_debug() is True
        lp1 = runner.run(set_val("persisted"))
        val, lp2 = runner.run(get_val())
        assert lp1 is lp and lp2 is lp
        assert val == "persisted"

    # Runner is closed after exiting context manager
    assert lp.is_closed()
    try:
        runner.get_loop()
        assert False, "expected RuntimeError on closed Runner"
    except RuntimeError:
        pass

    # Custom loop_factory
    created = []

    def my_factory():
        loop = asyncio.SelectorEventLoop()
        created.append(loop)
        return loop

    with asyncio.Runner(loop_factory=my_factory) as runner2:
        async def ping():
            return "pong"

        assert runner2.run(ping()) == "pong"
        assert runner2.get_loop() is created[0]
    assert created[0].is_closed()
    print("runner: ok")


# ---------------------------------------------------------------------------
# 2. Subprocesses: create_subprocess_exec, create_subprocess_shell, Process
# ---------------------------------------------------------------------------
async def test_subprocesses():
    # exec with stdin/stdout/stderr pipes + communicate
    proc = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "import sys; data = sys.stdin.read(); sys.stdout.write('OUT:' + data); sys.stderr.write('ERR:ok')",
        stdin=asyncio.subprocess.PIPE,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    assert isinstance(proc, asyncio.subprocess.Process)
    assert isinstance(proc.pid, int) and proc.pid > 0
    out, err = await proc.communicate(b"hello-sub")
    assert out == b"OUT:hello-sub"
    assert err == b"ERR:ok"
    assert proc.returncode == 0

    # Streaming stdout line-by-line + wait()
    proc2 = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "import sys; sys.stdout.write('L1\\nL2\\n'); sys.stdout.flush()",
        stdout=asyncio.subprocess.PIPE,
    )
    l1 = await proc2.stdout.readline()
    l2 = await proc2.stdout.readline()
    l3 = await proc2.stdout.readline()
    rc = await proc2.wait()
    assert l1.strip() == b"L1"
    assert l2.strip() == b"L2"
    assert l3 == b""
    assert rc == 0 and proc2.returncode == 0

    # Merged stderr=STDOUT
    proc3 = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "import sys; sys.stdout.write('A'); sys.stdout.flush(); sys.stderr.write('B'); sys.stderr.flush()",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    out3, err3 = await proc3.communicate()
    assert err3 is None
    assert b"A" in out3 and b"B" in out3
    assert proc3.returncode == 0

    # DEVNULL
    proc4 = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "print('discarded')",
        stdout=asyncio.subprocess.DEVNULL,
        stderr=asyncio.subprocess.DEVNULL,
    )
    out4, err4 = await proc4.communicate()
    assert out4 is None and err4 is None and proc4.returncode == 0

    # Shell execution
    shell_cmd = f'"{sys.executable}" -c "import sys; sys.stdout.write(\'SHELL_OK\')"'
    proc5 = await asyncio.create_subprocess_shell(
        shell_cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    out5, _ = await proc5.communicate()
    assert out5 == b"SHELL_OK" and proc5.returncode == 0

    # kill() on sleeping child
    proc6 = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "import time; time.sleep(30)",
        stdout=asyncio.subprocess.DEVNULL,
        stderr=asyncio.subprocess.DEVNULL,
    )
    proc6.kill()
    rc6 = await proc6.wait()
    assert rc6 != 0

    # Low-level loop.subprocess_exec with custom SubprocessProtocol
    loop = asyncio.get_running_loop()
    done_fut = loop.create_future()

    class CaptureSubprocProto(asyncio.SubprocessProtocol):
        def __init__(self):
            self.chunks = bytearray()
            self.exited = False

        def pipe_data_received(self, fd, data):
            if fd == 1:
                self.chunks.extend(data)

        def process_exited(self):
            self.exited = True

        def connection_lost(self, exc):
            if not done_fut.done():
                done_fut.set_result((bytes(self.chunks), self.exited))

    tr, proto = await loop.subprocess_exec(
        CaptureSubprocProto,
        sys.executable,
        "-c",
        "import sys; sys.stdout.write('PROTO_SUB')",
        stdin=None,
        stdout=asyncio.subprocess.PIPE,
        stderr=None,
    )
    assert isinstance(tr, asyncio.SubprocessTransport)
    assert tr.get_pid() > 0
    captured, exited = await done_fut
    assert captured == b"PROTO_SUB" and exited is True
    assert tr.get_returncode() == 0
    tr.close()
    print("subprocesses: ok")


# ---------------------------------------------------------------------------
# 3. to_thread, wrap_future, run_in_executor
# ---------------------------------------------------------------------------
async def test_threads_and_futures():
    cvar = contextvars.ContextVar("thread_var", default="init")
    cvar.set("from-async")

    def compute(a, b, *, mult=1):
        return (a + b) * mult, cvar.get()

    res = await asyncio.to_thread(compute, 3, 4, mult=10)
    assert res == (70, "from-async")

    def boom():
        raise ValueError("thread-err")

    try:
        await asyncio.to_thread(boom)
        assert False, "expected ValueError"
    except ValueError as e:
        assert str(e) == "thread-err"

    # run_in_executor(None, ...)
    loop = asyncio.get_running_loop()
    assert await loop.run_in_executor(None, pow, 2, 10) == 1024

    # wrap_future pass-through for asyncio.Future
    af = loop.create_future()
    assert asyncio.wrap_future(af) is af
    af.set_result("direct")
    assert await af == "direct"

    # wrap_future rejects non-future objects
    try:
        asyncio.wrap_future("not-a-future")
        assert False, "expected error"
    except (TypeError, AssertionError):
        pass
    print("threads_and_futures: ok")


# ---------------------------------------------------------------------------
# 4. eager_task_factory, create_eager_task_factory, handles, exception handler
# ---------------------------------------------------------------------------
async def test_eager_and_loop_controls():
    loop = asyncio.get_running_loop()

    # Default lazy task runs on next turn; eager_task_factory runs synchronously up to first await
    events = []

    async def immediate():
        events.append("immediate")
        return 99

    assert loop.get_task_factory() is None
    loop.set_task_factory(asyncio.eager_task_factory)
    assert loop.get_task_factory() is asyncio.eager_task_factory
    t = asyncio.create_task(immediate())
    events.append("after-create")
    assert t.done()
    assert await t == 99
    assert events == ["immediate", "after-create"]

    # create_eager_task_factory with custom Task subclass
    class CustomTask(asyncio.Task):
        custom_tag = "tagged"

    loop.set_task_factory(asyncio.create_eager_task_factory(CustomTask))
    t2 = asyncio.create_task(immediate())
    assert isinstance(t2, CustomTask) and t2.custom_tag == "tagged"
    assert await t2 == 99
    loop.set_task_factory(None)

    # Handle & TimerHandle scheduling and cancellation
    fired = []
    h1 = loop.call_soon(fired.append, "soon")
    h2 = loop.call_soon_threadsafe(fired.append, "threadsafe")
    h_cancel = loop.call_soon(fired.append, "never")
    h_cancel.cancel()
    assert h_cancel.cancelled() and not h1.cancelled() and not h2.cancelled()

    now = loop.time()
    th1 = loop.call_later(0.01, fired.append, "later")
    th2 = loop.call_at(now + 0.02, fired.append, "at")
    assert isinstance(th1, asyncio.TimerHandle)
    assert isinstance(th1, asyncio.Handle)
    assert th1.when() <= th2.when()
    await asyncio.sleep(0.04)
    assert fired == ["soon", "threadsafe", "later", "at"]

    # Custom exception handler
    caught_contexts = []

    def my_handler(lp, ctx):
        caught_contexts.append(ctx["message"])

    assert loop.get_exception_handler() is None
    loop.set_exception_handler(my_handler)
    assert loop.get_exception_handler() is my_handler
    loop.call_exception_handler({"message": "custom-err"})
    assert caught_contexts == ["custom-err"]
    loop.set_exception_handler(None)
    print("eager_and_loop_controls: ok")


test_runner()


async def main():
    await test_subprocesses()
    await test_threads_and_futures()
    await test_eager_and_loop_controls()


asyncio.run(main())
