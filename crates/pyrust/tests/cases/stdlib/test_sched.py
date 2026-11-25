# CPython 3.12 parity tests for the native `sched` module (issue #3248).

import sched

print("all:", sched.__all__)

# Deterministic simulated clock
now = [0]
delays = []
log = []

def fake_time():
    return now[0]

def fake_delay(dt):
    delays.append(dt)
    now[0] += dt

s = sched.scheduler(fake_time, fake_delay)
print("initially_empty:", s.empty(), s.queue)

def record(tag, *args, **kwargs):
    log.append((now[0], tag, args, sorted(kwargs.items())))

e1 = s.enter(10, 1, record, ("ten",), {"x": 1})
e2 = s.enter(5, 2, record, ("five-low",))
e3 = s.enter(5, 1, record, ("five-high",))
e4 = s.enterabs(5, 1, record, ("five-high-2",))
e5 = s.enter(7, 1, record, ("cancelled",))

print("event_is_tuple:", isinstance(e1, tuple), len(e1))
print("event_fields:", e1._fields)
print("event_attrs:", e1.time, e1.priority, e1.sequence, e1.argument, e1.kwargs)
print("event_order:", e3 < e4, e3 < e2, e2 < e1)
print("empty_false:", s.empty())
print("queue_order:", [(ev.time, ev.priority, ev.argument) for ev in s.queue])

# Cancel existing event and verify ValueError on second cancel
s.cancel(e5)
print("after_cancel:", [(ev.time, ev.priority, ev.argument) for ev in s.queue])
try:
    s.cancel(e5)
    print("UNEXPECTED_CANCEL_OK")
except ValueError:
    print("double_cancel_valueerror: ok")

# Non-blocking run before deadline returns remaining delay
rem = s.run(blocking=False)
print("nonblocking_rem:", rem)

# Blocking run executes everything in exact (time, priority, sequence) order
res = s.run()
print("run_result:", res, "empty_after:", s.empty())
print("log:", log)
print("delays:", delays)

# Reentrant scheduling inside an action & delayfunc
relog = []
rnow = [0]
rs = sched.scheduler(lambda: rnow[0], lambda dt: rnow.__setitem__(0, rnow[0] + dt))

def first_action():
    relog.append(("first", rnow[0]))
    rs.enter(3, 1, lambda: relog.append(("chained", rnow[0])))

rs.enter(2, 1, first_action)
rs.run()
print("reentrant_log:", relog)

# Event _make / _asdict / _replace
ev = sched.Event._make([1, 2, 3, record, ("a",), {"k": "v"}])
print("event_asdict_keys:", list(ev._asdict().keys()))
ev2 = ev._replace(priority=99, argument=("b",))
print("event_replace:", ev2.time, ev2.priority, ev2.sequence, ev2.argument, ev2.kwargs)
