# Parity fixture for `threading` synchronization primitives (issue #3208).

import threading
from threading import (
    TIMEOUT_MAX,
    Barrier,
    BoundedSemaphore,
    BrokenBarrierError,
    Condition,
    Event,
    Lock,
    RLock,
    Semaphore,
    Thread,
    ThreadError,
    Timer,
    active_count,
    current_thread,
    enumerate as thread_enumerate,
    get_ident,
    get_native_id,
    local,
    main_thread,
)


def catch_error(fn):
    try:
        fn()
    except Exception as exc:
        return (type(exc).__name__, str(exc))
    return ("OK", None)


# ── 1. Constants & exception hierarchy ────────────────────────────────────────

print("TIMEOUT_MAX valid:", isinstance(TIMEOUT_MAX, float), TIMEOUT_MAX > 0)
print("ThreadError is RuntimeError:", ThreadError is RuntimeError)
print(
    "BrokenBarrierError subclass:",
    issubclass(BrokenBarrierError, RuntimeError),
)

# ── 2. Lock ───────────────────────────────────────────────────────────────────

lk = Lock()
print("lock init:", lk.locked())
print("lock acquire:", lk.acquire(), lk.locked())
print("lock nonblock while locked:", lk.acquire(False))
print("lock timeout=0 while locked:", lk.acquire(True, 0))
print("lock timeout=0.005 while locked:", lk.acquire(True, 0.005))
print(
    "lock nonblock+timeout:",
    catch_error(lambda: lk.acquire(False, 1)),
)
print(
    "lock neg timeout:",
    catch_error(lambda: lk.acquire(True, -2)),
)
print(
    "lock overflow timeout:",
    catch_error(lambda: lk.acquire(True, TIMEOUT_MAX + 1))[0],
)
lk.release()
print("lock released:", lk.locked())
print("lock double release:", catch_error(lambda: lk.release()))

with lk as acquired:
    print("lock ctx inside:", acquired, lk.locked())
print("lock ctx outside:", lk.locked())

# Exception inside context manager still releases lock
try:
    with lk:
        assert lk.locked()
        raise KeyError("boom")
except KeyError:
    pass
print("lock released after exc:", lk.locked())

# ── 3. RLock ──────────────────────────────────────────────────────────────────

rlk = RLock()
print("rlock first:", rlk.acquire())
print("rlock nested:", rlk.acquire())
print("rlock nonblock nested:", rlk.acquire(False))
print(
    "rlock nonblock+timeout:",
    catch_error(lambda: rlk.acquire(False, 1)),
)
print(
    "rlock neg timeout:",
    catch_error(lambda: rlk.acquire(True, -2)),
)
rlk.release()
rlk.release()
rlk.release()
print("rlock over-release:", catch_error(lambda: rlk.release()))

with rlk as a1:
    with rlk as a2:
        print("rlock nested ctx:", a1, a2)
print("rlock over-release after ctx:", catch_error(lambda: rlk.release()))

# ── 4. Condition ──────────────────────────────────────────────────────────────

cond_default = Condition()
print("cond wait unlocked:", catch_error(lambda: cond_default.wait(0)))
print("cond notify unlocked:", catch_error(lambda: cond_default.notify()))
print("cond notify_all unlocked:", catch_error(lambda: cond_default.notify_all()))

with cond_default:
    cond_default.notify()
    cond_default.notify(2)
    cond_default.notify_all()
    print("cond wait timeout=0:", cond_default.wait(0))
    print("cond wait timeout=0.005:", cond_default.wait(0.005))
    print("cond wait_for true:", cond_default.wait_for(lambda: "ready", timeout=0.01))
    calls = []

    def pred():
        calls.append(1)
        return len(calls) >= 2

    print("cond wait_for stateful:", cond_default.wait_for(pred, timeout=0.05), len(calls))
    print("cond wait_for false:", cond_default.wait_for(lambda: False, timeout=0.005))

# Condition wrapping an explicit Lock
backing_lock = Lock()
cond_lk = Condition(backing_lock)
with cond_lk:
    assert backing_lock.locked()
    assert cond_lk.wait(0) is False
    assert backing_lock.locked()
assert not backing_lock.locked()
print("cond explicit Lock: ok")

# ── 5. Event ──────────────────────────────────────────────────────────────────

ev = Event()
print("event init:", ev.is_set(), ev.wait(0), ev.wait(0.005))
ev.set()
print("event set:", ev.is_set(), ev.wait(), ev.wait(0))
ev.clear()
print("event cleared:", ev.is_set(), ev.wait(0))

# ── 6. Semaphore & BoundedSemaphore ───────────────────────────────────────────

print("sem neg init:", catch_error(lambda: Semaphore(-1)))
sem = Semaphore(2)
print("sem acq 1:", sem.acquire())
print("sem acq 2:", sem.acquire(blocking=False))
print("sem acq 3 nonblock:", sem.acquire(blocking=False))
print("sem acq 3 timeout=0:", sem.acquire(timeout=0))
print(
    "sem nonblock+timeout:",
    catch_error(lambda: sem.acquire(False, 0.1)),
)
print("sem release bad n:", catch_error(lambda: sem.release(0)))
sem.release(2)
with sem as ok:
    print("sem ctx:", ok)
# Unbounded semaphore may be released past initial value
sem.release(3)
print("sem unbounded extra release: ok")

