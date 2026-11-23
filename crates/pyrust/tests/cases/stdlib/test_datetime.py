import copy
import datetime as dt_mod
import sys
import warnings
from datetime import (
    MAXYEAR,
    MINYEAR,
    UTC,
    date,
    datetime,
    time,
    timedelta,
    timezone,
    tzinfo,
)

warnings.simplefilter("ignore", DeprecationWarning)

# 1. Module metadata and exports
print("in sys.modules:", "datetime" in sys.modules)
print("__all__:", dt_mod.__all__)
print("MINYEAR/MAXYEAR:", MINYEAR, MAXYEAR)
print("UTC is timezone.utc:", UTC is timezone.utc)
print("issubclass datetime date:", issubclass(datetime, date))
print("issubclass timezone tzinfo:", issubclass(timezone, tzinfo))
for cls in (date, time, datetime, timedelta, tzinfo, timezone):
    print("module:", cls.__name__, cls.__module__)

# 2. timedelta construction, normalization, rounding, arithmetic, comparisons, repr/str
print("td limits:", repr(timedelta.min), repr(timedelta.max), repr(timedelta.resolution))
for td in [
    timedelta(),
    timedelta(days=1),
    timedelta(days=-1),
    timedelta(seconds=3661),
    timedelta(days=2, seconds=5, microseconds=7),
    timedelta(microseconds=-1),
    timedelta(weeks=1, hours=2, minutes=3, seconds=4, milliseconds=5, microseconds=6),
    timedelta(seconds=0.5),
    timedelta(microseconds=2.5),
    timedelta(microseconds=3.5),
    timedelta(hours=1.5),
]:
    print("td:", repr(td), str(td), td.days, td.seconds, td.microseconds, td.total_seconds(), bool(td))

