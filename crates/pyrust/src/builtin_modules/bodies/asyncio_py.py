# Python-level surface of the `asyncio` module: a real single-threaded event
# loop (issues #2281, #3154).
#
# Exec'd once into a private namespace and copied onto the module by
# `asyncio::inject_python_members` (wired from `env.rs::load_module`).  The
# native bridge functions `_step`, `_throw`, `_iscoroutine`, `_monotonic` and
# `_sleep` are pre-seeded into that namespace; everything else (the loop,
# `Future`, `Task`, `TaskGroup`, `Timeout`, `Queue`, synchronization primitives,
# `sleep`, `gather`, `wait`, `wait_for`, `as_completed`, `shield`,
# `create_task`, `ensure_future`, `current_task`, `all_tasks`) is defined here
# in Python, mirroring CPython's own mostly-Python asyncio.
#
# Yield protocol: `Future.__await__` does `yield self` while pending.  That
# `self` bubbles up the `await`/`yield from` chain to the Task driver, which
# resumes the coroutine one step via `_step(coro, value)`.  When `_step`
# reports the coroutine suspended on a Future, the Task registers a
# done-callback that re-schedules itself, and stops; when the Future resolves
# (a timer fires, or another task completes) the callback runs and the loop
# steps the Task again.

from collections import deque as _deque
import heapq as _heapq
from types import GenericAlias as _GenericAlias


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


class CancelledError(BaseException):
    """The operation has been cancelled (matches CPython's
    asyncio.CancelledError, which subclasses BaseException since 3.8)."""

    __module__ = "asyncio.exceptions"


class InvalidStateError(Exception):
    """The operation is not allowed in this state."""

    __module__ = "asyncio.exceptions"


class BrokenBarrierError(RuntimeError):
    """Barrier is broken by barrier.abort() call."""

    __module__ = "asyncio.exceptions"


# asyncio.TimeoutError is the builtin TimeoutError since CPython 3.11 (it used
# to be a distinct subclass of OSError-era TimeoutError). Re-export the builtin
# so `asyncio.TimeoutError is TimeoutError` and `except asyncio.TimeoutError`
# both behave like CPython 3.12.
TimeoutError = TimeoutError


# ---------------------------------------------------------------------------
# Future & Task states / constants
# ---------------------------------------------------------------------------

_PENDING = "PENDING"
_CANCELLED = "CANCELLED"
_FINISHED = "FINISHED"

FIRST_COMPLETED = "FIRST_COMPLETED"
FIRST_EXCEPTION = "FIRST_EXCEPTION"
ALL_COMPLETED = "ALL_COMPLETED"


def isfuture(obj):
    """Check for a Future (duck-type compatible with CPython base_futures)."""
    return (
        hasattr(obj.__class__, "_asyncio_future_blocking")
        and obj._asyncio_future_blocking is not None
    )


def _isawaitable(obj):
    if _iscoroutine(obj) or isfuture(obj):
        return True
    return hasattr(type(obj), "__await__")


# ---------------------------------------------------------------------------
# Future
# ---------------------------------------------------------------------------


class Future:
    """An asyncio.Future: a result-or-exception holder that a coroutine can
    `await`, plus done-callbacks the event loop uses to wake waiters."""

    __module__ = "_asyncio"

    # Class-level marker checked by `isfuture()` and the Task driver.
    _asyncio_future_blocking = False

    __class_getitem__ = classmethod(
        lambda cls, item: _GenericAlias(cls, item)
    )

    def __init__(self, *, loop=None):
        self._loop = loop if loop is not None else _get_running_loop()
        self._state = _PENDING
        self._result = None
        self._exception = None
        self._callbacks = []
        # Cancellation message (passed to CancelledError), set by cancel().
        self._cancel_message = None
        # Saved CancelledError for chaining/propagation when a task cancels.
        self._cancelled_exc = None
        self._asyncio_future_blocking = False

    def get_loop(self):
        """Return the event loop the Future is bound to."""
        loop = self._loop
        if loop is None:
            raise RuntimeError("Future object is not initialized.")
        return loop

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
        if self._cancelled_exc is not None:
            exc = self._cancelled_exc
            self._cancelled_exc = None
            return exc
        if self._cancel_message is None:
            return CancelledError()
        return CancelledError(self._cancel_message)

    def result(self):
        if self._state == _CANCELLED:
            raise self._make_cancelled_error()
        if self._state != _FINISHED:
            raise InvalidStateError("Result is not ready.")
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
        if type(exception) is StopIteration:
            raise TypeError(
                "StopIteration interacts badly with generators "
                "and cannot be raised into a Future"
            )
        self._exception = exception
        self._state = _FINISHED
        self._schedule_callbacks()

    def add_done_callback(self, fn, *, context=None):
        if self._state != _PENDING:
            self._loop.call_soon(fn, self)
        else:
            self._callbacks.append(fn)

    def remove_done_callback(self, fn):
        """Remove all instances of a callback from the done-callback list.
        Returns the number of callbacks removed."""
        filtered = [cb for cb in self._callbacks if cb != fn]
        removed = len(self._callbacks) - len(filtered)
        if removed:
            self._callbacks[:] = filtered
        return removed

    def _schedule_callbacks(self):
        callbacks = self._callbacks[:]
        if not callbacks:
            return
        self._callbacks[:] = []
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


# ---------------------------------------------------------------------------
# Task registry & naming
# ---------------------------------------------------------------------------

