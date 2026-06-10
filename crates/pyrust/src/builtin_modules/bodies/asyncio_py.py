# Python-level surface of the `asyncio` module: a real single-threaded event
# loop (issue #2281).
#
# Exec'd once into a private namespace and copied onto the module by
# `asyncio::inject_python_members` (wired from `env.rs::load_module`).  The
# native bridge functions `_step`, `_throw`, `_iscoroutine`, `_monotonic` and
# `_sleep` are pre-seeded into that namespace; everything else (the loop,
# `Future`, `Task`, `sleep`, `gather`, `create_task`, `ensure_future`) is
# defined here in Python, mirroring CPython's own mostly-Python asyncio.
#
# Yield protocol: `Future.__await__` does `yield self` while pending.  That
# `self` bubbles up the `await`/`yield from` chain to the Task driver, which
# resumes the coroutine one step via `_step(coro, value)`.  When `_step`
# reports the coroutine suspended on a Future, the Task registers a
# done-callback that re-schedules itself, and stops; when the Future resolves
# (a timer fires, or another task completes) the callback runs and the loop
# steps the Task again.


class CancelledError(BaseException):
    """The operation has been cancelled (matches CPython's
    asyncio.CancelledError, which subclasses BaseException since 3.8)."""


class InvalidStateError(Exception):
    """The operation is not allowed in this state."""


# asyncio.TimeoutError is the builtin TimeoutError since CPython 3.11 (it used
# to be a distinct subclass of OSError-era TimeoutError). Re-export the builtin
# so `asyncio.TimeoutError is TimeoutError` and `except asyncio.TimeoutError`
# both behave like CPython 3.12.
TimeoutError = TimeoutError


_PENDING = "PENDING"
_CANCELLED = "CANCELLED"
_FINISHED = "FINISHED"


class Future:
    """A minimal asyncio.Future: a result-or-exception holder that a coroutine
    can `await`, plus done-callbacks the event loop uses to wake waiters."""

    def __init__(self, loop=None):
        self._loop = loop if loop is not None else _get_running_loop()
        self._state = _PENDING
        self._result = None
        self._exception = None
        self._callbacks = []
        # Cancellation message (passed to CancelledError), set by cancel().
        self._cancel_message = None
        # Sentinel the Task driver checks to recognise a Future bubbling up
        # through `await` (CPython uses the same `_asyncio_future_blocking`).
        self._asyncio_future_blocking = False

    def done(self):
        return self._state != _PENDING

    def cancelled(self):
        return self._state == _CANCELLED

    def cancel(self, msg=None):
        """Cancel the future. Returns True if it transitioned to cancelled,
        False if it was already done (CPython Future.cancel semantics)."""
        if self._state != _PENDING:
            return False
        self._state = _CANCELLED
        self._cancel_message = msg
        self._schedule_callbacks()
        return True

    def _make_cancelled_error(self):
        if self._cancel_message is None:
            return CancelledError()
        return CancelledError(self._cancel_message)

    def result(self):
        if self._state == _CANCELLED:
            raise self._make_cancelled_error()
        if self._state != _FINISHED:
            raise InvalidStateError("Result is not set.")
        if self._exception is not None:
            raise self._exception
        return self._result

    def exception(self):
        if self._state == _CANCELLED:
            raise self._make_cancelled_error()
        if self._state != _FINISHED:
            raise InvalidStateError("Exception is not set.")
        return self._exception

    def set_result(self, result):
        if self._state != _PENDING:
            raise InvalidStateError("invalid state")
        self._result = result
        self._state = _FINISHED
        self._schedule_callbacks()

    def set_exception(self, exception):
        if self._state != _PENDING:
            raise InvalidStateError("invalid state")
        if isinstance(exception, type):
            exception = exception()
        self._exception = exception
        self._state = _FINISHED
        self._schedule_callbacks()

    def add_done_callback(self, cb):
        if self._state != _PENDING:
            self._loop.call_soon(cb, self)
        else:
            self._callbacks.append(cb)

    def _schedule_callbacks(self):
        callbacks = self._callbacks
        self._callbacks = []
        for cb in callbacks:
            self._loop.call_soon(cb, self)

    def __await__(self):
        if not self.done():
            self._asyncio_future_blocking = True
            yield self  # the Task driver suspends here until we are resolved
        if not self.done():
            raise RuntimeError("await wasn't used with future")
        return self.result()

    # `await fut` and `yield from fut` share the iterator protocol.
    __iter__ = __await__


