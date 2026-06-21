# Parity tests for the `time` module (issue #2787).
#
# The clock functions are time-sensitive, so they are checked with
# approximate / boolean assertions that hold on both CPython 3.12 and pyrust.
# The calendar conversions are pinned to deterministic epoch instants
# (`gmtime(0)`, `strftime(..., gmtime(0))`) so the output is byte-for-byte
# stable across interpreters and timezones.
#
# See: https://docs.python.org/3/library/time.html
import time

# ── wall clock ───────────────────────────────────────────────────────────────
print(time.time() > 0)                 # True
print(isinstance(time.time(), float))  # True
print(isinstance(time.time_ns(), int)) # True
print(time.time_ns() > 0)              # True

# ── monotonic / perf_counter never decrease ──────────────────────────────────
m1 = time.monotonic()
time.sleep(0.001)
m2 = time.monotonic()
print(m2 >= m1)                            # True
print(isinstance(time.perf_counter(), float))  # True
print(time.perf_counter_ns() >= 0)             # True
print(isinstance(time.perf_counter_ns(), int)) # True
print(isinstance(time.monotonic_ns(), int))    # True

# ── process_time ─────────────────────────────────────────────────────────────
print(time.process_time() >= 0)               # True
print(isinstance(time.process_time(), float)) # True
print(isinstance(time.process_time_ns(), int))# True

# ── sleep ────────────────────────────────────────────────────────────────────
before = time.perf_counter()
time.sleep(0.01)
print(time.perf_counter() - before >= 0.005)  # True (slept >= 5ms)
print(time.sleep(0) is None)                  # True (zero sleep returns None)

try:
    time.sleep(-1)
except ValueError as e:
    print("sleep negative:", e)               # sleep length must be non-negative

# ── struct_time shape (deterministic via the epoch) ──────────────────────────
gt = time.gmtime(0)
print(tuple(gt))                              # (1970, 1, 1, 0, 0, 0, 3, 1, 0)
print(len(gt) == 9)                           # True
print(gt[0] == gt.tm_year)                    # True
print(gt.tm_year, gt.tm_mon, gt.tm_mday)      # 1970 1 1
print(gt.tm_hour, gt.tm_min, gt.tm_sec)       # 0 0 0
print(gt.tm_wday, gt.tm_yday, gt.tm_isdst)    # 3 1 0
print(type(gt).__name__)                      # struct_time
print(isinstance(gt, tuple))                  # True

# A non-epoch instant: New Year 2021 12:34:56 UTC.
t2021 = time.gmtime(1609504496)
print(tuple(t2021))                           # (2021, 1, 1, 12, 34, 56, 4, 1, 0)

# struct_time is constructed from a single nine-element iterable (CPython form).
manual = time.struct_time((2020, 6, 15, 1, 2, 3, 0, 167, 0))
print(manual.tm_year, manual.tm_mon, manual[2])  # 2020 6 15
print(manual == (2020, 6, 15, 1, 2, 3, 0, 167, 0))  # True
try:
    time.struct_time((1, 2, 3))               # wrong length -> TypeError
except TypeError:
    print("struct_time short seq -> TypeError")

# gmtime() with no argument is "now": still a valid calendar date.
now = time.gmtime()
print(now.tm_year >= 2024)                    # True
print(1 <= now.tm_mon <= 12)                  # True
print(1 <= now.tm_mday <= 31)                 # True
print(0 <= now.tm_hour <= 23)                 # True

# localtime() likewise produces a current-year struct_time.
lt = time.localtime()
print(lt.tm_year >= 2024)                     # True

# ── mktime is the inverse of localtime ───────────────────────────────────────
ts = 1609504496
print(time.mktime(time.localtime(ts)) == float(ts))  # True
print(abs(time.mktime(time.localtime()) - time.time()) < 2)  # True

# ── strftime (deterministic via the epoch) ───────────────────────────────────
print(time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(0)))  # 1970-01-01 00:00:00
print(time.strftime("%A", time.gmtime(0)))                 # Thursday
print(time.strftime("%B", time.gmtime(0)))                 # January
print(time.strftime("%j", time.gmtime(0)))                 # 001
year_str = time.strftime("%Y")
print(len(year_str) == 4 and year_str.isdigit())          # True
print(int(year_str) >= 2024)                              # True

try:
    time.strftime(123)
except TypeError:
    print("strftime non-str -> TypeError")

# ── timezone constants ───────────────────────────────────────────────────────
print(isinstance(time.timezone, int))   # True
print(isinstance(time.altzone, int))    # True
print(isinstance(time.daylight, int))   # True
print(time.daylight in (0, 1))          # True
tz = time.tzname
print(isinstance(tz, tuple))            # True
print(len(tz) == 2)                     # True
print(all(isinstance(n, str) for n in tz))  # True

print("time ok")
