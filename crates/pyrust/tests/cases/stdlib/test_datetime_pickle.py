# Parity fixture for issue #3332: pickle / __reduce__ support for
# datetime.date / datetime / time / timedelta / timezone / tzinfo, with the
# exact CPython 3.12.3 compact wire layout.

import copy
import datetime as D
import pickle

PROTOS = range(pickle.HIGHEST_PROTOCOL + 1)


def err(fn):
    try:
        return repr(fn())
    except Exception as exc:
        return "%s: %s" % (type(exc).__name__, exc)


class MyDate(D.date):
    pass


class MyDateTime(D.datetime):
    pass


class MyTime(D.time):
    pass


class MyDelta(D.timedelta):
    pass


class FixedTZ(D.tzinfo):
    def utcoffset(self, dt):
        return D.timedelta(0)

    def dst(self, dt):
        return None

    def tzname(self, dt):
        return "FIXED"


class ArgTZ(D.tzinfo):
    def __init__(self, hours=0):
        self.hours = hours

    def __getinitargs__(self):
        return (self.hours,)

    def utcoffset(self, dt):
        return D.timedelta(hours=self.hours)

    def dst(self, dt):
        return None

    def tzname(self, dt):
        return "ARG%d" % self.hours


JST = D.timezone(D.timedelta(hours=9), "JST")
UTC = D.timezone.utc

objs = [
    D.date(2024, 1, 15),
    D.date(1, 1, 1),
    D.date(9999, 12, 31),
    D.date(255, 2, 28),
    D.date(256, 12, 31),
    D.datetime(2024, 1, 15, 12, 30, 45, 123456),
    D.datetime(1, 1, 1),
    D.datetime(9999, 12, 31, 23, 59, 59, 999999),
    D.datetime(2024, 1, 15, 12, 30, tzinfo=UTC),
    D.datetime(2024, 1, 15, 12, 30, tzinfo=JST),
    D.datetime(2024, 1, 15, 12, 30, tzinfo=D.timezone(D.timedelta(hours=-5))),
    D.datetime(2024, 11, 3, 1, 30, fold=1),
    D.datetime(2024, 11, 3, 1, 30, tzinfo=UTC, fold=1),
    D.time(0, 0),
    D.time(12, 30, 45, 123456),
    D.time(23, 59, 59, 999999),
    D.time(12, 30, tzinfo=D.timezone(D.timedelta(hours=9))),
    D.time(1, 30, fold=1),
    D.time(1, 30, tzinfo=UTC, fold=1),
    D.timedelta(0),
    D.timedelta(days=-2, seconds=3600, microseconds=42),
    D.timedelta.min,
    D.timedelta.max,
    D.timezone(D.timedelta(hours=9), "JST"),
    D.timezone(D.timedelta(hours=-5)),
    D.timezone(D.timedelta(minutes=1)),
    D.timezone.utc,
    D.timezone.min,
    D.timezone.max,
    D.timezone(D.timedelta(0), "ZERO"),
]

# 1. Round trip across every protocol, preserving type and equality.
for obj in objs:
    for proto in PROTOS:
        data = pickle.dumps(obj, protocol=proto)
        rt = pickle.loads(data)
        assert type(rt) is type(obj), (obj, proto, type(rt))
        assert rt == obj, (obj, proto, rt)
        if isinstance(obj, (D.datetime, D.time)):
            assert rt.tzinfo == obj.tzinfo
            # fold is only pickled from protocol 4 on
            print("fold", repr(obj), proto, rt.fold)
print("round trips ok")

# 2. __reduce__ / __reduce_ex__ return values.
for obj in objs:
    print("reduce", repr(obj), repr(obj.__reduce__()))
    for proto in PROTOS:
        print("reduce_ex", repr(obj), proto, repr(obj.__reduce_ex__(proto)))

# 3. Exact wire payloads for every protocol.
for obj in objs:
    for proto in PROTOS:
        print("dumps", repr(obj), proto, pickle.dumps(obj, protocol=proto).hex())

# 4. Default protocol payloads and several objects in one pickle (memo reuse).
for obj in objs:
    print("default", repr(obj), pickle.dumps(obj).hex())
bundle = (D.date(2024, 1, 15), D.datetime(2024, 1, 15, tzinfo=UTC), D.time(1, 2, tzinfo=UTC), D.timedelta(3), UTC, JST)
for proto in PROTOS:
    data = pickle.dumps(bundle, protocol=proto)
    print("bundle", proto, data.hex())
    assert pickle.loads(data) == bundle