_task_name_counter = 0
_scheduled_tasks = set()
_current_tasks = {}


def _next_task_name():
    global _task_name_counter
    _task_name_counter += 1
    return f"Task-{_task_name_counter}"


def _register_task(task):
    _scheduled_tasks.add(task)


def _unregister_task(task):
    _scheduled_tasks.discard(task)


def _enter_task(loop, task):
    cur = _current_tasks.get(loop)
    if cur is not None:
        raise RuntimeError(
            f"Cannot enter into task {task!r} while another "
            f"task {cur!r} is being executed."
        )
    _current_tasks[loop] = task


def _leave_task(loop, task):
    cur = _current_tasks.get(loop)
    if cur is not task:
        raise RuntimeError(
            f"Leaving task {task!r} does not match "
            f"the current task {cur!r}."
        )
    del _current_tasks[loop]


def current_task(loop=None):
    """Return the currently running Task in `loop` (or the running loop)."""
    if loop is None:
        loop = get_running_loop()
    return _current_tasks.get(loop)


def all_tasks(loop=None):
    """Return a set of all not-yet-finished Tasks for `loop`."""
    if loop is None:
        loop = get_running_loop()
    return {
        t for t in list(_scheduled_tasks)
        if t._loop is loop and not t.done()
    }


def _set_task_name(task, name):
    if name is not None:
        task.set_name(name)


# ---------------------------------------------------------------------------
# Task
# ---------------------------------------------------------------------------


class Task(Future):
    """A Future that drives a coroutine: it steps the coroutine until the
    coroutine awaits a *pending* Future, suspends on that Future, and resumes
    when the Future resolves."""

    __module__ = "_asyncio"

    def __init__(self, coro, *, loop=None, name=None, context=None):
        super().__init__(loop=loop)
        if not _iscoroutine(coro):
            raise TypeError(f"a coroutine was expected, got {coro!r}")
        if name is None:
            self._name = _next_task_name()
        else:
            self._name = str(name)
        self._coro = coro
        self._context = context
        # Number of pending cancel() requests (CPython 3.11+ cancelling/uncancel).
        self._num_cancels_requested = 0
        # The pending Future this task is currently suspended on (None when the
        # task is running or not yet suspended).
        self._fut_waiter = None
        # Set by cancel() when the task is not currently suspended on a Future:
        # the next step throws CancelledError into the coroutine.
        self._must_cancel = False
        # Kick the coroutine off on the next loop turn.
        self._loop.call_soon(self._step_run)
        _register_task(self)

    def get_coro(self):
        return self._coro

    def get_name(self):
        return self._name

    def set_name(self, value):
        self._name = str(value)

    def set_result(self, result):
        raise RuntimeError("Task does not support set_result operation")

    def set_exception(self, exception):
        raise RuntimeError("Task does not support set_exception operation")

    def cancel(self, msg=None):
        """Request cancellation of the task. Returns True if cancellation was
        requested, False if the task is already done (CPython Task.cancel).

        Increments the task's cancellation request counter (`cancelling()`).
        """
        if self.done():
            return False
        self._num_cancels_requested += 1
        if self._fut_waiter is not None:
            # Suspended on a Future: cancelling it wakes the task with a
            # CancelledError (via _wakeup -> future.result()).
            if self._fut_waiter.cancel(msg=msg):
                return True
        # Not suspended on a cancellable Future: throw at the next step.
        self._must_cancel = True
        self._cancel_message = msg
        return True

    def cancelling(self):
        """Return the count of pending cancellation requests on this task."""
        return self._num_cancels_requested

    def uncancel(self):
        """Decrement the task's count of pending cancellation requests.
        Returns the remaining count."""
        if self._num_cancels_requested > 0:
            self._num_cancels_requested -= 1
        return self._num_cancels_requested

    def _finish_result(self, value):
        _unregister_task(self)
        Future.set_result(self, value)

    def _finish_exception(self, exc):
        _unregister_task(self)
        Future.set_exception(self, exc)

    def _finish_cancel(self, msg=None):
        _unregister_task(self)
        Future.cancel(self, msg=msg)

    def _step_run(self, exc=None):
        if self.done():
            return
        if self._must_cancel:
            if not isinstance(exc, CancelledError):
                exc = self._make_cancelled_error()
            self._must_cancel = False
        self._fut_waiter = None

        _enter_task(self._loop, self)
        try:
            self._step_run_and_handle_result(exc)
        finally:
            _leave_task(self._loop, self)

    def _step_run_and_handle_result(self, exc):
        try:
            if exc is None:
                state, value = _step(self._coro, None)
            else:
                state, value = _throw(self._coro, exc)
        except CancelledError as e:
            # Save the original exception so callers see the exact CancelledError.
            self._cancelled_exc = e
            self._finish_cancel(msg=self._cancel_message)
            return
        except (KeyboardInterrupt, SystemExit) as e:
            self._finish_exception(e)
            raise
        except BaseException as e:
            self._finish_exception(e)
            return

        if state == 1:
            # Coroutine returned: `value` is its result.
            if self._must_cancel:
                self._must_cancel = False
                self._finish_cancel(msg=self._cancel_message)
            else:
                self._finish_result(value)
            return

        # state == 0: coroutine suspended, yielding `value`.
        blocking = getattr(value, "_asyncio_future_blocking", None)
        if blocking is not None:
            if value._loop is not self._loop:
                self._loop.call_soon(
                    self._step_run,
                    RuntimeError(
                        f"Task {self!r} got Future "
                        f"{value!r} attached to a different loop"
                    ),
                )
            elif blocking:
                if value is self:
                    self._loop.call_soon(
                        self._step_run,
                        RuntimeError(f"Task cannot await on itself: {self!r}"),
                    )
                else:
                    value._asyncio_future_blocking = False
                    self._fut_waiter = value
                    value.add_done_callback(self._wakeup)
                    if self._must_cancel:
                        if self._fut_waiter.cancel(msg=self._cancel_message):
                            self._must_cancel = False
            else:
                self._loop.call_soon(
                    self._step_run,
                    RuntimeError(
                        f"yield was used instead of yield from "
                        f"in task {self!r} with {value!r}"
                    ),
                )
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


