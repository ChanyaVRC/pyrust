"""Thread module emulating a subset of Java's threading model.

Single-interpreter-safe Python 3.12 synchronization primitives for PyRust
(issue #3208).
"""

from collections import deque as _deque, namedtuple as _namedtuple
import sys as _sys
from time import monotonic as _time, sleep as _sleep
from types import (
    GetSetDescriptorType as _GetSetDescriptorType,
    MemberDescriptorType as _MemberDescriptorType,
)

__all__ = [
    "get_ident",
    "active_count",
    "Condition",
    "current_thread",
    "enumerate",
    "main_thread",
    "TIMEOUT_MAX",
    "Event",
    "Lock",
    "RLock",
    "Semaphore",
    "BoundedSemaphore",
    "Thread",
    "Barrier",
    "BrokenBarrierError",
    "Timer",
    "ThreadError",
    "setprofile",
    "settrace",
    "local",
    "stack_size",
    "excepthook",
    "ExceptHookArgs",
    "gettrace",
    "getprofile",
    "setprofile_all_threads",
    "settrace_all_threads",
    "get_native_id",
]

TIMEOUT_MAX = _PLATFORM_TIMEOUT_MAX
ThreadError = RuntimeError

_MAIN_THREAD_IDENT = _native_get_ident()


def get_ident():
    """Return the 'thread identifier' of the current thread."""
    return _MAIN_THREAD_IDENT


def get_native_id():
    """Return the native integral Thread ID of the current thread."""
    return _MAIN_THREAD_IDENT


def _validate_lock_timeout(blocking, timeout):
    if isinstance(timeout, bool):
        timeout = float(timeout)
    elif not isinstance(timeout, (int, float)):
        raise TypeError(
            f"must be real number, not {type(timeout).__name__}"
        )
    if not blocking:
        if timeout != -1:
            raise ValueError(
                "can't specify a timeout for a non-blocking call"
            )
    else:
        if timeout < 0 and timeout != -1:
            raise ValueError("timeout value must be positive")
        if timeout > TIMEOUT_MAX:
            raise OverflowError("timeout value is too large")
    return float(timeout)


class _Lock:
    """Single-interpreter-safe non-reentrant lock."""

    def __init__(self):
        self._locked = False

    def acquire(self, blocking=True, timeout=-1):
        blocking = bool(blocking)
        t = _validate_lock_timeout(blocking, timeout)
        if not self._locked:
            self._locked = True
            return True
        if not blocking:
            return False
        if t >= 0.0:
            if t > 0.0:
                _sleep(t)
            return False
        raise RuntimeError(
            "cannot acquire already-locked Lock without timeout in "
            "single-threaded interpreter"
        )

    acquire_lock = acquire

    def release(self):
        if not self._locked:
            raise RuntimeError("release unlocked lock")
        self._locked = False

    release_lock = release

    def locked(self):
        return self._locked

    locked_lock = locked

    def _at_fork_reinit(self):
        self._locked = False

    def __enter__(self):
        self.acquire()
        return True

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.release()

    def __repr__(self):
        status = "locked" if self._locked else "unlocked"
        return f"<{status} threading.Lock object at {id(self):#x}>"


_LockType = _Lock


def Lock():
    """Factory function that returns a new primitive lock object."""
    return _Lock()


_allocate_lock = Lock


class _RLock:
    """Reentrant lock object."""

    def __init__(self):
        self._block = _allocate_lock()
        self._owner = None
        self._count = 0

    def __repr__(self):
        owner = self._owner
        if owner == _MAIN_THREAD_IDENT and _main_thread is not None:
            owner = _main_thread.name
        return "<%s %s.%s object owner=%r count=%d at %s>" % (
            "locked" if self._block.locked() else "unlocked",
            self.__class__.__module__,
            self.__class__.__qualname__,
            owner,
            self._count,
            hex(id(self)),
        )

    def _at_fork_reinit(self):
        self._block._at_fork_reinit()
        self._owner = None
        self._count = 0

    def acquire(self, blocking=True, timeout=-1):
        blocking = bool(blocking)
        _validate_lock_timeout(blocking, timeout)
        me = get_ident()
        if self._owner == me:
            self._count += 1
            return True
        rc = self._block.acquire(blocking, timeout)
        if rc:
            self._owner = me
            self._count = 1
        return bool(rc)

    def __enter__(self):
        return self.acquire()

    def release(self):
        if self._owner != get_ident():
            raise RuntimeError("cannot release un-acquired lock")
        self._count = count = self._count - 1
        if not count:
            self._owner = None
            self._block.release()

    def __exit__(self, t, v, tb):
        self.release()

    def _acquire_restore(self, state):
        self._block.acquire()
        self._count, self._owner = state

    def _release_save(self):
        if self._count == 0:
            raise RuntimeError("cannot release un-acquired lock")
        count = self._count
        self._count = 0
        owner = self._owner
        self._owner = None
        self._block.release()
        return (count, owner)

    def _is_owned(self):
        return self._owner == get_ident()

    def _recursion_count(self):
        if self._owner != get_ident():
            return 0
        return self._count