shared = [UTC, UTC, D.datetime(2024, 1, 15, tzinfo=UTC)]
for proto in PROTOS:
    data = pickle.dumps(shared, protocol=proto)
    print("shared", proto, data.hex())
    rt = pickle.loads(data)
    print("shared identity", proto, rt[0] is rt[1], rt[0] is rt[2].tzinfo, rt[0] is UTC)

# 5. Unpickling payloads produced by CPython 3.12.3.
print(pickle.loads(bytes.fromhex("8004952a000000000000008c086461746574696d65948c086461746574696d65949394430a07e8010f0c1e2d01e24094859452942e")))
print(repr(pickle.loads(pickle.dumps(D.datetime(2024, 11, 3, 1, 30, fold=1), 4)).fold))
print(repr(pickle.loads(pickle.dumps(D.datetime(2024, 11, 3, 1, 30, fold=1), 3)).fold))
print(repr(pickle.loads(pickle.dumps(D.time(1, 30, fold=1), 4)).fold))
print(repr(pickle.loads(pickle.dumps(D.time(1, 30, fold=1), 2)).fold))

# 6. Singletons survive a round trip.
for proto in PROTOS:
    print("utc singleton", proto, pickle.loads(pickle.dumps(UTC, proto)) is D.timezone.utc)
    print("zero offset", proto, pickle.loads(pickle.dumps(D.timezone(D.timedelta(0)), proto)) is D.timezone.utc)

# 7. Subclasses.
sub_date = MyDate(2024, 1, 15)
sub_date.extra = 1
sub_dt = MyDateTime(2024, 1, 15, 1, 2, 3, 4, tzinfo=UTC, fold=1)
sub_dt.extra = 2
sub_time = MyTime(1, 2, 3, 4)
sub_time.extra = 3
sub_delta = MyDelta(1, 2, 3)
sub_delta.extra = 4
for obj in (sub_date, sub_dt, sub_time, sub_delta):
    print("subclass reduce", type(obj).__name__, repr(obj.__reduce__()))
    print("subclass getstate", type(obj).__name__, obj.__getstate__())
    for proto in PROTOS:
        data = pickle.dumps(obj, protocol=proto)
        print("subclass dumps", type(obj).__name__, proto, data.hex())
        rt = pickle.loads(data)
        assert type(rt) is type(obj)
        assert rt == obj
        print("subclass extra", type(obj).__name__, proto, hasattr(rt, "extra"))
print("plain getstate", D.date(2024, 1, 1).__getstate__(), D.datetime(2024, 1, 1).__getstate__())
print("plain getstate", D.time(1).__getstate__(), D.timedelta(1).__getstate__())
print("plain getstate", UTC.__getstate__(), D.tzinfo().__getstate__())

# 8. tzinfo subclasses and __getinitargs__.
fixed = FixedTZ()
arg = ArgTZ(5)
print("tzinfo reduce", repr(D.tzinfo().__reduce__()))
print("tzinfo reduce", repr(fixed.__reduce__()))
print("tzinfo reduce", repr(arg.__reduce__()))
print("timezone initargs", UTC.__getinitargs__(), JST.__getinitargs__())
arg.note = "hi"
print("tzinfo reduce state", repr(arg.__reduce__()))
for proto in PROTOS:
    print("tzinfo dumps", proto, pickle.dumps(fixed, proto).hex())
    print("tzinfo dumps", proto, pickle.dumps(arg, proto).hex())
    rt = pickle.loads(pickle.dumps(arg, proto))
    print("tzinfo arg", proto, type(rt).__name__, rt.hours, rt.note)
    dt_obj = D.datetime(2024, 1, 15, 12, 30, tzinfo=arg)
    data = pickle.dumps(dt_obj, proto)
    print("dt with tzinfo", proto, data.hex())
    rt = pickle.loads(data)
    print("dt with tzinfo rt", proto, rt, type(rt.tzinfo).__name__, rt.tzinfo.hours, rt.tzinfo.note)
    rt = pickle.loads(pickle.dumps(D.time(1, 2, tzinfo=fixed), proto))
    print("time with tzinfo rt", proto, rt, type(rt.tzinfo).__name__)