# ---------------------------------------------------------------------------
# Event loop
# ---------------------------------------------------------------------------


class _Handle:
    """A scheduled callback (call_soon / call_at)."""

    def __init__(self, callback, args):
        self._callback = callback
        self._args = args
        self._cancelled = False

    def cancel(self):
        self._cancelled = True

    def cancelled(self):
        return self._cancelled

    def _run(self):
        if self._cancelled:
            return
        self._callback(*self._args)


class _EventLoop:
    """A single-threaded event loop: a ready queue plus a time-ordered list of
    timer handles.  No real I/O — the only blocking is `_sleep` until the next
    timer when the ready queue is empty."""

    def __init__(self):
        self._ready = _deque()  # deque of _Handle, FIFO
        self._scheduled = []  # list of (when, _Handle), kept sorted by `when`
        self._stopping = False

    def time(self):
        return _monotonic()

    def is_running(self):
        return _running_loop is self

    def create_future(self):
        return Future(loop=self)

    def create_task(self, coro, *, name=None, context=None):
        return Task(coro, loop=self, name=name, context=context)

    def call_exception_handler(self, context):
        pass

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

    def _discard_cancelled_timers(self):
        while self._scheduled and self._scheduled[0][1]._cancelled:
            self._scheduled.pop(0)

    def _run_once(self):
        self._discard_cancelled_timers()

        # If nothing is ready, wait until the earliest live timer is due.
        if not self._ready and self._scheduled:
            when = self._scheduled[0][0]
            now = self.time()
            if when > now:
                _sleep(when - now)

        # Move all due live timers into the ready queue.
        now = self.time()
        while self._scheduled and self._scheduled[0][0] <= now:
            _, h = self._scheduled.pop(0)
            if not h._cancelled:
                self._ready.append(h)

        # Drain exactly the handles ready at the start of this turn (handles
        # appended during this turn run on the next turn — CPython semantics).
        ntodo = len(self._ready)
        for _ in range(ntodo):
            h = self._ready.popleft()
            h._run()

    def run_until_complete(self, future):
        future.add_done_callback(self._stop_on_done)
        while not self._stopping:
            self._discard_cancelled_timers()
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


# ---------------------------------------------------------------------------
# Task helpers: ensure_future, create_task, sleep, gather, wait, wait_for,
# as_completed, shield
# ---------------------------------------------------------------------------


def ensure_future(coro_or_future, *, loop=None):
    """Wrap a coroutine or awaitable in a Task; pass Futures/Tasks through."""
    if isfuture(coro_or_future):
        if loop is not None and loop is not coro_or_future._loop:
            raise ValueError(
                "The future belongs to a different loop than "
                "the one specified as the loop argument"
            )
        return coro_or_future
    if not _iscoroutine(coro_or_future):
        if _isawaitable(coro_or_future):
            async def _wrap_awaitable(awaitable):
                return await awaitable

            coro_or_future = _wrap_awaitable(coro_or_future)
        else:
            raise TypeError(
                "An asyncio.Future, a coroutine or an awaitable is required"
            )
    if loop is None:
        loop = get_event_loop()
    return loop.create_task(coro_or_future)


def create_task(coro, *, name=None, context=None):
    """Schedule `coro` to run concurrently and return the Task driving it."""
    loop = get_running_loop()
    if context is None:
        task = loop.create_task(coro)
    else:
        task = loop.create_task(coro, context=context)
    _set_task_name(task, name)
    return task


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
        await _YieldOnce()
        return result
    loop = get_running_loop()
    future = loop.create_future()
    h = loop.call_later(delay, _set_result_unless_done, future, result)
    try:
        return await future
    finally:
        h.cancel()


def _set_result_unless_done(fut, value):
    if not fut.done():
        fut.set_result(value)


def _release_waiter(waiter, *args):
    if not waiter.done():
        waiter.set_result(None)


class _GatheringFuture(Future):
    """Helper for `gather()`.

    Overrides `cancel()` to cancel all pending children without immediately
    marking itself cancelled (matches CPython)."""

    def __init__(self, children, *, loop):
        super().__init__(loop=loop)
        self._children = children
        self._cancel_requested = False

    def cancel(self, msg=None):
        if self.done():
            return False
        ret = False
        for child in self._children:
            if child.cancel(msg=msg):
                ret = True
        if ret:
            self._cancel_requested = True
        return ret


