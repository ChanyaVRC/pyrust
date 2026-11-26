"""
Python 3.12 ``datetime`` module backed by calendar/formatting helpers in
``datetime.rs`` (issue #2805).
"""

__all__ = (
    "date",
    "datetime",
    "time",
    "timedelta",
    "timezone",
    "tzinfo",
    "MINYEAR",
    "MAXYEAR",
    "UTC",
)

MINYEAR = 1
MAXYEAR = 9999
_MAX_TD_DAYS = 999999999
_US_PER_SEC = 1000000
_US_PER_DAY = 86400 * _US_PER_SEC


def _round_div_even(num, den):
    q, r = divmod(num, den)
    r2 = r * 2
    if r2 > den or (r2 == den and (q & 1)):
        q += 1
    return q


def _require_int(val, name):
    if isinstance(val, int):
        return int(val)
    if isinstance(val, float):
        raise TypeError("'" + type(val).__name__ + "' object cannot be interpreted as an integer")
    try:
        return _operator.index(val)
    except TypeError:
        raise TypeError("'" + type(val).__name__ + "' object cannot be interpreted as an integer")


_UNSET = object()

_DATE_INTERNAL = frozenset(("_year", "_month", "_day"))
_TIME_INTERNAL = frozenset(("_hour", "_minute", "_second", "_microsecond", "_tzinfo", "_fold"))
_DATETIME_INTERNAL = _DATE_INTERNAL | _TIME_INTERNAL
_TIMEDELTA_INTERNAL = frozenset(("_days", "_seconds", "_microseconds"))
_TIMEZONE_INTERNAL = frozenset(("_offset", "_name"))


def _user_state(obj, internal):
    # Mirror ``object.__getstate__``: only user-visible instance attributes
    # (e.g. those a subclass adds), never the backing fields.
    state = {k: v for k, v in obj.__dict__.items() if k not in internal}
    return state or None


def _missing_arg(name, pos):
    return TypeError("function missing required argument '" + name + "' (pos " + str(pos) + ")")


def _raise_missing_ymd(year, month):
    # Arguments are converted in order before the first missing one is
    # reported (``date(b"bad")`` fails on the bytes year first).
    _require_int(year, "year")
    if month is _UNSET:
        raise _missing_arg("month", 2)
    _require_int(month, "month")
    raise _missing_arg("day", 3)


def _pickled_state(state, size, idx, mask, lo, hi, kind):
    """Return the packed ``bytes`` payload when ``state`` is a pickled
    ``date``/``datetime``/``time`` state (CPython's compact format), else None.

    Only the length and a month/hour sanity check are applied, exactly like
    CPython; the remaining fields are taken verbatim.
    """
    if isinstance(state, bytes):
        if len(state) == size and lo <= (state[idx] & mask) <= hi:
            return state
        return None
    if isinstance(state, str):
        if len(state) == size and lo <= (ord(state[idx]) & mask) <= hi:
            try:
                return state.encode("latin1")
            except UnicodeEncodeError:
                raise ValueError(
                    "Failed to encode latin1 string when unpickling a "
                    + kind
                    + " object. pickle.load(data, encoding='latin1') is assumed."
                )
    return None


def _require_tzinfo(tz):
    if tz is not None and not isinstance(tz, tzinfo):
        raise TypeError("tzinfo argument must be None or of a tzinfo subclass, not type '" + type(tz).__name__ + "'")


def _check_offset_bounds(name, offset):
    if offset is None:
        return None
    if not isinstance(offset, timedelta):
        raise TypeError("tzinfo." + name + "() must return None or timedelta, not '" + type(offset).__name__ + "'")
    if not (-timedelta(hours=24) < offset < timedelta(hours=24)):
        raise ValueError("offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24).")
    return offset


def _cls_repr_prefix(cls):
    if cls.__module__ == "datetime":
        return "datetime." + cls.__name__
    return cls.__name__


def _fmt_tz_suffix(offset):
    if offset is None:
        return ""
    total_us = offset._to_us()
    sign = "-" if total_us < 0 else "+"
    if total_us < 0:
        total_us = -total_us
    total_s, rem_us = divmod(total_us, _US_PER_SEC)
    hh, rem_s = divmod(total_s, 3600)
    mm, ss = divmod(rem_s, 60)
    base = "%s%02d:%02d" % (sign, hh, mm)
    if rem_us:
        return "%s:%02d.%06d" % (base, ss, rem_us)
    if ss:
        return "%s:%02d" % (base, ss)
    return base


def _tz_from_us(tz_us):
    if tz_us is None:
        return None
    if tz_us == 0:
        return timezone.utc
    return timezone(timedelta(microseconds=tz_us))


class IsoCalendarDate(tuple):
    def __new__(cls, year, week, weekday):
        return tuple.__new__(cls, (year, week, weekday))

    @property
    def year(self):
        return self[0]

    @property
    def week(self):
        return self[1]

    @property
    def weekday(self):
        return self[2]

    def __repr__(self):
        return (
            "datetime.IsoCalendarDate(year="
            + repr(self[0])
            + ", week="
            + repr(self[1])
            + ", weekday="
            + repr(self[2])
            + ")"
        )


def _scale_component_to_us(val, factor_us):
    if isinstance(val, int):
        return int(val) * factor_us, 1
    if isinstance(val, float):
        if not _math.isfinite(val):
            raise OverflowError("cannot convert float infinity or NaN to integer")
        num, den = val.as_integer_ratio()
        return num * factor_us, den
    raise TypeError("unsupported type for timedelta component: " + type(val).__name__)