_PyRLock = _RLock


def RLock():
    """Factory function that returns a new reentrant lock object."""
    return _RLock()


class Condition:
    """Class that implements a condition variable."""

    def __init__(self, lock=None):
        if lock is None:
            lock = RLock()
        self._lock = lock
        self.acquire = lock.acquire
        self.release = lock.release
        if hasattr(lock, "_release_save"):
            self._release_save = lock._release_save
        if hasattr(lock, "_acquire_restore"):
            self._acquire_restore = lock._acquire_restore
        if hasattr(lock, "_is_owned"):
            self._is_owned = lock._is_owned
        self._waiters = _deque()

    def _at_fork_reinit(self):
        self._lock._at_fork_reinit()
        self._waiters.clear()

    def __enter__(self):
        return self._lock.__enter__()

    def __exit__(self, *args):
        return self._lock.__exit__(*args)

    def __repr__(self):
        return "<Condition(%s, %d)>" % (self._lock, len(self._waiters))

    def _release_save(self):
        self._lock.release()

    def _acquire_restore(self, x):
        self._lock.acquire()

    def _is_owned(self):
        if self._lock.acquire(False):
            self._lock.release()
            return False
        return True

    def wait(self, timeout=None):
        if not self._is_owned():
            raise RuntimeError("cannot wait on un-acquired lock")
        waiter = _allocate_lock()
        waiter.acquire()
        self._waiters.append(waiter)
        saved_state = self._release_save()
        gotit = False
        try:
            if timeout is None:
                waiter.acquire()
                gotit = True
            else:
                if timeout > 0:
                    gotit = waiter.acquire(True, timeout)
                else:
                    gotit = waiter.acquire(False)
            return gotit
        finally:
            self._acquire_restore(saved_state)
            if not gotit:
                try:
                    self._waiters.remove(waiter)
                except ValueError:
                    pass

    def wait_for(self, predicate, timeout=None):
        endtime = None
        waittime = timeout
        result = predicate()
        while not result:
            if waittime is not None:
                if endtime is None:
                    endtime = _time() + waittime
                else:
                    waittime = endtime - _time()
                    if waittime <= 0:
                        break
            self.wait(waittime)
            result = predicate()
        return result

    def notify(self, n=1):
        if not self._is_owned():
            raise RuntimeError("cannot notify on un-acquired lock")
        waiters = self._waiters
        while waiters and n > 0:
            waiter = waiters[0]
            try:
                waiter.release()
            except RuntimeError:
                pass
            else:
                n -= 1
            try:
                waiters.remove(waiter)
            except ValueError:
                pass

    def notify_all(self):
        self.notify(len(self._waiters))

    def notifyAll(self):
        self.notify_all()


class Semaphore:
    """This class implements semaphore objects."""

    def __init__(self, value=1):
        if value < 0:
            raise ValueError("semaphore initial value must be >= 0")
        self._cond = Condition(Lock())
        self._value = value

    def __repr__(self):
        cls = self.__class__
        return (
            f"<{cls.__module__}.{cls.__qualname__} at {id(self):#x}:"
            f" value={self._value}>"
        )

    def acquire(self, blocking=True, timeout=None):
        if not blocking and timeout is not None:
            raise ValueError("can't specify timeout for non-blocking acquire")
        rc = False
        endtime = None
        with self._cond:
            while self._value == 0:
                if not blocking:
                    break
                if timeout is not None:
                    if endtime is None:
                        endtime = _time() + timeout
                    else:
                        timeout = endtime - _time()
                        if timeout <= 0:
                            break
                self._cond.wait(timeout)
            else:
                self._value -= 1
                rc = True
        return rc

    __enter__ = acquire

    def release(self, n=1):
        if n < 1:
            raise ValueError("n must be one or more")
        with self._cond:
            self._value += n
            self._cond.notify(n)

    def __exit__(self, t, v, tb):
        self.release()