def gather(*coros_or_futures, return_exceptions=False):
    """Run the awaitables concurrently; return a Future resolving to their
    results in argument order.

    If `return_exceptions` is True, exceptions (including CancelledError) in
    children are returned in the result list instead of being raised. If the
    gather itself is cancelled, cancellation still propagates."""
    if not coros_or_futures:
        loop = get_event_loop()
        outer = loop.create_future()
        outer.set_result([])
        return outer

    def _done_callback(fut):
        nonlocal nfinished
        nfinished += 1

        if outer is None or outer.done():
            return

        if not return_exceptions:
            if fut.cancelled():
                exc = fut._make_cancelled_error()
                outer.set_exception(exc)
                return
            exc = fut.exception()
            if exc is not None:
                outer.set_exception(exc)
                return

        if nfinished == nfuts:
            results = []
            for child in children:
                if child.cancelled():
                    res = CancelledError(
                        ""
                        if child._cancel_message is None
                        else child._cancel_message
                    )
                else:
                    res = child.exception()
                    if res is None:
                        res = child.result()
                results.append(res)

            if outer._cancel_requested:
                exc = fut._make_cancelled_error()
                outer.set_exception(exc)
            else:
                outer.set_result(results)

    arg_to_fut = {}
    children = []
    nfuts = 0
    nfinished = 0
    done_futs = []
    loop = None
    outer = None
    for arg in coros_or_futures:
        if arg not in arg_to_fut:
            fut = ensure_future(arg, loop=loop)
            if loop is None:
                loop = fut._loop
            nfuts += 1
            arg_to_fut[arg] = fut
            if fut.done():
                done_futs.append(fut)
            else:
                fut.add_done_callback(_done_callback)
        else:
            fut = arg_to_fut[arg]
        children.append(fut)

    outer = _GatheringFuture(children, loop=loop)
    for fut in done_futs:
        _done_callback(fut)
    return outer


async def wait(fs, *, timeout=None, return_when=ALL_COMPLETED):
    """Wait for the Futures or Tasks given by `fs` to complete.

    Returns two sets: `(done, pending)`. Does not raise TimeoutError.
    Passing bare coroutines raises TypeError (Python 3.11+ semantics)."""
    if isfuture(fs) or _iscoroutine(fs):
        raise TypeError(f"expect a list of futures, not {type(fs).__name__}")
    if not fs:
        raise ValueError("Set of Tasks/Futures is empty.")
    if return_when not in (FIRST_COMPLETED, FIRST_EXCEPTION, ALL_COMPLETED):
        raise ValueError(f"Invalid return_when value: {return_when}")

    fs = set(fs)
    if any(_iscoroutine(f) for f in fs):
        raise TypeError("Passing coroutines is forbidden, use tasks explicitly.")

    loop = get_running_loop()
    return await _wait(fs, timeout, return_when, loop)


async def _wait(fs, timeout, return_when, loop):
    waiter = loop.create_future()
    timeout_handle = None
    if timeout is not None:
        timeout_handle = loop.call_later(timeout, _release_waiter, waiter)
    counter = len(fs)

    def _on_completion(f):
        nonlocal counter
        counter -= 1
        if (
            counter <= 0
            or return_when == FIRST_COMPLETED
            or (
                return_when == FIRST_EXCEPTION
                and (not f.cancelled() and f.exception() is not None)
            )
        ):
            if timeout_handle is not None:
                timeout_handle.cancel()
            if not waiter.done():
                waiter.set_result(None)

    for f in fs:
        f.add_done_callback(_on_completion)

    try:
        await waiter
    finally:
        if timeout_handle is not None:
            timeout_handle.cancel()
        for f in fs:
            f.remove_done_callback(_on_completion)

    done, pending = set(), set()
    for f in fs:
        if f.done():
            done.add(f)
        else:
            pending.add(f)
    return done, pending


async def _cancel_and_wait(fut):
    """Cancel `fut` and wait until it completes."""
    loop = get_running_loop()
    waiter = loop.create_future()

    def _cb(_f):
        _release_waiter(waiter)

    fut.add_done_callback(_cb)
    try:
        fut.cancel()
        await waiter
    finally:
        fut.remove_done_callback(_cb)


async def wait_for(fut, timeout):
    """Wait for a single Future or coroutine to complete, with timeout."""
    if timeout is not None and timeout <= 0:
        fut = ensure_future(fut)
        if fut.done():
            return fut.result()
        await _cancel_and_wait(fut)
        try:
            return fut.result()
        except CancelledError as exc:
            raise TimeoutError from exc

    async with _timeout_cm(timeout):
        return await fut


def as_completed(fs, *, timeout=None):
    """Return an iterator of coroutines yielding results in completion order."""
    if isfuture(fs) or _iscoroutine(fs):
        raise TypeError(
            f"expect an iterable of futures, not {type(fs).__name__}"
        )

    done = Queue()
    loop = get_event_loop()
    todo = {ensure_future(f, loop=loop) for f in set(fs)}
    timeout_handle = None

    def _on_timeout():
        for f in todo:
            f.remove_done_callback(_on_completion)
            done.put_nowait(None)
        todo.clear()

    def _on_completion(f):
        if not todo:
            return
        todo.remove(f)
        done.put_nowait(f)
        if not todo and timeout_handle is not None:
            timeout_handle.cancel()

    async def _wait_for_one():
        f = await done.get()
        if f is None:
            raise TimeoutError
        return f.result()

    for f in todo:
        f.add_done_callback(_on_completion)
    if todo and timeout is not None:
        timeout_handle = loop.call_later(timeout, _on_timeout)
    for _ in range(len(todo)):
        yield _wait_for_one()