class Task(Future):
    """A Future that drives a coroutine: it steps the coroutine until the
    coroutine awaits a *pending* Future, suspends on that Future, and resumes
    when the Future resolves."""

    def __init__(self, coro, loop=None):
        super().__init__(loop=loop)
        self._coro = coro
        # The pending Future this task is currently suspended on (None when the
        # task is running or not yet suspended). Used by cancel() to inject a
        # CancelledError at the task's await point.
        self._fut_waiter = None
        # Set by cancel() when the task is not currently suspended on a Future:
        # the next step throws CancelledError into the coroutine.
        self._must_cancel = False
        # Kick the coroutine off on the next loop turn.
        self._loop.call_soon(self._step_run)

    def cancel(self, msg=None):
        """Request cancellation of the task. Returns True if cancellation was
        requested, False if the task is already done (CPython Task.cancel).

        The CancelledError is injected at the task's current await point on the
        next loop turn; the coroutine may catch and absorb it."""
        if self.done():
            return False
        if self._fut_waiter is not None:
            # Suspended on a Future: cancelling it wakes the task with a
            # CancelledError (via _wakeup -> future.result()).
            if self._fut_waiter.cancel(msg=msg):
                return True
        # Not suspended on a cancellable Future: throw at the next step.
        self._must_cancel = True
        self._cancel_message = msg
        return True

    def _step_run(self, exc=None):
        if self.done():
            return
        if self._must_cancel:
            # A cancel() landed while the task was not blocked on a Future:
            # turn it into a CancelledError injected at the await point.
            if exc is None:
                exc = self._make_cancelled_error()
            self._must_cancel = False
        self._fut_waiter = None
        try:
            if exc is None:
                state, value = _step(self._coro, None)
            else:
                state, value = _throw(self._coro, exc)
        except CancelledError:
            # The coroutine did not absorb the cancellation: the task ends
            # cancelled (Future state CANCELLED, not a FINISHED exception).
            Future.cancel(self, msg=self._cancel_message)
            return
        except BaseException as e:
            self.set_exception(e)
            return

        if state == 1:
            # Coroutine returned: `value` is its result. (If a cancellation was
            # caught and the coroutine returned normally, the cancellation is
            # absorbed and the task finishes with this result.)
            self.set_result(value)
            return

        # state == 0: coroutine suspended, yielding `value`.
        if isinstance(value, Future):
            # Awaiting a (pending) Future: resume this task when it resolves.
            value._asyncio_future_blocking = False
            self._fut_waiter = value
            value.add_done_callback(self._wakeup)
            if self._must_cancel:
                # A cancel() arrived during this step: propagate to the waiter.
                if self._fut_waiter.cancel(msg=self._cancel_message):
                    self._must_cancel = False
        elif value is None:
            # A bare `yield None` (sleep(0) fairness point): reschedule.
            self._loop.call_soon(self._step_run)
        else:
            # Anything else yielded out of `await` is a protocol error.
            self._loop.call_soon(
                self._step_run,
                RuntimeError("Task got bad yield: " + repr(value)),
            )

    def _wakeup(self, future):
        self._fut_waiter = None
        try:
            future.result()
        except BaseException as exc:
            # The awaited future failed (or was cancelled): throw into the coro.
            self._step_run(exc)
        else:
            self._step_run()


class _Handle:
    """A scheduled callback (call_soon / call_at)."""

    def __init__(self, callback, args):
        self._callback = callback
        self._args = args
        self._cancelled = False

    def cancel(self):
        self._cancelled = True

    def _run(self):
        if self._cancelled:
            return
        self._callback(*self._args)


class _EventLoop:
    """A single-threaded event loop: a ready queue plus a time-ordered list of
    timer handles.  No real I/O — the only blocking is `_sleep` until the next
    timer when the ready queue is empty."""

    def __init__(self):
        self._ready = []  # list of _Handle, FIFO
        self._scheduled = []  # list of (when, _Handle), kept sorted by `when`
        self._stopping = False

    def time(self):
        return _monotonic()

    def call_soon(self, callback, *args):
        h = _Handle(callback, args)
        self._ready.append(h)
        return h

    def call_at(self, when, callback, *args):
        h = _Handle(callback, args)
        # Insert keeping `_scheduled` sorted by wake time (stable for equal
        # deadlines — preserves scheduling order).
        i = 0
        n = len(self._scheduled)
        while i < n and self._scheduled[i][0] <= when:
            i += 1
        self._scheduled.insert(i, (when, h))
        return h

    def call_later(self, delay, callback, *args):
        return self.call_at(self.time() + delay, callback, *args)

    def _run_once(self):
        # If nothing is ready, wait until the earliest timer is due.
        if not self._ready and self._scheduled:
            when = self._scheduled[0][0]
            now = self.time()
            if when > now:
                _sleep(when - now)

        # Move all due timers into the ready queue.
        now = self.time()
        while self._scheduled and self._scheduled[0][0] <= now:
            self._ready.append(self._scheduled.pop(0)[1])

        # Drain exactly the handles ready at the start of this turn (handles
        # appended during this turn run on the next turn — CPython semantics).
        ntodo = len(self._ready)
        for _ in range(ntodo):
            h = self._ready.pop(0)
            h._run()

    def run_until_complete(self, future):
        future.add_done_callback(self._stop_on_done)
        while not self._stopping:
            if not self._ready and not self._scheduled:
                break
            self._run_once()
        self._stopping = False
        if not future.done():
            raise RuntimeError("Event loop stopped before Future completed.")
        return future.result()

    def _stop_on_done(self, future):
        self._stopping = True


# The loop currently running on this thread (asyncio is single-threaded here).
_running_loop = None