class BoundedSemaphore(Semaphore):
    """Implements a bounded semaphore."""

    def __init__(self, value=1):
        super().__init__(value)
        self._initial_value = value

    def __repr__(self):
        cls = self.__class__
        return (
            f"<{cls.__module__}.{cls.__qualname__} at {id(self):#x}:"
            f" value={self._value}/{self._initial_value}>"
        )

    def release(self, n=1):
        if n < 1:
            raise ValueError("n must be one or more")
        with self._cond:
            if self._value + n > self._initial_value:
                raise ValueError("Semaphore released too many times")
            self._value += n
            self._cond.notify(n)


class Event:
    """Class implementing event objects."""

    def __init__(self):
        self._cond = Condition(Lock())
        self._flag = False

    def __repr__(self):
        cls = self.__class__
        status = "set" if self._flag else "unset"
        return f"<{cls.__module__}.{cls.__qualname__} at {id(self):#x}: {status}>"

    def _at_fork_reinit(self):
        self._cond._at_fork_reinit()

    def is_set(self):
        return self._flag

    def isSet(self):
        return self.is_set()

    def set(self):
        with self._cond:
            self._flag = True
            self._cond.notify_all()

    def clear(self):
        with self._cond:
            self._flag = False

    def wait(self, timeout=None):
        with self._cond:
            signaled = self._flag
            if not signaled:
                signaled = self._cond.wait(timeout)
            return signaled


class BrokenBarrierError(RuntimeError):
    """Raised when a Barrier is broken or reset while waiting."""


class Barrier:
    """Implements a Barrier."""

    def __init__(self, parties, action=None, timeout=None):
        if parties < 1:
            raise ValueError("parties must be >= 1")
        self._cond = Condition(Lock())
        self._action = action
        self._timeout = timeout
        self._parties = parties
        self._state = 0  # 0 filling, 1 draining, -1 resetting, -2 broken
        self._count = 0

    def __repr__(self):
        cls = self.__class__
        if self.broken:
            return f"<{cls.__module__}.{cls.__qualname__} at {id(self):#x}: broken>"
        return (
            f"<{cls.__module__}.{cls.__qualname__} at {id(self):#x}:"
            f" waiters={self.n_waiting}/{self.parties}>"
        )

    def wait(self, timeout=None):
        if timeout is None:
            timeout = self._timeout
        with self._cond:
            self._enter()
            index = self._count
            self._count += 1
            try:
                if index + 1 == self._parties:
                    self._release()
                else:
                    self._wait(timeout)
                return index
            finally:
                self._count -= 1
                self._exit()

    def _enter(self):
        while self._state in (-1, 1):
            self._cond.wait()
        if self._state < 0:
            raise BrokenBarrierError
        assert self._state == 0

    def _release(self):
        try:
            if self._action:
                self._action()
            self._state = 1
            self._cond.notify_all()
        except BaseException:
            self._break()
            raise

    def _wait(self, timeout):
        if not self._cond.wait_for(lambda: self._state != 0, timeout):
            self._break()
            raise BrokenBarrierError
        if self._state < 0:
            raise BrokenBarrierError
        assert self._state == 1

    def _exit(self):
        if self._count == 0:
            if self._state in (-1, 1):
                self._state = 0
                self._cond.notify_all()

    def reset(self):
        with self._cond:
            if self._count > 0:
                if self._state == 0:
                    self._state = -1
                elif self._state == -2:
                    self._state = -1
            else:
                self._state = 0
            self._cond.notify_all()

    def abort(self):
        with self._cond:
            self._break()

    def _break(self):
        self._state = -2
        self._cond.notify_all()

    @property
    def parties(self):
        return self._parties

    @property
    def n_waiting(self):
        if self._state == 0:
            return self._count
        return 0

    @property
    def broken(self):
        return self._state == -2


# ---------------------------------------------------------------------------
# Thread-local storage
# ---------------------------------------------------------------------------


def _lookup_type_descriptor(cls, name):
    for base in cls.__mro__:
        ns = base.__dict__
        if name in ns:
            return ns[name]
    return None


def _is_data_descriptor(desc):
    if desc is None:
        return False
    t = type(desc)
    if t is _MemberDescriptorType or t is _GetSetDescriptorType:
        return True
    return hasattr(t, "__set__") or hasattr(t, "__delete__")