def shield(arg):
    """Wait for a future/coroutine, shielding it from outer cancellation."""
    inner = ensure_future(arg)
    if inner.done():
        return inner
    loop = inner._loop
    outer = loop.create_future()

    def _inner_done_callback(inner):
        if outer.cancelled():
            return
        if inner.cancelled():
            outer.cancel()
        else:
            exc = inner.exception()
            if exc is not None:
                outer.set_exception(exc)
            else:
                outer.set_result(inner.result())

    def _outer_done_callback(outer):
        if not inner.done():
            inner.remove_done_callback(_inner_done_callback)

    inner.add_done_callback(_inner_done_callback)
    outer.add_done_callback(_outer_done_callback)
    return outer


# ---------------------------------------------------------------------------
# Structured concurrency: Timeout, timeout(), timeout_at(), TaskGroup
# ---------------------------------------------------------------------------

_TIMEOUT_CREATED = "created"
_TIMEOUT_ENTERED = "active"
_TIMEOUT_EXPIRING = "expiring"
_TIMEOUT_EXPIRED = "expired"
_TIMEOUT_EXITED = "finished"


class Timeout:
    """Asynchronous context manager for cancelling overdue coroutines."""

    __module__ = "asyncio.timeouts"

    def __init__(self, when):
        self._state = _TIMEOUT_CREATED
        self._timeout_handler = None
        self._task = None
        self._when = when

    def when(self):
        """Return the current deadline."""
        return self._when

    def reschedule(self, when):
        """Reschedule the timeout."""
        if self._state != _TIMEOUT_ENTERED:
            if self._state == _TIMEOUT_CREATED:
                raise RuntimeError("Timeout has not been entered")
            raise RuntimeError(
                f"Cannot change state of {self._state} Timeout"
            )

        self._when = when
        if self._timeout_handler is not None:
            self._timeout_handler.cancel()

        if when is None:
            self._timeout_handler = None
        else:
            loop = get_running_loop()
            if when <= loop.time():
                self._timeout_handler = loop.call_soon(self._on_timeout)
            else:
                self._timeout_handler = loop.call_at(when, self._on_timeout)

    def expired(self):
        """Return True if timeout expired during execution."""
        return self._state in (_TIMEOUT_EXPIRING, _TIMEOUT_EXPIRED)

    def __repr__(self):
        info = [""]
        if self._state == _TIMEOUT_ENTERED:
            when = round(self._when, 3) if self._when is not None else None
            info.append(f"when={when}")
        info_str = " ".join(info)
        return f"<Timeout [{self._state}]{info_str}>"

    async def __aenter__(self):
        if self._state != _TIMEOUT_CREATED:
            raise RuntimeError("Timeout has already been entered")
        task = current_task()
        if task is None:
            raise RuntimeError("Timeout should be used inside a task")
        self._state = _TIMEOUT_ENTERED
        self._task = task
        self._cancelling = self._task.cancelling()
        self.reschedule(self._when)
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        if self._timeout_handler is not None:
            self._timeout_handler.cancel()
            self._timeout_handler = None

        if self._state == _TIMEOUT_EXPIRING:
            self._state = _TIMEOUT_EXPIRED
            if (
                self._task.uncancel() <= self._cancelling
                and exc_type is CancelledError
            ):
                raise TimeoutError from exc_val
        elif self._state == _TIMEOUT_ENTERED:
            self._state = _TIMEOUT_EXITED

        return None

    def _on_timeout(self):
        self._task.cancel()
        self._state = _TIMEOUT_EXPIRING
        self._timeout_handler = None


def timeout(delay):
    """Timeout async context manager with a relative `delay` in seconds."""
    loop = get_running_loop()
    return Timeout(loop.time() + delay if delay is not None else None)


_timeout_cm = timeout


def timeout_at(when):
    """Timeout async context manager with an absolute `loop.time()` deadline."""
    return Timeout(when)