def _get_running_loop():
    if _running_loop is None:
        raise RuntimeError("no running event loop")
    return _running_loop


def get_running_loop():
    if _running_loop is None:
        raise RuntimeError("no running event loop")
    return _running_loop


def get_event_loop():
    if _running_loop is not None:
        return _running_loop
    raise RuntimeError("no running event loop")


def ensure_future(obj, loop=None):
    """Wrap a coroutine in a Task; pass Futures/Tasks through unchanged."""
    if isinstance(obj, Future):
        return obj
    if _iscoroutine(obj):
        return Task(obj, loop=loop)
    raise TypeError(
        "An asyncio.Future, a coroutine or an awaitable is required"
    )


def create_task(coro, *, name=None):
    """Schedule `coro` to run concurrently and return the Task driving it."""
    if not _iscoroutine(coro):
        raise TypeError("a coroutine was expected, got " + repr(coro))
    return Task(coro, loop=_get_running_loop())


class _YieldOnce:
    """An awaitable that yields control to the loop exactly once."""

    def __await__(self):
        yield None

    __iter__ = __await__


async def sleep(delay, result=None):
    """Suspend the current task for `delay` seconds, then return `result`.

    `sleep(0)` (or any non-positive delay) yields control for one loop turn
    without a real wait, letting other ready tasks run."""
    if delay <= 0:
        # Fairness point: bare `yield` lets the loop run other ready tasks.
        await _YieldOnce()
        return result
    loop = _get_running_loop()
    future = Future(loop=loop)
    loop.call_later(delay, _set_result_unless_done, future, result)
    return await future


def _set_result_unless_done(fut, value):
    if not fut.done():
        fut.set_result(value)


class _GatheringFuture(Future):
    """The Future returned by `gather`: resolves to a list of child results in
    argument order once every child completes, or to the first child exception
    as soon as one occurs (default return_exceptions=False)."""

    def __init__(self, children, loop):
        super().__init__(loop=loop)
        self._children = children
        self._nfinished = 0
        self._nchildren = len(children)
        for i, child in enumerate(children):
            child.add_done_callback(self._make_done_cb(i))

    def _make_done_cb(self, index):
        def _done(child):
            if self.done():
                return
            if child.cancelled():
                # A child was cancelled: propagate cancellation to the gather
                # (default return_exceptions=False).
                Future.cancel(self)
                return
            exc = child.exception()
            if exc is not None:
                # First child error: propagate it immediately.
                self.set_exception(exc)
                return
            self._nfinished += 1
            if self._nfinished == self._nchildren:
                self.set_result([c.result() for c in self._children])

        return _done

    def cancel(self, msg=None):
        """Cancelling the gather cancels every still-pending child. The gather
        future itself transitions to cancelled only once a child wakes with a
        CancelledError and reports it via `_done` (CPython semantics — this
        preserves the ordering where the children observe the cancellation at
        their await points before the gather's awaiter resumes)."""
        if self.done():
            return False
        ret = False
        for child in self._children:
            if child.cancel(msg=msg):
                ret = True
        return ret


def gather(*aws):
    """Run the awaitables concurrently; return a Future resolving to their
    results in argument order.

    Like CPython, `gather` is a plain function that returns the
    `_GatheringFuture` immediately (so it can be `.cancel()`led before being
    awaited); `await asyncio.gather(...)` awaits that future. The default
    (return_exceptions=False) propagates the first child exception, or
    cancellation, as soon as it occurs."""
    loop = _get_running_loop()
    if not aws:
        # An empty gather resolves immediately to an empty list.
        fut = Future(loop=loop)
        fut.set_result([])
        return fut
    children = [ensure_future(a, loop=loop) for a in aws]
    return _GatheringFuture(children, loop)


async def wait_for(aw, timeout):
    """Wait for `aw` to complete within `timeout` seconds.

    Returns its result if it finishes in time. On timeout, cancels the inner
    operation and raises `TimeoutError` (the builtin, per CPython 3.12). A
    `timeout` of None waits forever (plain await)."""
    loop = _get_running_loop()
    if timeout is None:
        return await ensure_future(aw, loop=loop)

    fut = ensure_future(aw, loop=loop)

    # A one-element box so the timer callback can flag that *it* triggered the
    # cancellation (vs an external cancel), distinguishing TimeoutError from a
    # propagated CancelledError.
    timed_out = [False]

    def _on_timeout():
        if not fut.done():
            timed_out[0] = True
            fut.cancel()

    timer = loop.call_later(timeout, _on_timeout)
    try:
        return await fut
    except CancelledError:
        if timed_out[0]:
            raise TimeoutError() from None
        raise
    finally:
        timer.cancel()


def _run_main(coro):
    """Entry point used by the native `asyncio.run`: build a fresh loop, run
    `coro` as the root task to completion, and return its result."""
    global _running_loop
    if _running_loop is not None:
        raise RuntimeError(
            "asyncio.run() cannot be called from a running event loop"
        )
    loop = _EventLoop()
    _running_loop = loop
    try:
        task = Task(coro, loop=loop)
        return loop.run_until_complete(task)
    finally:
        _running_loop = None