class local:
    """Thread-local data storage supporting subclassing and independent instances."""

    __slots__ = ("_local__dicts", "_local__init_args")

    def __new__(cls, *args, **kwargs):
        if (args or kwargs) and (cls.__init__ is object.__init__):
            raise TypeError("Initialization arguments are not supported")
        self = object.__new__(cls)
        object.__setattr__(self, "_local__dicts", {get_ident(): {}})
        object.__setattr__(self, "_local__init_args", (args, kwargs))
        return self

    def _local_get_dict(self):
        dicts = object.__getattribute__(self, "_local__dicts")
        tid = get_ident()
        dct = dicts.get(tid)
        if dct is None:
            dct = {}
            dicts[tid] = dct
            args, kwargs = object.__getattribute__(self, "_local__init_args")
            self.__init__(*args, **kwargs)
        return dct

    def __getattribute__(self, name):
        if name in ("_local__dicts", "_local__init_args", "_local_get_dict"):
            return object.__getattribute__(self, name)
        if name == "__dict__":
            return object.__getattribute__(self, "_local_get_dict")()
        cls = object.__getattribute__(self, "__class__")
        desc = _lookup_type_descriptor(cls, name)
        if _is_data_descriptor(desc):
            return object.__getattribute__(self, name)
        dct = object.__getattribute__(self, "_local_get_dict")()
        if name in dct:
            return dct[name]
        return object.__getattribute__(self, name)

    def __setattr__(self, name, value):
        if name == "__dict__":
            raise AttributeError(
                "%r object attribute '__dict__' is read-only"
                % self.__class__.__name__
            )
        cls = object.__getattribute__(self, "__class__")
        desc = _lookup_type_descriptor(cls, name)
        if _is_data_descriptor(desc):
            object.__setattr__(self, name, value)
            return
        dct = object.__getattribute__(self, "_local_get_dict")()
        dct[name] = value

    def __delattr__(self, name):
        if name == "__dict__":
            raise AttributeError(
                "%r object attribute '__dict__' is read-only"
                % self.__class__.__name__
            )
        cls = object.__getattribute__(self, "__class__")
        desc = _lookup_type_descriptor(cls, name)
        if _is_data_descriptor(desc):
            object.__delattr__(self, name)
            return
        dct = object.__getattribute__(self, "_local_get_dict")()
        if name in dct:
            del dct[name]
            return
        raise AttributeError(
            f"{self.__class__.__name__!r} object has no attribute {name!r}"
        )

    def __dir__(self):
        dct = object.__getattribute__(self, "_local_get_dict")()
        names = set(object.__dir__(self))
        names.discard("_local__dicts")
        names.discard("_local__init_args")
        names.discard("_local_get_dict")
        names.update(dct.keys())
        return sorted(names)


# ---------------------------------------------------------------------------
# Thread, Timer, and Thread inspection
# ---------------------------------------------------------------------------

_counter = 0


def _newname(name_template="Thread-%d"):
    global _counter
    _counter += 1
    return name_template % _counter


_trace_hook = None
_profile_hook = None
_stack_size = 0


def settrace(func):
    """Set a trace function for all threads started from the threading module."""
    global _trace_hook
    _trace_hook = func


def gettrace():
    """Get the trace function as set by threading.settrace()."""
    return _trace_hook


def setprofile(func):
    """Set a profile function for all threads started from the threading module."""
    global _profile_hook
    _profile_hook = func


def getprofile():
    """Get the profile function as set by threading.setprofile()."""
    return _profile_hook


def settrace_all_threads(func):
    settrace(func)
    if hasattr(_sys, "settrace"):
        _sys.settrace(func)


def setprofile_all_threads(func):
    setprofile(func)
    if hasattr(_sys, "setprofile"):
        _sys.setprofile(func)


def stack_size(size=None):
    """Return or set the thread stack size used when creating new threads."""
    global _stack_size
    old = _stack_size
    if size is not None:
        if not isinstance(size, int):
            raise TypeError("an integer is required")
        if size < 0:
            raise ValueError("size must be 0 or a positive value")
        _stack_size = size
    return old


_ExceptHookArgsBase = _namedtuple(
    "ExceptHookArgs",
    ("exc_type", "exc_value", "exc_traceback", "thread"),
)


class ExceptHookArgs(_ExceptHookArgsBase):
    def __new__(cls, sequence):
        items = tuple(sequence)
        if len(items) != 4:
            raise TypeError(
                f"threading.ExceptHookArgs() takes a 4-sequence ({len(items)}-sequence given)"
            )
        return _ExceptHookArgsBase.__new__(cls, *items)