class TaskGroup:
    """Asynchronous context manager for managing groups of tasks (PEP 654)."""

    __module__ = "asyncio.taskgroups"

    def __init__(self):
        self._entered = False
        self._exiting = False
        self._aborting = False
        self._loop = None
        self._parent_task = None
        self._parent_cancel_requested = False
        self._tasks = set()
        self._errors = []
        self._base_error = None
        self._on_completed_fut = None

    def __repr__(self):
        info = [""]
        if self._tasks:
            info.append(f"tasks={len(self._tasks)}")
        if self._errors:
            info.append(f"errors={len(self._errors)}")
        if self._aborting:
            info.append("cancelling")
        elif self._entered:
            info.append("entered")
        info_str = " ".join(info)
        return f"<TaskGroup{info_str}>"

    async def __aenter__(self):
        if self._entered:
            raise RuntimeError(
                f"TaskGroup {self!r} has already been entered"
            )
        if self._loop is None:
            self._loop = get_running_loop()
        self._parent_task = current_task(self._loop)
        if self._parent_task is None:
            raise RuntimeError(
                f"TaskGroup {self!r} cannot determine the parent task"
            )
        self._entered = True
        return self

    async def __aexit__(self, et, exc, tb):
        self._exiting = True

        if (
            exc is not None
            and self._is_base_error(exc)
            and self._base_error is None
        ):
            self._base_error = exc

        propagate_cancellation_error = (
            exc if et is CancelledError else None
        )
        if self._parent_cancel_requested:
            if self._parent_task.uncancel() == 0:
                propagate_cancellation_error = None

        if et is not None:
            if not self._aborting:
                self._abort()

        while self._tasks:
            if self._on_completed_fut is None:
                self._on_completed_fut = self._loop.create_future()

            try:
                await self._on_completed_fut
            except CancelledError as ex:
                if not self._aborting:
                    propagate_cancellation_error = ex
                    self._abort()

            self._on_completed_fut = None

        if self._base_error is not None:
            raise self._base_error

        if propagate_cancellation_error and not self._errors:
            raise propagate_cancellation_error

        if et is not None and et is not CancelledError:
            self._errors.append(exc)

        if self._errors:
            try:
                me = BaseExceptionGroup(
                    "unhandled errors in a TaskGroup", self._errors
                )
                raise me from None
            finally:
                self._errors = None

    def create_task(self, coro, *, name=None, context=None):
        """Create a new task in this group and return it."""
        if not self._entered:
            raise RuntimeError(f"TaskGroup {self!r} has not been entered")
        if self._exiting and not self._tasks:
            raise RuntimeError(f"TaskGroup {self!r} is finished")
        if self._aborting:
            raise RuntimeError(f"TaskGroup {self!r} is shutting down")
        if context is None:
            task = self._loop.create_task(coro)
        else:
            task = self._loop.create_task(coro, context=context)
        _set_task_name(task, name)
        if task.done():
            self._on_task_done(task)
        else:
            self._tasks.add(task)
            task.add_done_callback(self._on_task_done)
        return task

    def _is_base_error(self, exc):
        return isinstance(exc, (SystemExit, KeyboardInterrupt))

    def _abort(self):
        self._aborting = True
        for t in self._tasks:
            if not t.done():
                t.cancel()

    def _on_task_done(self, task):
        self._tasks.discard(task)

        if self._on_completed_fut is not None and not self._tasks:
            if not self._on_completed_fut.done():
                self._on_completed_fut.set_result(True)

        if task.cancelled():
            return

        exc = task.exception()
        if exc is None:
            return

        self._errors.append(exc)
        if self._is_base_error(exc) and self._base_error is None:
            self._base_error = exc

        if self._parent_task.done():
            self._loop.call_exception_handler(
                {
                    "message": (
                        f"Task {task!r} has errored out but its parent "
                        f"task {self._parent_task} is already completed"
                    ),
                    "exception": exc,
                    "task": task,
                }
            )
            return

        if not self._aborting and not self._parent_cancel_requested:
            self._abort()
            self._parent_cancel_requested = True
            self._parent_task.cancel()


# ---------------------------------------------------------------------------
# Loop-bound mixin & Synchronization primitives
# ---------------------------------------------------------------------------


class _LoopBoundMixin:
    _loop = None

    def _get_loop(self):
        loop = _get_running_loop()
        if self._loop is None:
            self._loop = loop
        if loop is not self._loop:
            raise RuntimeError(
                f"{self!r} is bound to a different event loop"
            )
        return loop


class _ContextManagerMixin:
    async def __aenter__(self):
        await self.acquire()
        return None

    async def __aexit__(self, exc_type, exc, tb):
        self.release()


class Lock(_ContextManagerMixin, _LoopBoundMixin):
    """Primitive lock objects."""

    __module__ = "asyncio.locks"

    def __init__(self):
        self._waiters = None
        self._locked = False

    def __repr__(self):
        res = super().__repr__()
        extra = "locked" if self._locked else "unlocked"
        if self._waiters:
            extra = f"{extra}, waiters:{len(self._waiters)}"
        return f"<{res[1:-1]} [{extra}]>"

    def locked(self):
        return self._locked

    async def acquire(self):
        if not self._locked and (
            self._waiters is None
            or all(w.cancelled() for w in self._waiters)
        ):
            self._locked = True
            return True

        if self._waiters is None:
            self._waiters = _deque()
        fut = self._get_loop().create_future()
        self._waiters.append(fut)

        try:
            try:
                await fut
            finally:
                self._waiters.remove(fut)
        except CancelledError:
            if not self._locked:
                self._wake_up_first()
            raise

        self._locked = True
        return True

    def release(self):
        if self._locked:
            self._locked = False
            self._wake_up_first()
        else:
            raise RuntimeError("Lock is not acquired.")

    def _wake_up_first(self):
        if not self._waiters:
            return
        try:
            fut = next(iter(self._waiters))
        except StopIteration:
            return
        if not fut.done():
            fut.set_result(True)


class Event(_LoopBoundMixin):
    """Asynchronous equivalent to threading.Event."""

    __module__ = "asyncio.locks"

    def __init__(self):
        self._waiters = _deque()
        self._value = False

    def __repr__(self):
        res = super().__repr__()
        extra = "set" if self._value else "unset"
        if self._waiters:
            extra = f"{extra}, waiters:{len(self._waiters)}"
        return f"<{res[1:-1]} [{extra}]>"

    def is_set(self):
        return self._value

    def set(self):
        if not self._value:
            self._value = True
            for fut in self._waiters:
                if not fut.done():
                    fut.set_result(True)

    def clear(self):
        self._value = False

    async def wait(self):
        if self._value:
            return True

        fut = self._get_loop().create_future()
        self._waiters.append(fut)
        try:
            await fut
            return True
        finally:
            self._waiters.remove(fut)