class timedelta:
    def __new__(
        cls,
        days=0,
        seconds=0,
        microseconds=0,
        milliseconds=0,
        minutes=0,
        hours=0,
        weeks=0,
    ):
        parts = (
            (days, _US_PER_DAY),
            (weeks, 7 * _US_PER_DAY),
            (hours, 3600 * _US_PER_SEC),
            (minutes, 60 * _US_PER_SEC),
            (seconds, _US_PER_SEC),
            (milliseconds, 1000),
            (microseconds, 1),
        )
        num = 0
        den = 1
        for raw, factor in parts:
            if raw == 0 and isinstance(raw, int):
                continue
            p_num, p_den = _scale_component_to_us(raw, factor)
            if p_den == den:
                num += p_num
            else:
                num = num * p_den + p_num * den
                den *= p_den
        total_us = num if den == 1 else _round_div_even(num, den)
        return cls._from_us(total_us)

    @classmethod
    def _from_us(cls, total_us):
        d, rem = divmod(total_us, _US_PER_DAY)
        s, us = divmod(rem, _US_PER_SEC)
        if abs(d) > _MAX_TD_DAYS:
            raise OverflowError("timedelta days overflow: " + str(d))
        obj = object.__new__(cls)
        obj._days = d
        obj._seconds = s
        obj._microseconds = us
        return obj

    def _to_us(self):
        return (self._days * 86400 + self._seconds) * _US_PER_SEC + self._microseconds

    @property
    def days(self):
        return self._days

    @property
    def seconds(self):
        return self._seconds

    @property
    def microseconds(self):
        return self._microseconds

    def total_seconds(self):
        return self._to_us() / _US_PER_SEC

    def __bool__(self):
        return self._days != 0 or self._seconds != 0 or self._microseconds != 0

    def __pos__(self):
        return timedelta._from_us(self._to_us())

    def __neg__(self):
        return timedelta._from_us(-self._to_us())

    def __abs__(self):
        us = self._to_us()
        return timedelta._from_us(-us if us < 0 else us)

    def __add__(self, other):
        if isinstance(other, timedelta):
            return timedelta._from_us(self._to_us() + other._to_us())
        return NotImplemented

    def __radd__(self, other):
        return self.__add__(other)

    def __sub__(self, other):
        if isinstance(other, timedelta):
            return timedelta._from_us(self._to_us() - other._to_us())
        return NotImplemented

    def __rsub__(self, other):
        if isinstance(other, timedelta):
            return timedelta._from_us(other._to_us() - self._to_us())
        return NotImplemented

    def __mul__(self, other):
        if isinstance(other, int):
            return timedelta._from_us(self._to_us() * int(other))
        if isinstance(other, float):
            if not _math.isfinite(other):
                raise OverflowError("cannot convert float infinity or NaN to integer")
            num, den = other.as_integer_ratio()
            return timedelta._from_us(_round_div_even(self._to_us() * num, den))
        return NotImplemented

    def __rmul__(self, other):
        return self.__mul__(other)

    def __floordiv__(self, other):
        if isinstance(other, timedelta):
            b = other._to_us()
            if b == 0:
                raise ZeroDivisionError("integer division or modulo by zero")
            return self._to_us() // b
        if isinstance(other, int):
            n = int(other)
            if n == 0:
                raise ZeroDivisionError("integer division or modulo by zero")
            return timedelta._from_us(self._to_us() // n)
        return NotImplemented

    def __truediv__(self, other):
        if isinstance(other, timedelta):
            b = other._to_us()
            if b == 0:
                raise ZeroDivisionError("division by zero")
            return self._to_us() / b
        if isinstance(other, int):
            n = int(other)
            if n == 0:
                raise ZeroDivisionError("division by zero")
            return timedelta._from_us(_round_div_even(self._to_us(), n))
        if isinstance(other, float):
            if _math.isnan(other):
                raise ValueError("cannot convert float NaN to integer")
            if _math.isinf(other):
                return timedelta._from_us(0)
            if other == 0.0:
                raise ZeroDivisionError("division by zero")
            num, den = other.as_integer_ratio()
            return timedelta._from_us(_round_div_even(self._to_us() * den, num))
        return NotImplemented

    def __mod__(self, other):
        if isinstance(other, timedelta):
            b = other._to_us()
            if b == 0:
                raise ZeroDivisionError("integer division or modulo by zero")
            return timedelta._from_us(self._to_us() % b)
        return NotImplemented

    def __divmod__(self, other):
        if isinstance(other, timedelta):
            b = other._to_us()
            if b == 0:
                raise ZeroDivisionError("integer division or modulo by zero")
            q, r = divmod(self._to_us(), b)
            return q, timedelta._from_us(r)
        return NotImplemented

    def __eq__(self, other):
        if isinstance(other, timedelta):
            return self._to_us() == other._to_us()
        return NotImplemented

    def __ne__(self, other):
        if isinstance(other, timedelta):
            return self._to_us() != other._to_us()
        return NotImplemented

    def __lt__(self, other):
        if isinstance(other, timedelta):
            return self._to_us() < other._to_us()
        return NotImplemented

    def __le__(self, other):
        if isinstance(other, timedelta):
            return self._to_us() <= other._to_us()
        return NotImplemented

    def __gt__(self, other):
        if isinstance(other, timedelta):
            return self._to_us() > other._to_us()
        return NotImplemented

    def __ge__(self, other):
        if isinstance(other, timedelta):
            return self._to_us() >= other._to_us()
        return NotImplemented

    def __hash__(self):
        return hash((self._days, self._seconds, self._microseconds))

    def __copy__(self):
        return self

    def __deepcopy__(self, memo):
        return self

    def __reduce__(self):
        return (type(self), (self._days, self._seconds, self._microseconds))

    def __getstate__(self):
        return _user_state(self, _TIMEDELTA_INTERNAL)

    def __str__(self):
        mm, ss = divmod(self._seconds, 60)
        hh, mm = divmod(mm, 60)
        res = "%d:%02d:%02d" % (hh, mm, ss)
        if self._days:
            unit = "day" if abs(self._days) == 1 else "days"
            res = "%d %s, %s" % (self._days, unit, res)
        if self._microseconds:
            res = "%s.%06d" % (res, self._microseconds)
        return res

    def __repr__(self):
        parts = []
        if self._days:
            parts.append("days=" + repr(self._days))
        if self._seconds:
            parts.append("seconds=" + repr(self._seconds))
        if self._microseconds:
            parts.append("microseconds=" + repr(self._microseconds))
        if not parts:
            parts.append("0")
        return _cls_repr_prefix(type(self)) + "(" + ", ".join(parts) + ")"


timedelta.min = timedelta(-_MAX_TD_DAYS)
timedelta.max = timedelta(days=_MAX_TD_DAYS, hours=23, minutes=59, seconds=59, microseconds=999999)
timedelta.resolution = timedelta(microseconds=1)


class tzinfo:
    def __reduce__(self):
        getinitargs = getattr(self, "__getinitargs__", None)
        args = getinitargs() if getinitargs is not None else ()
        return (type(self), args, self.__getstate__())

    def tzname(self, dt):
        raise NotImplementedError("tzinfo subclass must override tzname()")

    def utcoffset(self, dt):
        raise NotImplementedError("tzinfo subclass must override utcoffset()")

    def dst(self, dt):
        raise NotImplementedError("tzinfo subclass must override dst()")

    def fromutc(self, dt):
        if not isinstance(dt, datetime):
            raise TypeError("fromutc() requires a datetime argument")
        if dt.tzinfo is not self:
            raise ValueError("dt.tzinfo is not self")
        dtoff = dt.utcoffset()
        if dtoff is None:
            raise ValueError("fromutc() requires a non-None utcoffset() result")
        dtdst = dt.dst()
        if dtdst is None:
            raise ValueError("fromutc() requires a non-None dst() result")
        delta = dtoff - dtdst
        if delta:
            dt += delta
            dtdst = dt.dst()
            if dtdst is None:
                raise ValueError("fromutc(): dt.dst gave inconsistent results; cannot convert")
        return dt + dtdst


_TZInfo = tzinfo


class timezone(tzinfo):
    _MAX_OFFSET = timedelta(hours=23, minutes=59)
    _MIN_OFFSET = -_MAX_OFFSET

    def __new__(cls, offset, name=None):
        if not isinstance(offset, timedelta):
            raise TypeError("offset must be a timedelta")
        if name is not None and not isinstance(name, str):
            raise TypeError("name must be a string")
        if not (-timedelta(hours=24) < offset < timedelta(hours=24)):
            raise ValueError("offset must be a timedelta strictly between -timedelta(hours=24) and timedelta(hours=24).")
        if name is None and offset == timedelta(0) and hasattr(cls, "utc"):
            return cls.utc
        obj = object.__new__(cls)
        obj._offset = offset
        obj._name = name
        return obj

    def __getinitargs__(self):
        if self._name is None:
            return (self._offset,)
        return (self._offset, self._name)

    def __getstate__(self):
        return _user_state(self, _TIMEZONE_INTERNAL)

    def utcoffset(self, dt):
        if dt is None or isinstance(dt, datetime):
            return self._offset
        raise TypeError("utcoffset() argument must be a datetime instance or None")

    def dst(self, dt):
        if dt is None or isinstance(dt, datetime):
            return None
        raise TypeError("dst() argument must be a datetime instance or None")

    def tzname(self, dt):
        if dt is not None and not isinstance(dt, datetime):
            raise TypeError("tzname() argument must be a datetime instance or None")
        if self._name is not None:
            return self._name
        if self._offset == timedelta(0):
            return "UTC"
        return "UTC" + _fmt_tz_suffix(self._offset)

    def fromutc(self, dt):
        if not isinstance(dt, datetime):
            raise TypeError("fromutc() argument must be a datetime")
        if dt.tzinfo is not self:
            raise ValueError("fromutc: dt.tzinfo is not self")
        return dt + self._offset

    def __eq__(self, other):
        if isinstance(other, timezone):
            return self._offset == other._offset
        return NotImplemented

    def __hash__(self):
        return hash(self._offset)

    def __str__(self):
        return self.tzname(None)

    def __repr__(self):
        if self is timezone.utc:
            return "datetime.timezone.utc"
        if self._name is None:
            return "datetime.timezone(" + repr(self._offset) + ")"
        return "datetime.timezone(" + repr(self._offset) + ", " + repr(self._name) + ")"


timezone.utc = timezone(timedelta(0))
timezone.min = timezone(timezone._MIN_OFFSET)
timezone.max = timezone(timezone._MAX_OFFSET)
UTC = timezone.utc


def _format_hms_timespec(h, m, s, us, timespec):
    if timespec == "auto":
        timespec = "microseconds" if us else "seconds"
    if timespec == "hours":
        return "%02d" % h
    if timespec == "minutes":
        return "%02d:%02d" % (h, m)
    if timespec == "seconds":
        return "%02d:%02d:%02d" % (h, m, s)
    if timespec == "milliseconds":
        return "%02d:%02d:%02d.%03d" % (h, m, s, us // 1000)
    if timespec == "microseconds":
        return "%02d:%02d:%02d.%06d" % (h, m, s, us)
    raise ValueError("Unknown timespec value")


def _build_struct_time(y, mo, d, h, mi, se, isdst):
    ord_val = _ymd_to_ordinal(y, mo, d)
    wday = (ord_val - 1) % 7
    yday = ord_val - _ymd_to_ordinal(y, 1, 1) + 1
    return _time.struct_time((y, mo, d, h, mi, se, wday, yday, isdst))


def _check_time_parts(hour, minute, second, microsecond, fold):
    hour = _require_int(hour, "hour")
    minute = _require_int(minute, "minute")
    second = _require_int(second, "second")
    microsecond = _require_int(microsecond, "microsecond")
    fold = _require_int(fold, "fold")
    if not (0 <= hour <= 23):
        raise ValueError("hour must be in 0..23")
    if not (0 <= minute <= 59):
        raise ValueError("minute must be in 0..59")
    if not (0 <= second <= 59):
        raise ValueError("second must be in 0..59")
    if not (0 <= microsecond <= 999999):
        raise ValueError("microsecond must be in 0..999999")
    if fold not in (0, 1):
        raise ValueError("fold must be either 0 or 1")
    return hour, minute, second, microsecond, fold


def _local_tz_for_timestamp(ts):
    lt = _time.localtime(ts)
    gt = _time.gmtime(ts)
    local_ord = _ymd_to_ordinal(lt.tm_year, lt.tm_mon, lt.tm_mday)
    gmt_ord = _ymd_to_ordinal(gt.tm_year, gt.tm_mon, gt.tm_mday)
    delta_secs = (
        (local_ord - gmt_ord) * 86400
        + (lt.tm_hour - gt.tm_hour) * 3600
        + (lt.tm_min - gt.tm_min) * 60
        + (lt.tm_sec - gt.tm_sec)
    )
    tz_names = getattr(_time, "tzname", ("UTC", "UTC"))
    name = tz_names[1] if (lt.tm_isdst > 0 and len(tz_names) > 1) else tz_names[0]
    return timezone(timedelta(seconds=delta_secs), name)


class date:
    def __new__(cls, year=_UNSET, month=_UNSET, day=_UNSET):
        if month is _UNSET or day is _UNSET:
            if year is _UNSET:
                raise _missing_arg("year", 1)
            if month is _UNSET and day is _UNSET:
                data = _pickled_state(year, 4, 2, 0xFF, 1, 12, "date")
                if data is not None:
                    obj = object.__new__(cls)
                    obj._year = (data[0] << 8) | data[1]
                    obj._month = data[2]
                    obj._day = data[3]
                    return obj
            _raise_missing_ymd(year, month)
        year = _require_int(year, "year")
        month = _require_int(month, "month")
        day = _require_int(day, "day")
        _ymd_to_ordinal(year, month, day)
        obj = object.__new__(cls)
        obj._year = year
        obj._month = month
        obj._day = day
        return obj

    @classmethod
    def fromtimestamp(cls, t):
        lt = _time.localtime(t)
        return cls(lt.tm_year, lt.tm_mon, lt.tm_mday)

    @classmethod
    def today(cls):
        return cls.fromtimestamp(_time.time())

    @classmethod
    def fromordinal(cls, n):
        n = _require_int(n, "ordinal")
        y, m, d = _ordinal_to_ymd(n)
        return cls(y, m, d)

    @classmethod
    def fromisoformat(cls, date_string):
        if not isinstance(date_string, str):
            raise TypeError("fromisoformat: argument must be str")
        y, m, d = _parse_iso_date(date_string)
        return cls(y, m, d)

    @classmethod
    def fromisocalendar(cls, year, week, day):
        year = _require_int(year, "year")
        week = _require_int(week, "week")
        day = _require_int(day, "day")
        y, m, d = _from_iso_calendar(year, week, day)
        return cls(y, m, d)

    @property
    def year(self):
        return self._year

    @property
    def month(self):
        return self._month

    @property
    def day(self):
        return self._day

    def replace(self, year=None, month=None, day=None):
        if year is None:
            year = self._year
        if month is None:
            month = self._month
        if day is None:
            day = self._day
        return type(self)(year, month, day)

    def toordinal(self):
        return _ymd_to_ordinal(self._year, self._month, self._day)

    def weekday(self):
        return (self.toordinal() - 1) % 7

    def isoweekday(self):
        return self.weekday() + 1

    def isocalendar(self):
        iy, iw, iday = _iso_calendar(self._year, self._month, self._day)
        return IsoCalendarDate(iy, iw, iday)

    def timetuple(self):
        return _build_struct_time(self._year, self._month, self._day, 0, 0, 0, -1)

    def isoformat(self):
        return "%04d-%02d-%02d" % (self._year, self._month, self._day)

    def ctime(self):
        return _format_strftime("%a %b ", self._year, self._month, self._day, 0, 0, 0, 0, None, None) + "%2d 00:00:00 %04d" % (self._day, self._year)

    def strftime(self, fmt):
        if not isinstance(fmt, str):
            raise TypeError("strftime() argument 1 must be str, not " + type(fmt).__name__)
        return _format_strftime(fmt, self._year, self._month, self._day, 0, 0, 0, 0, None, None)

    def __format__(self, fmt):
        if not isinstance(fmt, str):
            raise TypeError("must be str, not " + type(fmt).__name__)
        if len(fmt) != 0:
            return self.strftime(fmt)
        return str(self)

    def __str__(self):
        return self.isoformat()

    def __repr__(self):
        return "%s(%d, %d, %d)" % (_cls_repr_prefix(type(self)), self._year, self._month, self._day)

    def __add__(self, other):
        if isinstance(other, timedelta):
            ord_val = self.toordinal() + other.days
            if not (1 <= ord_val <= 3652059):
                raise OverflowError("result out of range")
            y, m, d = _ordinal_to_ymd(ord_val)
            return type(self)(y, m, d)
        return NotImplemented

    def __radd__(self, other):
        return self.__add__(other)

    def __sub__(self, other):
        if isinstance(other, timedelta):
            return self + timedelta(-other.days)
        if isinstance(other, date) and not isinstance(other, datetime):
            return timedelta(days=self.toordinal() - other.toordinal())
        return NotImplemented

    def _cmp_tuple(self):
        return (self._year, self._month, self._day)

    def __eq__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return self._cmp_tuple() == other._cmp_tuple()
        return NotImplemented

    def __ne__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return self._cmp_tuple() != other._cmp_tuple()
        return NotImplemented

    def __lt__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return self._cmp_tuple() < other._cmp_tuple()
        return NotImplemented

    def __le__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return self._cmp_tuple() <= other._cmp_tuple()
        return NotImplemented

    def __gt__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return self._cmp_tuple() > other._cmp_tuple()
        return NotImplemented

    def __ge__(self, other):
        if isinstance(other, date) and not isinstance(other, datetime):
            return self._cmp_tuple() >= other._cmp_tuple()
        return NotImplemented

    def __hash__(self):
        return hash((self._year, self._month, self._day))

    def __copy__(self):
        return self

    def __deepcopy__(self, memo):
        return self

    def __reduce__(self):
        y = self._year
        return (type(self), (bytes((y >> 8, y & 0xFF, self._month, self._day)),))

    def __getstate__(self):
        return _user_state(self, _DATETIME_INTERNAL)


date.min = date(MINYEAR, 1, 1)
date.max = date(MAXYEAR, 12, 31)
date.resolution = timedelta(days=1)


class time:
    def __new__(cls, hour=0, minute=_UNSET, second=0, microsecond=0, tzinfo=None, *, fold=0):
        if isinstance(hour, (bytes, str)) and second == 0 and microsecond == 0 and tzinfo is None:
            data = _pickled_state(hour, 6, 0, 0x7F, 0, 23, "time")
            if data is not None:
                # ``minute`` carries the optional pickled tzinfo.
                tz = None if minute is _UNSET else minute
                if tz is not None and not isinstance(tz, _TZInfo):
                    raise TypeError("bad tzinfo state arg")
                obj = object.__new__(cls)
                obj._hour = data[0] & 0x7F
                obj._minute = data[1]
                obj._second = data[2]
                obj._microsecond = (data[3] << 16) | (data[4] << 8) | data[5]
                obj._tzinfo = tz
                obj._fold = 1 if data[0] & 0x80 else 0
                return obj
        if minute is _UNSET:
            minute = 0
        hour, minute, second, microsecond, fold = _check_time_parts(
            hour, minute, second, microsecond, fold
        )
        _require_tzinfo(tzinfo)
        obj = object.__new__(cls)
        obj._hour = hour
        obj._minute = minute
        obj._second = second
        obj._microsecond = microsecond
        obj._tzinfo = tzinfo
        obj._fold = fold
        return obj

    @classmethod
    def fromisoformat(cls, time_string):
        if not isinstance(time_string, str):
            raise TypeError("fromisoformat: argument must be str")
        h, mi, se, us, tz_us = _parse_iso_time(time_string)
        return cls(h, mi, se, us, _tz_from_us(tz_us))

    @property
    def hour(self):
        return self._hour

    @property
    def minute(self):
        return self._minute

    @property
    def second(self):
        return self._second

    @property
    def microsecond(self):
        return self._microsecond

    @property
    def tzinfo(self):
        return self._tzinfo

    @property
    def fold(self):
        return self._fold

    def utcoffset(self):
        if self._tzinfo is None:
            return None
        return _check_offset_bounds("utcoffset", self._tzinfo.utcoffset(None))

    def dst(self):
        if self._tzinfo is None:
            return None
        return _check_offset_bounds("dst", self._tzinfo.dst(None))

    def tzname(self):
        if self._tzinfo is None:
            return None
        name = self._tzinfo.tzname(None)
        if name is not None and not isinstance(name, str):
            raise TypeError("tzinfo.tzname() must return None or string, not '" + type(name).__name__ + "'")
        return name

    def replace(
        self,
        hour=None,
        minute=None,
        second=None,
        microsecond=None,
        tzinfo=True,
        *,
        fold=None,
    ):
        if hour is None:
            hour = self._hour
        if minute is None:
            minute = self._minute
        if second is None:
            second = self._second
        if microsecond is None:
            microsecond = self._microsecond
        if tzinfo is True:
            tzinfo = self._tzinfo
        if fold is None:
            fold = self._fold
        return type(self)(hour, minute, second, microsecond, tzinfo, fold=fold)

    def isoformat(self, timespec="auto"):
        s = _format_hms_timespec(self._hour, self._minute, self._second, self._microsecond, timespec)
        return s + _fmt_tz_suffix(self.utcoffset())

    def strftime(self, fmt):
        if not isinstance(fmt, str):
            raise TypeError("strftime() argument 1 must be str, not " + type(fmt).__name__)
        off = self.utcoffset()
        off_us = None if off is None else off._to_us()
        return _format_strftime(
            fmt, 1900, 1, 1, self._hour, self._minute, self._second, self._microsecond, off_us, self.tzname()
        )

    def __format__(self, fmt):
        if not isinstance(fmt, str):
            raise TypeError("must be str, not " + type(fmt).__name__)
        if len(fmt) != 0:
            return self.strftime(fmt)
        return str(self)

    def __bool__(self):
        return True

    def __str__(self):
        return self.isoformat()

    def __repr__(self):
        if self._microsecond != 0:
            s = "%s(%d, %d, %d, %d" % (
                _cls_repr_prefix(type(self)),
                self._hour,
                self._minute,
                self._second,
                self._microsecond,
            )
        elif self._second != 0:
            s = "%s(%d, %d, %d" % (_cls_repr_prefix(type(self)), self._hour, self._minute, self._second)
        else:
            s = "%s(%d, %d" % (_cls_repr_prefix(type(self)), self._hour, self._minute)
        if self._tzinfo is not None:
            s += ", tzinfo=" + repr(self._tzinfo)
        if self._fold:
            s += ", fold=1"
        return s + ")"

    def _time_us(self):
        return (self._hour * 3600 + self._minute * 60 + self._second) * _US_PER_SEC + self._microsecond

    def _cmp_key(self, other, allow_mixed):
        my_off = self.utcoffset()
        ot_off = other.utcoffset()
        if (my_off is None) != (ot_off is None):
            if allow_mixed:
                return None, None
            raise TypeError("can't compare offset-naive and offset-aware times")
        if my_off is None or my_off == ot_off:
            return self._time_us(), other._time_us()
        return self._time_us() - my_off._to_us(), other._time_us() - ot_off._to_us()

    def __eq__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other, True)
        if a is None:
            return False
        return a == b

    def __ne__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other, True)
        if a is None:
            return True
        return a != b

    def __lt__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other, False)
        return a < b

    def __le__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other, False)
        return a <= b

    def __gt__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other, False)
        return a > b

    def __ge__(self, other):
        if not isinstance(other, time):
            return NotImplemented
        a, b = self._cmp_key(other, False)
        return a >= b

    def __hash__(self):
        t = self.replace(fold=0) if self._fold else self
        off = t.utcoffset()
        if off is None:
            return hash(("naive_time", t._time_us()))
        return hash(("aware_time", t._time_us() - off._to_us()))

    def __copy__(self):
        return self

    def __deepcopy__(self, memo):
        return self

    def __reduce__(self):
        return self.__reduce_ex__(2)

    def __reduce_ex__(self, protocol):
        # The fold bit is only part of the pickled state from protocol 4 on,
        # so pickles for older protocols stay readable by older Pythons.
        us = self._microsecond
        data = bytes(
            (
                self._hour | (self._fold << 7 if protocol > 3 else 0),
                self._minute,
                self._second,
                us >> 16,
                (us >> 8) & 0xFF,
                us & 0xFF,
            )
        )
        if self._tzinfo is None:
            return (type(self), (data,))
        return (type(self), (data, self._tzinfo))

    def __getstate__(self):
        return _user_state(self, _TIME_INTERNAL)