def excepthook(args, /):
    """Handle uncaught Thread.run() exception."""
    if args.exc_type is SystemExit:
        return
    name = "<unknown>" if args.thread is None else args.thread.name
    _sys.stderr.write(f"Exception in thread {name}:\n")
    import traceback

    traceback.print_exception(
        args.exc_type, args.exc_value, args.exc_traceback, file=_sys.stderr
    )


__excepthook__ = excepthook


class Thread:
    """A class that represents a thread of control."""

    _initialized = False

    def __init__(
        self,
        group=None,
        target=None,
        name=None,
        args=(),
        kwargs=None,
        *,
        daemon=None,
    ):
        assert group is None, "group argument must be None for now"
        if kwargs is None:
            kwargs = {}
        if name:
            name = str(name)
        else:
            name = _newname("Thread-%d")
            if target is not None:
                try:
                    target_name = target.__name__
                    name += f" ({target_name})"
                except AttributeError:
                    pass
        self._target = target
        self._name = name
        self._args = args
        self._kwargs = kwargs
        if daemon is not None:
            self._daemonic = bool(daemon)
        else:
            self._daemonic = _main_thread.daemon if _main_thread is not None else False
        self._ident = None
        self._native_id = None
        self._started = Event()
        self._is_stopped = False
        self._initialized = True

    def __repr__(self):
        assert self._initialized, "Thread.__init__() was not called"
        status = "initial"
        if self._started.is_set():
            status = "started"
        if self._is_stopped:
            status = "stopped"
        if self._daemonic:
            status += " daemon"
        if self._ident is not None:
            status += " %s" % self._ident
        return "<%s(%s, %s)>" % (self.__class__.__name__, self._name, status)

    def start(self):
        if not self._initialized:
            raise RuntimeError("thread.__init__() not called")
        if self._started.is_set():
            raise RuntimeError("threads can only be started once")
        raise RuntimeError(
            "can't start new thread in single-threaded interpreter"
        )

    def run(self):
        """Method representing the thread's activity."""
        try:
            if self._target is not None:
                self._target(*self._args, **self._kwargs)
        finally:
            del self._target, self._args, self._kwargs

    def join(self, timeout=None):
        if not self._initialized:
            raise RuntimeError("Thread.__init__() not called")
        if not self._started.is_set():
            raise RuntimeError("cannot join thread before it is started")
        if self is current_thread():
            raise RuntimeError("cannot join current thread")
        return None

    @property
    def name(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._name

    @name.setter
    def name(self, value):
        assert self._initialized, "Thread.__init__() not called"
        self._name = str(value)

    @property
    def ident(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._ident

    @property
    def native_id(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._native_id

    def is_alive(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._started.is_set() and not self._is_stopped

    @property
    def daemon(self):
        assert self._initialized, "Thread.__init__() not called"
        return self._daemonic

    @daemon.setter
    def daemon(self, daemonic):
        if not self._initialized:
            raise RuntimeError("Thread.__init__() not called")
        if self._started.is_set():
            raise RuntimeError("cannot set daemon status of active thread")
        self._daemonic = bool(daemonic)

    def isDaemon(self):
        return self.daemon

    def setDaemon(self, daemonic):
        self.daemon = daemonic

    def getName(self):
        return self.name

    def setName(self, name):
        self.name = name


class Timer(Thread):
    """Call a function after a specified number of seconds."""

    def __init__(self, interval, function, args=None, kwargs=None):
        Thread.__init__(self)
        self.interval = interval
        self.function = function
        self.args = args if args is not None else []
        self.kwargs = kwargs if kwargs is not None else {}
        self.finished = Event()

    def cancel(self):
        """Stop the timer if it hasn't finished yet."""
        self.finished.set()

    def run(self):
        self.finished.wait(self.interval)
        if not self.finished.is_set():
            self.function(*self.args, **self.kwargs)
        self.finished.set()


class _MainThread(Thread):
    def __init__(self):
        Thread.__init__(self, name="MainThread", daemon=False)
        self._started.set()
        self._ident = _MAIN_THREAD_IDENT
        self._native_id = _MAIN_THREAD_IDENT


_main_thread = None
_main_thread = _MainThread()


def main_thread():
    """Return the main thread object."""
    return _main_thread


def current_thread():
    """Return the current Thread object, corresponding to the caller's thread of control."""
    return _main_thread


currentThread = current_thread


def active_count():
    """Return the number of Thread objects currently alive."""
    return 1


activeCount = active_count


def enumerate():
    """Return a list of all Thread objects currently alive."""
    return [_main_thread]