class Condition(_ContextManagerMixin, _LoopBoundMixin):
    """Asynchronous equivalent to threading.Condition."""

    __module__ = "asyncio.locks"

    def __init__(self, lock=None):
        if lock is None:
            lock = Lock()
        self._lock = lock
        self.locked = lock.locked
        self.acquire = lock.acquire
        self.release = lock.release
        self._waiters = _deque()

    def __repr__(self):
        res = super().__repr__()
        extra = "locked" if self.locked() else "unlocked"
        if self._waiters:
            extra = f"{extra}, waiters:{len(self._waiters)}"
        return f"<{res[1:-1]} [{extra}]>"

    async def wait(self):
        if not self.locked():
            raise RuntimeError("cannot wait on un-acquired lock")

        self.release()
        try:
            fut = self._get_loop().create_future()
            self._waiters.append(fut)
            try:
                await fut
                return True
            finally:
                self._waiters.remove(fut)
        finally:
            cancelled = False
            while True:
                try:
                    await self.acquire()
                    break
                except CancelledError:
                    cancelled = True
            if cancelled:
                raise CancelledError

    async def wait_for(self, predicate):
        result = predicate()
        while not result:
            await self.wait()
            result = predicate()
        return result

    def notify(self, n=1):
        if not self.locked():
            raise RuntimeError("cannot notify on un-acquired lock")

        idx = 0
        for fut in self._waiters:
            if idx >= n:
                break
            if not fut.done():
                idx += 1
                fut.set_result(False)

    def notify_all(self):
        self.notify(len(self._waiters))


class Semaphore(_ContextManagerMixin, _LoopBoundMixin):
    """A Semaphore implementation."""

    __module__ = "asyncio.locks"

    def __init__(self, value=1):
        if value < 0:
            raise ValueError("Semaphore initial value must be >= 0")
        self._waiters = None
        self._value = value

    def __repr__(self):
        res = super().__repr__()
        extra = (
            "locked" if self.locked() else f"unlocked, value:{self._value}"
        )
        if self._waiters:
            extra = f"{extra}, waiters:{len(self._waiters)}"
        return f"<{res[1:-1]} [{extra}]>"

    def locked(self):
        return self._value == 0 or any(
            not w.cancelled() for w in (self._waiters or ())
        )

    async def acquire(self):
        if not self.locked():
            self._value -= 1
            return True

        if self._waiters is None:
            self._waiters = _deque()
        fut = self._get_loop().create_future()
        self._waiters.append(fut)

        try:
            try:
                await fut
            finally:
                self._waiters.remove(fut)
        except CancelledError:
            if not fut.cancelled():
                self._value += 1
                self._wake_up_next()
            raise

        if self._value > 0:
            self._wake_up_next()
        return True

    def release(self):
        self._value += 1
        self._wake_up_next()

    def _wake_up_next(self):
        if not self._waiters:
            return
        for fut in self._waiters:
            if not fut.done():
                self._value -= 1
                fut.set_result(True)
                return


class BoundedSemaphore(Semaphore):
    """A bounded semaphore that raises ValueError on excess release()."""

    __module__ = "asyncio.locks"

    def __init__(self, value=1):
        self._bound_value = value
        super().__init__(value)

    def release(self):
        if self._value >= self._bound_value:
            raise ValueError("BoundedSemaphore released too many times")
        super().release()


_BARRIER_FILLING = "filling"
_BARRIER_DRAINING = "draining"
_BARRIER_RESETTING = "resetting"
_BARRIER_BROKEN = "broken"


class Barrier(_LoopBoundMixin):
    """Asyncio equivalent to threading.Barrier."""

    __module__ = "asyncio.locks"

    def __init__(self, parties):
        if parties < 1:
            raise ValueError("parties must be > 0")
        self._cond = Condition()
        self._parties = parties
        self._state = _BARRIER_FILLING
        self._count = 0

    def __repr__(self):
        res = super().__repr__()
        extra = f"{self._state}"
        if not self.broken:
            extra += f", waiters:{self.n_waiting}/{self.parties}"
        return f"<{res[1:-1]} [{extra}]>"

    async def __aenter__(self):
        return await self.wait()

    async def __aexit__(self, *args):
        pass

    async def wait(self):
        async with self._cond:
            await self._block()
            try:
                index = self._count
                self._count += 1
                if index + 1 == self._parties:
                    await self._release()
                else:
                    await self._wait()
                return index
            finally:
                self._count -= 1
                self._exit()

    async def _block(self):
        await self._cond.wait_for(
            lambda: self._state not in (_BARRIER_DRAINING, _BARRIER_RESETTING)
        )
        if self._state == _BARRIER_BROKEN:
            raise BrokenBarrierError("Barrier aborted")

    async def _release(self):
        self._state = _BARRIER_DRAINING
        self._cond.notify_all()

    async def _wait(self):
        await self._cond.wait_for(lambda: self._state != _BARRIER_FILLING)
        if self._state in (_BARRIER_BROKEN, _BARRIER_RESETTING):
            raise BrokenBarrierError("Abort or reset of barrier")

    def _exit(self):
        if self._count == 0:
            if self._state in (_BARRIER_RESETTING, _BARRIER_DRAINING):
                self._state = _BARRIER_FILLING
            self._cond.notify_all()

    async def reset(self):
        async with self._cond:
            if self._count > 0:
                if self._state != _BARRIER_RESETTING:
                    self._state = _BARRIER_RESETTING
            else:
                self._state = _BARRIER_FILLING
            self._cond.notify_all()

    async def abort(self):
        async with self._cond:
            self._state = _BARRIER_BROKEN
            self._cond.notify_all()

    @property
    def parties(self):
        return self._parties

    @property
    def n_waiting(self):
        if self._state == _BARRIER_FILLING:
            return self._count
        return 0

    @property
    def broken(self):
        return self._state == _BARRIER_BROKEN