time.min = time(0, 0)
time.max = time(23, 59, 59, 999999)
time.resolution = timedelta(microseconds=1)


class datetime(date):
    def __new__(
        cls,
        year=_UNSET,
        month=_UNSET,
        day=_UNSET,
        hour=0,
        minute=0,
        second=0,
        microsecond=0,
        tzinfo=None,
        *,
        fold=0,
    ):
        if day is _UNSET or month is _UNSET:
            if year is _UNSET:
                raise _missing_arg("year", 1)
            if day is _UNSET:
                data = _pickled_state(year, 10, 2, 0x7F, 1, 12, "datetime")
                if data is not None:
                    # ``month`` carries the optional pickled tzinfo.
                    tz = None if month is _UNSET else month
                    if tz is not None and not isinstance(tz, _TZInfo):
                        raise TypeError("bad tzinfo state arg")
                    obj = object.__new__(cls)
                    obj._year = (data[0] << 8) | data[1]
                    obj._month = data[2] & 0x7F
                    obj._day = data[3]
                    obj._hour = data[4]
                    obj._minute = data[5]
                    obj._second = data[6]
                    obj._microsecond = (data[7] << 16) | (data[8] << 8) | data[9]
                    obj._tzinfo = tz
                    obj._fold = 1 if data[2] & 0x80 else 0
                    return obj
            _raise_missing_ymd(year, month)
        year = _require_int(year, "year")
        month = _require_int(month, "month")
        day = _require_int(day, "day")
        _ymd_to_ordinal(year, month, day)
        hour, minute, second, microsecond, fold = _check_time_parts(
            hour, minute, second, microsecond, fold
        )
        _require_tzinfo(tzinfo)
        obj = object.__new__(cls)
        obj._year = year
        obj._month = month
        obj._day = day
        obj._hour = hour
        obj._minute = minute
        obj._second = second
        obj._microsecond = microsecond
        obj._tzinfo = tzinfo
        obj._fold = fold
        return obj

    @property
    def hour(self):
        return self._hour

    @property
    def minute(self):
        return self._minute

    @property
    def second(self):
        return self._second

    @property
    def microsecond(self):
        return self._microsecond

    @property
    def tzinfo(self):
        return self._tzinfo

    @property
    def fold(self):
        return self._fold

    @classmethod
    def _from_utc_timestamp(cls, ts, tz):
        if isinstance(ts, float) and not _math.isfinite(ts):
            raise ValueError("timestamp out of range")
        total_us = _round_div_even(int(ts * _US_PER_SEC * 2), 2) if isinstance(ts, float) else int(ts) * _US_PER_SEC
        if isinstance(ts, float):
            num, den = ts.as_integer_ratio()
            total_us = _round_div_even(num * _US_PER_SEC, den)
        days, rem_us = divmod(total_us, _US_PER_DAY)
        secs, us = divmod(rem_us, _US_PER_SEC)
        hh, rem_s = divmod(secs, 3600)
        mm, ss = divmod(rem_s, 60)
        ord_val = 719163 + days
        if not (1 <= ord_val <= 3652059):
            raise OverflowError("timestamp out of range for platform time_t")
        y, mo, d = _ordinal_to_ymd(ord_val)
        res = cls(y, mo, d, hh, mm, ss, us, tzinfo=tz)
        if tz is not None:
            res = tz.fromutc(res)
        return res

    @classmethod
    def fromtimestamp(cls, timestamp, tz=None):
        _require_tzinfo(tz)
        if tz is not None:
            return cls._from_utc_timestamp(timestamp, tz)
        if isinstance(timestamp, float) and not _math.isfinite(timestamp):
            raise ValueError("timestamp out of range")
        if isinstance(timestamp, float):
            num, den = timestamp.as_integer_ratio()
            total_us = _round_div_even(num * _US_PER_SEC, den)
        else:
            total_us = int(timestamp) * _US_PER_SEC
        secs, us = divmod(total_us, _US_PER_SEC)
        lt = _time.localtime(secs)
        return cls(lt.tm_year, lt.tm_mon, lt.tm_mday, lt.tm_hour, lt.tm_min, lt.tm_sec, us)

    @classmethod
    def utcfromtimestamp(cls, timestamp):
        return cls._from_utc_timestamp(timestamp, None)

    @classmethod
    def today(cls):
        return cls.fromtimestamp(_time.time())

    @classmethod
    def now(cls, tz=None):
        _require_tzinfo(tz)
        return cls.fromtimestamp(_time.time(), tz)

    @classmethod
    def utcnow(cls):
        return cls.utcfromtimestamp(_time.time())

    @classmethod
    def fromordinal(cls, n):
        n = _require_int(n, "ordinal")
        y, m, d = _ordinal_to_ymd(n)
        return cls(y, m, d)

    @classmethod
    def fromisocalendar(cls, year, week, day):
        year = _require_int(year, "year")
        week = _require_int(week, "week")
        day = _require_int(day, "day")
        y, m, d = _from_iso_calendar(year, week, day)
        return cls(y, m, d)

    @classmethod
    def combine(cls, date_obj, time_obj, tzinfo=True):
        if not isinstance(date_obj, date):
            raise TypeError("date argument must be a date instance")
        if not isinstance(time_obj, time):
            raise TypeError("time argument must be a time instance")
        if tzinfo is True:
            tzinfo = time_obj.tzinfo
        return cls(
            date_obj.year,
            date_obj.month,
            date_obj.day,
            time_obj.hour,
            time_obj.minute,
            time_obj.second,
            time_obj.microsecond,
            tzinfo,
            fold=time_obj.fold,
        )

    @classmethod
    def fromisoformat(cls, date_string):
        if not isinstance(date_string, str):
            raise TypeError("fromisoformat: argument must be str")
        y, mo, d, h, mi, se, us, tz_us = _parse_iso_datetime(date_string)
        return cls(y, mo, d, h, mi, se, us, _tz_from_us(tz_us))

    @classmethod
    def strptime(cls, date_string, format):
        if not isinstance(date_string, str):
            raise TypeError("strptime() argument 1 must be str, not " + type(date_string).__name__)
        if not isinstance(format, str):
            raise TypeError("strptime() argument 2 must be str, not " + type(format).__name__)
        y, mo, d, h, mi, se, us, tz_us, tz_name = _parse_strptime(date_string, format)
        tz = None
        if tz_us is not None:
            if tz_name is not None:
                tz = timezone(timedelta(microseconds=tz_us), tz_name)
            else:
                tz = _tz_from_us(tz_us)
        return cls(y, mo, d, h, mi, se, us, tz)

    def date(self):
        return date(self._year, self._month, self._day)

    def time(self):
        return time(self._hour, self._minute, self._second, self._microsecond, fold=self._fold)

    def timetz(self):
        return time(
            self._hour, self._minute, self._second, self._microsecond, self._tzinfo, fold=self._fold
        )

    def replace(
        self,
        year=None,
        month=None,
        day=None,
        hour=None,
        minute=None,
        second=None,
        microsecond=None,
        tzinfo=True,
        *,
        fold=None,
    ):
        if year is None:
            year = self._year
        if month is None:
            month = self._month
        if day is None:
            day = self._day
        if hour is None:
            hour = self._hour
        if minute is None:
            minute = self._minute
        if second is None:
            second = self._second
        if microsecond is None:
            microsecond = self._microsecond
        if tzinfo is True:
            tzinfo = self._tzinfo
        if fold is None:
            fold = self._fold
        return type(self)(
            year, month, day, hour, minute, second, microsecond, tzinfo, fold=fold
        )

    def utcoffset(self):
        if self._tzinfo is None:
            return None
        return _check_offset_bounds("utcoffset", self._tzinfo.utcoffset(self))

    def dst(self):
        if self._tzinfo is None:
            return None
        return _check_offset_bounds("dst", self._tzinfo.dst(self))

    def tzname(self):
        if self._tzinfo is None:
            return None
        name = self._tzinfo.tzname(self)
        if name is not None and not isinstance(name, str):
            raise TypeError("tzinfo.tzname() must return None or string, not '" + type(name).__name__ + "'")
        return name

    def timetuple(self):
        dst_val = self.dst()
        if dst_val is None:
            isdst = -1
        elif dst_val:
            isdst = 1
        else:
            isdst = 0
        return _build_struct_time(
            self._year, self._month, self._day, self._hour, self._minute, self._second, isdst
        )

    def utctimetuple(self):
        off = self.utcoffset()
        if off:
            u = self - off
            return _build_struct_time(u._year, u._month, u._day, u._hour, u._minute, u._second, 0)
        return _build_struct_time(
            self._year, self._month, self._day, self._hour, self._minute, self._second, 0
        )

    def timestamp(self):
        if self._tzinfo is None:
            tm = _build_struct_time(
                self._year, self._month, self._day, self._hour, self._minute, self._second, -1
            )
            return _time.mktime(tm) + self._microsecond / _US_PER_SEC
        return (self - _EPOCH).total_seconds()

    def astimezone(self, tz=None):
        _require_tzinfo(tz)
        my_off = self.utcoffset()
        if my_off is None:
            ts = self.timestamp()
            local_tz = _local_tz_for_timestamp(ts)
            if tz is None:
                return self.replace(tzinfo=local_tz)
            utc_dt = (self - local_tz.utcoffset(self)).replace(tzinfo=tz)
            return tz.fromutc(utc_dt)
        if tz is None:
            ts = self.timestamp()
            tz = _local_tz_for_timestamp(ts)
        if tz is self._tzinfo:
            return self
        utc_dt = (self - my_off).replace(tzinfo=tz)
        return tz.fromutc(utc_dt)

    def isoformat(self, sep="T", timespec="auto"):
        if not isinstance(sep, str) or len(sep) != 1:
            raise TypeError("isoformat() argument 1 must be a unicode character, not " + type(sep).__name__)
        d_str = "%04d-%02d-%02d" % (self._year, self._month, self._day)
        t_str = _format_hms_timespec(self._hour, self._minute, self._second, self._microsecond, timespec)
        return d_str + sep + t_str + _fmt_tz_suffix(self.utcoffset())

    def ctime(self):
        return (
            _format_strftime("%a %b ", self._year, self._month, self._day, self._hour, self._minute, self._second, 0, None, None)
            + "%2d %02d:%02d:%02d %04d" % (self._day, self._hour, self._minute, self._second, self._year)
        )

    def strftime(self, fmt):
        if not isinstance(fmt, str):
            raise TypeError("strftime() argument 1 must be str, not " + type(fmt).__name__)
        off = self.utcoffset()
        off_us = None if off is None else off._to_us()
        return _format_strftime(
            fmt,
            self._year,
            self._month,
            self._day,
            self._hour,
            self._minute,
            self._second,
            self._microsecond,
            off_us,
            self.tzname(),
        )

    def __str__(self):
        return self.isoformat(" ")

    def __repr__(self):
        parts = [self._year, self._month, self._day, self._hour, self._minute]
        if self._microsecond != 0:
            parts.append(self._second)
            parts.append(self._microsecond)
        elif self._second != 0:
            parts.append(self._second)
        s = _cls_repr_prefix(type(self)) + "(" + ", ".join(str(p) for p in parts)
        if self._fold:
            s += ", fold=1"
        if self._tzinfo is not None:
            s += ", tzinfo=" + repr(self._tzinfo)
        return s + ")"

    def _abs_us(self):
        ord_val = self.toordinal()
        return ((ord_val * 86400 + self._hour * 3600 + self._minute * 60 + self._second) * _US_PER_SEC) + self._microsecond

    def __add__(self, other):
        if not isinstance(other, timedelta):
            return NotImplemented
        total_us = self._abs_us() + other._to_us()
        days, rem_us = divmod(total_us, _US_PER_DAY)
        if not (1 <= days <= 3652059):
            raise OverflowError("result out of range")
        secs, us = divmod(rem_us, _US_PER_SEC)
        hh, rem_s = divmod(secs, 3600)
        mm, ss = divmod(rem_s, 60)
        y, mo, d = _ordinal_to_ymd(days)
        return type(self)(y, mo, d, hh, mm, ss, us, self._tzinfo)

    def __radd__(self, other):
        return self.__add__(other)

    def __sub__(self, other):
        if isinstance(other, timedelta):
            return self + (-other)
        if not isinstance(other, datetime):
            return NotImplemented
        diff_us = self._abs_us() - other._abs_us()
        if self._tzinfo is other._tzinfo:
            return timedelta._from_us(diff_us)
        my_off = self.utcoffset()
        ot_off = other.utcoffset()
        if (my_off is None) != (ot_off is None):
            raise TypeError("can't subtract offset-naive and offset-aware datetimes")
        if my_off is not None:
            diff_us -= my_off._to_us() - ot_off._to_us()
        return timedelta._from_us(diff_us)

    def _cmp_key(self, other, allow_mixed):
        if self._tzinfo is other._tzinfo:
            return self._abs_us(), other._abs_us()
        my_off = self.utcoffset()
        ot_off = other.utcoffset()
        if (my_off is None) != (ot_off is None):
            if allow_mixed:
                return None, None
            raise TypeError("can't compare offset-naive and offset-aware datetimes")
        if my_off is None or my_off == ot_off:
            return self._abs_us(), other._abs_us()
        return self._abs_us() - my_off._to_us(), other._abs_us() - ot_off._to_us()

    def __eq__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_key(other, True)
        if a is None:
            return False
        return a == b

    def __ne__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_key(other, True)
        if a is None:
            return True
        return a != b

    def __lt__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_key(other, False)
        return a < b

    def __le__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_key(other, False)
        return a <= b

    def __gt__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_key(other, False)
        return a > b

    def __ge__(self, other):
        if not isinstance(other, datetime):
            return NotImplemented
        a, b = self._cmp_key(other, False)
        return a >= b

    def __hash__(self):
        dt_obj = self.replace(fold=0) if self._fold else self
        off = dt_obj.utcoffset()
        if off is None:
            return hash(("naive_dt", dt_obj._abs_us()))
        return hash(("aware_dt", dt_obj._abs_us() - off._to_us()))

    def __reduce__(self):
        return self.__reduce_ex__(2)

    def __reduce_ex__(self, protocol):
        # The fold bit is only part of the pickled state from protocol 4 on.
        y = self._year
        us = self._microsecond
        data = bytes(
            (
                y >> 8,
                y & 0xFF,
                self._month | (self._fold << 7 if protocol > 3 else 0),
                self._day,
                self._hour,
                self._minute,
                self._second,
                us >> 16,
                (us >> 8) & 0xFF,
                us & 0xFF,
            )
        )
        if self._tzinfo is None:
            return (type(self), (data,))
        return (type(self), (data, self._tzinfo))


datetime.min = datetime(MINYEAR, 1, 1)
datetime.max = datetime(MAXYEAR, 12, 31, 23, 59, 59, 999999)
datetime.resolution = timedelta(microseconds=1)
_EPOCH = datetime(1970, 1, 1, tzinfo=timezone.utc)