bsem = BoundedSemaphore(2)
print("bsem over-release init:", catch_error(lambda: bsem.release()))
assert bsem.acquire()
assert bsem.acquire()
assert bsem.acquire(False) is False
print("bsem over-release n=3:", catch_error(lambda: bsem.release(3)))
bsem.release(2)
print("bsem over-release after restore:", catch_error(lambda: bsem.release()))
with bsem:
    assert bsem.acquire(False) is True
    bsem.release()
print("bsem ctx: ok")

# ── 7. Barrier ────────────────────────────────────────────────────────────────

action_log = []
b1 = Barrier(1, action=lambda: action_log.append("fired"))
print("barrier1 init:", b1.parties, b1.n_waiting, b1.broken)
print("barrier1 wait:", b1.wait(), action_log, b1.n_waiting, b1.broken)
# Reusable
print("barrier1 wait again:", b1.wait(), len(action_log))

# Action raising breaks barrier
def bad_action():
    raise ValueError("action failed")


b_fail = Barrier(1, action=bad_action)
print("barrier action fail:", catch_error(lambda: b_fail.wait()), b_fail.broken)
print("barrier after fail:", catch_error(lambda: b_fail.wait()))
b_fail.reset()
print("barrier after reset:", b_fail.broken)

# Multi-party barrier times out -> breaks -> raises BrokenBarrierError
b2 = Barrier(2, timeout=0.005)
print("barrier2 timeout:", catch_error(lambda: b2.wait()), b2.broken)
b2.reset()
print("barrier2 reset:", b2.broken)
b2.abort()
print("barrier2 abort:", b2.broken, catch_error(lambda: b2.wait()))

# ── 8. threading.local ────────────────────────────────────────────────────────

l1 = local()
l2 = local()
l1.x = 10
l1.y = 20
l2.x = 99
print("local independent:", l1.x, l1.y, l2.x, hasattr(l2, "y"))
print("local dict:", sorted(l1.__dict__.items()))
l1.__dict__["z"] = 30
print("local dict sync:", l1.z)
del l1.y
err_del_y = catch_error(lambda: delattr(l1, "y"))
print("local del:", hasattr(l1, "y"), err_del_y[0], "has no attribute 'y'" in err_del_y[1])
print("local bad init args:", catch_error(lambda: local(1)))
err_set_dict = catch_error(lambda: setattr(l1, "__dict__", {}))
err_del_dict = catch_error(lambda: delattr(l1, "__dict__"))
print("local readonly __dict__:", err_set_dict[0], "'__dict__' is read-only" in err_set_dict[1])
print("local del __dict__:", err_del_dict[0], "'__dict__' is read-only" in err_del_dict[1])


class CustomLocal(local):
    default_val = 5

    def __init__(self, scale=1):
        self.scale = scale

    def compute(self):
        return self.default_val * self.scale


cl = CustomLocal(scale=3)
print("local subclass:", cl.default_val, cl.scale, cl.compute())
cl.default_val = 7
print("local subclass override:", cl.compute())
del cl.default_val
print("local subclass restored:", cl.compute())


class SlottedLocal(local):
    __slots__ = ("shared_slot",)


sl = SlottedLocal()
sl.shared_slot = "slot_val"
sl.extra = "dict_val"
print("local slotted:", sl.shared_slot, sl.extra, sorted(sl.__dict__.items()))

# ── 9. Thread inspection, Thread, Timer, hooks ────────────────────────────────

tid = get_ident()
nid = get_native_id()
mt = main_thread()
ct = current_thread()
print("ids positive ints:", isinstance(tid, int) and tid > 0, isinstance(nid, int) and nid > 0)
print("main is current:", mt is ct)
print("main attrs:", mt.name, mt.daemon, mt.is_alive(), mt.ident == tid, mt.native_id == nid)
print("active_count:", active_count())
print("enumerate:", len(thread_enumerate()), thread_enumerate()[0] is mt)
print("main join self:", catch_error(lambda: mt.join()))
print("main set daemon:", catch_error(lambda: setattr(mt, "daemon", True)))

ran = []
t = Thread(target=lambda a, b=0: ran.append(a + b), args=(2,), kwargs={"b": 3}, name="Worker-1", daemon=True)
print("thread init:", t.name, t.daemon, t.ident, t.is_alive())
t.name = "Worker-Renamed"
t.daemon = False
print("thread mutated:", t.name, t.daemon)
print("thread join before start:", catch_error(lambda: t.join()))
t.run()
print("thread run:", ran)

timer_log = []
tmr = Timer(0.001, lambda x: timer_log.append(x), args=("tick",))
tmr.run()
print("timer run:", timer_log, tmr.finished.is_set())

tmr_cancel = Timer(1.0, lambda: timer_log.append("should-not-run"))
tmr_cancel.cancel()
tmr_cancel.run()
print("timer canceled:", timer_log)

# stack_size & trace/profile getters/setters
print("trace hook:", threading.gettrace())
hook = lambda frame, event, arg: None
threading.settrace(hook)
print("trace hook set:", threading.gettrace() is hook)
threading.settrace(None)
print("profile hook:", threading.getprofile())
threading.setprofile(hook)
print("profile hook set:", threading.getprofile() is hook)
threading.setprofile(None)
print("stack_size default:", isinstance(threading.stack_size(), int))

eha = threading.ExceptHookArgs((ValueError, ValueError("x"), None, mt))
print("ExceptHookArgs:", eha.exc_type is ValueError, str(eha.exc_value), eha.thread is mt)

print("threading tests ok")