# ---------------------------------------------------------------------------
# Queues: Queue, LifoQueue, PriorityQueue, QueueEmpty, QueueFull
# ---------------------------------------------------------------------------


class QueueEmpty(Exception):
    """Raised when Queue.get_nowait() is called on an empty Queue."""

    __module__ = "asyncio.queues"


class QueueFull(Exception):
    """Raised when Queue.put_nowait() is called on a full Queue."""

    __module__ = "asyncio.queues"


class Queue(_LoopBoundMixin):
    """A FIFO queue for coordinating producer and consumer coroutines."""

    __module__ = "asyncio.queues"

    __class_getitem__ = classmethod(
        lambda cls, item: _GenericAlias(cls, item)
    )

    def __init__(self, maxsize=0):
        self._maxsize = maxsize
        self._getters = _deque()
        self._putters = _deque()
        self._unfinished_tasks = 0
        self._finished = Event()
        self._finished.set()
        self._init(maxsize)

    def _init(self, maxsize):
        self._queue = _deque()

    def _get(self):
        return self._queue.popleft()

    def _put(self, item):
        self._queue.append(item)

    def _wakeup_next(self, waiters):
        while waiters:
            waiter = waiters.popleft()
            if not waiter.done():
                waiter.set_result(None)
                break

    def __repr__(self):
        return f"<{type(self).__name__} at {id(self):#x} {self._format()}>"

    def __str__(self):
        return f"<{type(self).__name__} {self._format()}>"

    def _format(self):
        result = f"maxsize={self._maxsize!r}"
        if getattr(self, "_queue", None):
            result += f" _queue={list(self._queue)!r}"
        if self._getters:
            result += f" _getters[{len(self._getters)}]"
        if self._putters:
            result += f" _putters[{len(self._putters)}]"
        if self._unfinished_tasks:
            result += f" tasks={self._unfinished_tasks}"
        return result

    def qsize(self):
        return len(self._queue)

    @property
    def maxsize(self):
        return self._maxsize

    def empty(self):
        return not self._queue

    def full(self):
        if self._maxsize <= 0:
            return False
        return self.qsize() >= self._maxsize

    async def put(self, item):
        while self.full():
            putter = self._get_loop().create_future()
            self._putters.append(putter)
            try:
                await putter
            except BaseException:
                putter.cancel()
                try:
                    self._putters.remove(putter)
                except ValueError:
                    pass
                if not self.full() and not putter.cancelled():
                    self._wakeup_next(self._putters)
                raise
        return self.put_nowait(item)

    def put_nowait(self, item):
        if self.full():
            raise QueueFull
        self._put(item)
        self._unfinished_tasks += 1
        self._finished.clear()
        self._wakeup_next(self._getters)

    async def get(self):
        while self.empty():
            getter = self._get_loop().create_future()
            self._getters.append(getter)
            try:
                await getter
            except BaseException:
                getter.cancel()
                try:
                    self._getters.remove(getter)
                except ValueError:
                    pass
                if not self.empty() and not getter.cancelled():
                    self._wakeup_next(self._getters)
                raise
        return self.get_nowait()

    def get_nowait(self):
        if self.empty():
            raise QueueEmpty
        item = self._get()
        self._wakeup_next(self._putters)
        return item

    def task_done(self):
        if self._unfinished_tasks <= 0:
            raise ValueError("task_done() called too many times")
        self._unfinished_tasks -= 1
        if self._unfinished_tasks == 0:
            self._finished.set()

    async def join(self):
        if self._unfinished_tasks > 0:
            await self._finished.wait()


class PriorityQueue(Queue):
    """A subclass of Queue; retrieves entries in priority order (lowest first)."""

    __module__ = "asyncio.queues"

    def _init(self, maxsize):
        self._queue = []

    def _put(self, item, heappush=_heapq.heappush):
        heappush(self._queue, item)

    def _get(self, heappop=_heapq.heappop):
        return heappop(self._queue)


class LifoQueue(Queue):
    """A subclass of Queue that retrieves most recently added entries first."""

    __module__ = "asyncio.queues"

    def _init(self, maxsize):
        self._queue = []

    def _put(self, item):
        self._queue.append(item)

    def _get(self):
        return self._queue.pop()


# ---------------------------------------------------------------------------
# Entry point for native `asyncio.run`
# ---------------------------------------------------------------------------


def _cancel_all_tasks(loop):
    to_cancel = all_tasks(loop)
    if not to_cancel:
        return
    for task in to_cancel:
        task.cancel()
    loop.run_until_complete(gather(*to_cancel, return_exceptions=True))


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
        task = loop.create_task(coro)
        return loop.run_until_complete(task)
    finally:
        try:
            _cancel_all_tasks(loop)
        finally:
            _current_tasks.pop(loop, None)
            for t in list(_scheduled_tasks):
                if t._loop is loop:
                    _scheduled_tasks.discard(t)
            _running_loop = None