t1 = timedelta(days=2, seconds=30)
t2 = timedelta(days=1, seconds=15)
print("td ops:", t1 + t2, t1 - t2, +t1, -t1, abs(-t1))
print("td mul:", t2 * 3, 3 * t2, timedelta(microseconds=3) * 0.5, timedelta(microseconds=5) * 0.5)
print("td div:", t1 // t2, t1 / t2, t1 // 2, t1 / 2, timedelta(microseconds=5) / 2.0, t1 % t2, divmod(t1, t2))
print("td cmp:", t1 > t2, t2 < t1, t1 == timedelta(days=2, seconds=30), t1 != t2, t1 == 5)
print("td hash:", hash(t1) == hash(timedelta(hours=48, seconds=30)))
print("td copy:", copy.copy(t1) == t1, copy.deepcopy(t1) == t1)

# 3. tzinfo and timezone
tz_ist = timezone(timedelta(hours=5, minutes=30))
tz_neg = timezone(timedelta(hours=-4, seconds=-30))
tz_named = timezone(timedelta(hours=2), "CEST")
tz_z = timezone(timedelta(0), "Z")
for tz in (timezone.utc, timezone.min, timezone.max, tz_ist, tz_neg, tz_named, tz_z):
    print("tz:", repr(tz), str(tz), tz.tzname(None), tz.utcoffset(None), tz.dst(None))
print("tz eq/hash:", timezone(timedelta(0)) is timezone.utc, tz_z == timezone.utc, hash(tz_z) == hash(timezone.utc))


class FixedOffsetTZ(tzinfo):
    def __init__(self, hours, dst_hours=0, name="FIX"):
        self._off = timedelta(hours=hours)
        self._dst = timedelta(hours=dst_hours)
        self._name = name

    def utcoffset(self, dt):
        return self._off

    def dst(self, dt):
        return self._dst

    def tzname(self, dt):
        return self._name


custom_tz = FixedOffsetTZ(3, 1, "CUSTOM")
dt_custom = datetime(2024, 6, 1, 12, 0, tzinfo=timezone.utc).astimezone(custom_tz)
print("custom tz fromutc:", dt_custom.isoformat(), dt_custom.tzname(), dt_custom.dst(), dt_custom.timetuple().tm_isdst)

# 4. date construction, classmethods, methods, arithmetic, comparisons
print("date limits:", repr(date.min), repr(date.max), repr(date.resolution))
d = date(2024, 2, 29)
print(
    "date props:",
    d.year,
    d.month,
    d.day,
    repr(d),
    str(d),
    d.isoformat(),
    d.ctime(),
    d.toordinal(),
    d.weekday(),
    d.isoweekday(),
)
ic = d.isocalendar()
print("isocalendar:", repr(ic), ic.year, ic.week, ic.weekday, tuple(ic), date.fromisocalendar(ic.year, ic.week, ic.weekday) == d)
print("fromordinal:", date.fromordinal(d.toordinal()) == d, date.fromordinal(1) == date.min, date.fromordinal(3652059) == date.max)
print("replace:", d.replace(year=2025, day=28))
print("date timetuple:", d.timetuple())
print("date arith:", d + timedelta(days=1), timedelta(days=2) + d, d - timedelta(days=29), date(2024, 3, 1) - d)
print("date cmp:", date(2024, 2, 28) < d, d <= d, d > date(2024, 1, 1), d == date(2024, 2, 29), d != date(2024, 3, 1))
print("date hash/copy:", hash(d) == hash(date(2024, 2, 29)), copy.copy(d) == d, copy.deepcopy(d) == d)
print("date format:", format(d, ""), format(d, "%Y/%m/%d"))

for s in ["2024-02-29", "20240229", "2024-W09-4", "2024W094", "2024-W01-1"]:
    print("date.fromisoformat:", s, "->", repr(date.fromisoformat(s)))

d_today = date.today()
print("date.today invariants:", isinstance(d_today, date), d_today.year >= 2020)

# 5. time construction, fromisoformat, isoformat, strftime, comparisons, hashing
print("time limits:", repr(time.min), repr(time.max), repr(time.resolution))
t0 = time(0, 0)
t_full = time(13, 5, 9, 123456, timezone.utc)
t_fold = time(1, 30, fold=1)
print("time basics:", repr(t0), str(t0), bool(t0), repr(t_full), str(t_full), repr(t_fold), t_fold.fold)
for spec in ["auto", "hours", "minutes", "seconds", "milliseconds", "microseconds"]:
    print("time isoformat", spec, "->", t_full.isoformat(spec))
print("time replace:", t_full.replace(hour=8, tzinfo=None, fold=1))
print("time format:", format(t_full, ""), format(t_full, "%H:%M:%S.%f %z %:z %Z"))

for s in [
    "04",
    "04:23",
    "0423",
    "04:23:01",
    "042301",
    "04:23:01.000384",
    "04:23:01,5",
    "04:23:01.123456Z",
    "04:23:01+05:30",
    "04:23:01-0400",
    "04:23:01+02:30:45.123456",
]:
    print("time.fromisoformat:", s, "->", repr(time.fromisoformat(s)))

t_utc = time(12, 0, tzinfo=timezone.utc)
t_p2 = time(14, 0, tzinfo=timezone(timedelta(hours=2)))
t_naive = time(12, 0)
print("time aware cmp/hash:", t_utc == t_p2, hash(t_utc) == hash(t_p2), t_naive == t_utc, t_naive != t_utc)
print("time copy:", copy.copy(t_full) == t_full, copy.deepcopy(t_full) == t_full)

# 6. datetime construction, classmethods, methods, arithmetic, comparisons, hashing
print("dt limits:", repr(datetime.min), repr(datetime.max), repr(datetime.resolution))
dt1 = datetime(2024, 2, 29, 13, 5, 9, 123000, timezone(timedelta(hours=9)))
print(
    "dt basics:",
    repr(dt1),
    str(dt1),
    dt1.date(),
    dt1.time(),
    dt1.timetz(),
    dt1.isoformat(),
    dt1.isoformat(" ", "seconds"),
    dt1.ctime(),
)
print("dt combine:", datetime.combine(date(2024, 5, 6), time(7, 8, 9, tzinfo=UTC)))
print("dt combine override tz:", datetime.combine(date(2024, 5, 6), time(7, 8, 9, tzinfo=UTC), tzinfo=None))
print("dt fromordinal/fromisocal:", datetime.fromordinal(d.toordinal()), datetime.fromisocalendar(2024, 9, 4))
print("dt replace:", dt1.replace(year=2025, day=28, tzinfo=UTC, fold=1))
print("dt timetuples:", dt1.timetuple(), dt1.utctimetuple())
print("dt astimezone:", dt1.astimezone(UTC).isoformat(), dt1.astimezone(tz_ist).isoformat())

# Timestamp round-trip in UTC (platform-independent)
ts_val = 1709211909.123456
dt_from_ts = datetime.fromtimestamp(ts_val, timezone.utc)
print("fromtimestamp UTC:", dt_from_ts.isoformat(), abs(dt_from_ts.timestamp() - ts_val) < 1e-5)
print("utcfromtimestamp:", datetime.utcfromtimestamp(1709211909).isoformat())

now_utc = datetime.now(timezone.utc)
print("now invariants:", isinstance(now_utc, datetime), now_utc.tzinfo is timezone.utc, now_utc.year >= 2020)
print("today/utcnow invariants:", isinstance(datetime.today(), datetime), isinstance(datetime.utcnow(), datetime))

# datetime arithmetic & aware-aware comparisons & date vs datetime rules
dt_u = datetime(2024, 1, 1, 0, 0, tzinfo=UTC)
dt_p2 = datetime(2024, 1, 1, 2, 0, tzinfo=timezone(timedelta(hours=2)))
dt_n = datetime(2024, 1, 1, 0, 0)
d_plain = date(2024, 1, 1)
print("dt arith:", (dt_u + timedelta(hours=3)).isoformat(), (timedelta(minutes=30) + dt_u).isoformat(), dt_p2 - dt_u)
print("dt aware eq/hash:", dt_u == dt_p2, hash(dt_u) == hash(dt_p2), dt_n == dt_u, dt_n != dt_u)
print("date vs dt eq:", d_plain == dt_n, dt_n == d_plain, d_plain != dt_n, dt_n != d_plain)
print("dt copy:", copy.copy(dt1) == dt1, copy.deepcopy(dt1) == dt1)

for s in [
    "2024-02-29",
    "20240229",
    "2024-02-29T13:05",
    "2024-02-29 13:05:09.123456Z",
    "2024-02-29t13:05:09+09:00",
    "2024-W09-4T08:00:00-05:00",
    "20240229T130509Z",
]:
    print("dt.fromisoformat:", s, "->", repr(datetime.fromisoformat(s)))

# 7. strftime and strptime
dt_fmt = datetime(2024, 2, 29, 13, 5, 9, 45000, timezone(timedelta(hours=5, minutes=30), "IST"))
print(
    "strftime full:",
    dt_fmt.strftime("%Y-%y-%m-%d %H:%I:%p:%M:%S.%f j=%j a=%a A=%A w=%w u=%u b=%b B=%B U=%U W=%W G=%G V=%V z=%z :z=%:z Z=%Z %%"),
)
print("date strftime:", date(2024, 1, 1).strftime("%Y-%m-%d %H:%M:%S.%f z=%z Z=%Z"))
print("time strftime:", time(14, 0, tzinfo=timezone(timedelta(hours=2))).strftime("%Y-%m-%d %H:%M:%S.%f z=%z :z=%:z Z=%Z"))
print("dt format:", format(dt_fmt, "%Y/%m/%d %H:%M"))

for text, fmt in [
    ("2024-02-29 13:05:09.045000", "%Y-%m-%d %H:%M:%S.%f"),
    ("29 Feb 2024 01:05 PM", "%d %b %Y %I:%M %p"),
    ("February 29, 2024 12:30 AM", "%B %d, %Y %I:%M %p"),
    ("24-060", "%y-%j"),
    ("2024-W09-4", "%G-W%V-%u"),
    ("2024-02-29 13:05:09+0530", "%Y-%m-%d %H:%M:%S%z"),
    ("2024-02-29 13:05:09+05:30", "%Y-%m-%d %H:%M:%S%z"),
    ("2024-02-29 13:05:09Z", "%Y-%m-%d %H:%M:%S%z"),
    ("2024-02-29 13:05:09+0000 UTC", "%Y-%m-%d %H:%M:%S%z %Z"),
    ("100% 2024-03-01", "100%% %Y-%m-%d"),
]:
    parsed = datetime.strptime(text, fmt)
    print("strptime:", text, "->", repr(parsed))

# 8. Subclassing
class SubTD(timedelta):
    pass


class SubD(date):
    pass


class SubT(time):
    pass


class SubDT(datetime):
    pass


std = SubTD(days=1)
sd = SubD(2024, 1, 2)
stm = SubT(1, 2)
sdt = SubDT(2024, 1, 2, 3, 4)
print("SubTD:", repr(std), type(std + std) is timedelta)
print(
    "SubD:",
    repr(sd),
    type(SubD.fromordinal(sd.toordinal())) is SubD,
    type(sd.replace(day=3)) is SubD,
    type(sd + timedelta(1)) is SubD,
    type(SubD.fromisoformat("2024-01-02")) is SubD,
)
print("SubT:", repr(stm), type(stm.replace(hour=3)) is SubT, type(SubT.fromisoformat("01:02")) is SubT)
print(
    "SubDT:",
    repr(sdt),
    type(sdt.replace(hour=5)) is SubDT,
    type(SubDT.combine(sd, stm)) is SubDT,
    type(sdt + timedelta(1)) is SubDT,
    type(SubDT.strptime("2024-01-02", "%Y-%m-%d")) is SubDT,
)

# 9. Error cases
def check_exc(label, exc_type, fn):
    try:
        fn()
        print("UNEXPECTED OK:", label)
    except exc_type as e:
        print("OK EXC:", label, type(e).__name__)


check_exc("td overflow", OverflowError, lambda: timedelta(days=10**10))
check_exc("td div0", ZeroDivisionError, lambda: timedelta(1) / 0)
check_exc("td floordiv0", ZeroDivisionError, lambda: timedelta(1) // timedelta(0))
check_exc("td bad type", TypeError, lambda: timedelta(days="1"))
check_exc("tz out of range +24h", ValueError, lambda: timezone(timedelta(hours=24)))
check_exc("tz out of range -24h", ValueError, lambda: timezone(timedelta(hours=-24)))
check_exc("tz bad offset type", TypeError, lambda: timezone(5))
check_exc("tzinfo abstract", NotImplementedError, lambda: tzinfo().utcoffset(None))
check_exc("date bad year", ValueError, lambda: date(0, 1, 1))
check_exc("date bad month", ValueError, lambda: date(2024, 13, 1))
check_exc("date bad leap day", ValueError, lambda: date(2023, 2, 29))
check_exc("date bad type float", TypeError, lambda: date(2024.0, 1, 1))
check_exc("date < datetime", TypeError, lambda: d_plain < dt_n)
check_exc("datetime < date", TypeError, lambda: dt_n < d_plain)
check_exc("time bad hour", ValueError, lambda: time(24, 0))
check_exc("time bad fold", ValueError, lambda: time(0, 0, fold=2))
check_exc("time naive < aware", TypeError, lambda: t_naive < t_utc)
check_exc("dt naive < aware", TypeError, lambda: dt_n < dt_u)
check_exc("dt naive - aware", TypeError, lambda: dt_n - dt_u)
check_exc("date fromiso bad", ValueError, lambda: date.fromisoformat("not-a-date"))
check_exc("time fromiso bad", ValueError, lambda: time.fromisoformat("25:00"))
check_exc("dt fromiso bad", ValueError, lambda: datetime.fromisoformat("2024-02-30T10:00"))
check_exc("strptime mismatch", ValueError, lambda: datetime.strptime("2024/02/29", "%Y-%m-%d"))
check_exc("strptime trailing", ValueError, lambda: datetime.strptime("2024-02-29 extra", "%Y-%m-%d"))