# 9. Constructors accepting the pickled state.
print(D.date(b"\x07\xe8\x01\x0f"))
print(D.date("\x07\xe8\x01\x0f"))
print(D.datetime(b"\x07\xe8\x01\x0f\x0c\x1e\x2d\x01\xe2\x40"))
print(D.datetime(b"\x07\xe8\x01\x0f\x0c\x1e\x2d\x01\xe2\x40", UTC))
print(D.datetime(b"\x07\xe8\x01\x0f\x0c\x1e\x2d\x01\xe2\x40", None))
print(D.datetime("\x07\xe8\x01\x0f\x0c\x1e\x2d\x01\xe2\x40"))
print(D.datetime(b"\x07\xe8\x8b\x0f\x0c\x1e\x2d\x01\xe2\x40").fold, D.datetime(b"\x07\xe8\x8b\x0f\x0c\x1e\x2d\x01\xe2\x40").month)
print(D.time(b"\x0c\x1e\x2d\x01\xe2\x40"))
print(D.time(b"\x0c\x1e\x2d\x01\xe2\x40", UTC))
print(D.time(b"\x0c\x1e\x2d\x01\xe2\x40", None))
print(D.time("\x0c\x1e\x2d\x01\xe2\x40"))
print(D.time(b"\x8c\x1e\x2d\x01\xe2\x40").fold, D.time(b"\x8c\x1e\x2d\x01\xe2\x40").hour)
print(type(MyDate(b"\x07\xe8\x01\x0f")).__name__, type(MyDateTime(b"\x07\xe8\x01\x0f\x0c\x1e\x2d\x01\xe2\x40")).__name__, type(MyTime(b"\x0c\x1e\x2d\x01\xe2\x40")).__name__)
# fields are taken verbatim, exactly like CPython
print(D.datetime(b"\x07\xe8\x01\x0f\x0c\x1e\x3d\x01\xe2\x40"))
print(D.date(b"\x07\xe8\x02\x1e"))
print(D.date(b"\x00\x00\x01\x01"))

# 10. Invalid state payloads fall back to the regular argument parsing.
print(err(lambda: D.date(b"\x07\xe8\x0d\x0f")))
print(err(lambda: D.date(b"\x07\xe8\x00\x0f")))
print(err(lambda: D.date(b"\x07\xe8\x01")))
print(err(lambda: D.date(bytearray(b"\x07\xe8\x01\x0f"))))
print(err(lambda: D.date("2024")))
print(err(lambda: D.date("Ā\x00\x01\x01")))
print(err(lambda: D.datetime(b"\x07\xe8\x01\x0f\x0c\x1e\x2d\x01\xe2")))
print(err(lambda: D.datetime(b"\x07\xe8\x0d\x0f\x0c\x1e\x2d\x01\xe2\x40")))
print(err(lambda: D.datetime(b"\x07\xe8\x01\x0f\x0c\x1e\x2d\x01\xe2\x40", 1)))
print(err(lambda: D.datetime(b"\x07\xe8\x01\x0f\x0c\x1e\x2d\x01\xe2\x40", UTC, 1)))
print(err(lambda: D.datetime("Ā\x00\x01\x01\x00\x00\x00\x00\x00\x00")))
print(err(lambda: D.time(b"\x0c\x1e\x2d\x01\xe2")))
print(err(lambda: D.time(b"\x18\x1e\x2d\x01\xe2\x40")))
print(err(lambda: D.time(b"\x0c\x1e\x2d\x01\xe2\x40", 1)))
print(err(lambda: D.time(b"\x0c\x1e\x2d\x01\xe2\x40", 0)))
print(err(lambda: D.time("Ā\x00\x01\x01\x00\x00")))
print(err(lambda: D.time("12:30")))
print(err(lambda: D.timezone(D.timedelta(0))))

# 11. Regular construction is unchanged.
print(D.date(2024, 1, 2), D.date(year=2024, month=1, day=2), D.date(2024, month=1, day=2))
print(D.datetime(2024, 1, 2), D.datetime(2024, 1, 2, 3, 4, 5, 6, UTC), D.datetime(year=2024, month=1, day=2, fold=1).fold)
print(D.time(), D.time(1), D.time(1, 2), D.time(1, 2, 3, 4, UTC), D.time(hour=1, minute=2, fold=1).fold, D.time(minute=5))
print(err(lambda: D.date(2024, 13, 1)))
print(err(lambda: D.datetime(2024, 1, 2, 24)))
print(err(lambda: D.time(24)))
print(err(lambda: D.date(1.5, 1, 1)))

# 12. copy / deepcopy are unaffected.
for obj in (D.date(2024, 1, 15), D.datetime(2024, 1, 15, tzinfo=UTC), D.time(1, 2), D.timedelta(1), JST):
    assert copy.copy(obj) == obj and copy.deepcopy(obj) == obj
print("copy ok", copy.copy(UTC) is UTC, copy.deepcopy(UTC) is UTC)

# 13. Pickler/Unpickler objects and fast paths.
import io

buf = io.BytesIO()
pickle.Pickler(buf, 4).dump([D.date(2024, 1, 15), D.date(2024, 1, 15)])
print(buf.getvalue().hex())
print(pickle.Unpickler(io.BytesIO(buf.getvalue())).load())
