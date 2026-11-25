"""IANA time zone support (PEP 615 / CPython 3.12 parity, issue #3226)."""

import bisect
from collections import OrderedDict
import datetime
import io
import os
import pickle
import struct
import sys
import warnings
import weakref

__all__ = [
    "ZoneInfo",
    "reset_tzpath",
    "available_timezones",
    "TZPATH",
    "ZoneInfoNotFoundError",
    "InvalidTZPathWarning",
]

__package__ = "zoneinfo"
__path__ = []

_EPOCH_ORDINAL = 719163  # datetime.date(1970, 1, 1).toordinal()
_DAYS_BEFORE_MONTH = (0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334)
_DAYS_IN_MONTH = (0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31)
_ZERO_TD = datetime.timedelta(0)


class ZoneInfoNotFoundError(KeyError):
    """Exception raised when a ZoneInfo key cannot be found."""


class InvalidTZPathWarning(RuntimeWarning):
    """Warning raised when an invalid path is found in PYTHONTZPATH."""


# ---------------------------------------------------------------------------
# Built-in IANA timezone rules (POSIX TZ strings) for cross-platform fallback
# when system /usr/share/zoneinfo is unavailable (e.g. Windows without tzdata).
# ---------------------------------------------------------------------------
_BUILTIN_ZONES = {
    "UTC": "UTC0",
    "Etc/UTC": "UTC0",
    "Etc/UCT": "UTC0",
    "Etc/GMT": "GMT0",
    "Etc/GMT+0": "GMT0",
    "Etc/GMT-0": "GMT0",
    "Etc/GMT0": "GMT0",
    "Etc/Greenwich": "GMT0",
    "Etc/Universal": "UTC0",
    "Etc/Zulu": "UTC0",
    "GMT": "GMT0",
    "GMT+0": "GMT0",
    "GMT-0": "GMT0",
    "GMT0": "GMT0",
    "Greenwich": "GMT0",
    "Universal": "UTC0",
    "Zulu": "UTC0",
    "Etc/GMT+1": "<-01>1",
    "Etc/GMT+2": "<-02>2",
    "Etc/GMT+3": "<-03>3",
    "Etc/GMT+4": "<-04>4",
    "Etc/GMT+5": "<-05>5",
    "Etc/GMT+6": "<-06>6",
    "Etc/GMT+7": "<-07>7",
    "Etc/GMT+8": "<-08>8",
    "Etc/GMT+9": "<-09>9",
    "Etc/GMT+10": "<-10>10",
    "Etc/GMT+11": "<-11>11",
    "Etc/GMT+12": "<-12>12",
    "Etc/GMT-1": "<+01>-1",
    "Etc/GMT-2": "<+02>-2",
    "Etc/GMT-3": "<+03>-3",
    "Etc/GMT-4": "<+04>-4",
    "Etc/GMT-5": "<+05>-5",
    "Etc/GMT-6": "<+06>-6",
    "Etc/GMT-7": "<+07>-7",
    "Etc/GMT-8": "<+08>-8",
    "Etc/GMT-9": "<+09>-9",
    "Etc/GMT-10": "<+10>-10",
    "Etc/GMT-11": "<+11>-11",
    "Etc/GMT-12": "<+12>-12",
    "Etc/GMT-13": "<+13>-13",
    "Etc/GMT-14": "<+14>-14",
    # Americas
    "America/New_York": "EST5EDT,M3.2.0,M11.1.0",
    "US/Eastern": "EST5EDT,M3.2.0,M11.1.0",
    "EST5EDT": "EST5EDT,M3.2.0,M11.1.0",
    "EST": "EST5",
    "America/Chicago": "CST6CDT,M3.2.0,M11.1.0",
    "US/Central": "CST6CDT,M3.2.0,M11.1.0",
    "CST6CDT": "CST6CDT,M3.2.0,M11.1.0",
    "America/Denver": "MST7MDT,M3.2.0,M11.1.0",
    "US/Mountain": "MST7MDT,M3.2.0,M11.1.0",
    "MST7MDT": "MST7MDT,M3.2.0,M11.1.0",
    "MST": "MST7",
    "America/Los_Angeles": "PST8PDT,M3.2.0,M11.1.0",
    "US/Pacific": "PST8PDT,M3.2.0,M11.1.0",
    "PST8PDT": "PST8PDT,M3.2.0,M11.1.0",
    "America/Phoenix": "MST7",
    "America/Anchorage": "AKST9AKDT,M3.2.0,M11.1.0",
    "Pacific/Honolulu": "HST10",
    "HST": "HST10",
    "America/Toronto": "EST5EDT,M3.2.0,M11.1.0",
    "America/Vancouver": "PST8PDT,M3.2.0,M11.1.0",
    "America/Halifax": "AST4ADT,M3.2.0,M11.1.0",
    "America/St_Johns": "NST3:30NDT,M3.2.0,M11.1.0",
    "America/Mexico_City": "CST6",
    "America/Bogota": "<-05>5",
    "America/Lima": "<-05>5",
    "America/Santiago": "<-04>4<-03>,M9.1.6/24,M4.1.6/24",
    "America/Sao_Paulo": "<-03>3",
    "America/Argentina/Buenos_Aires": "<-03>3",
    # Europe
    "Europe/London": "GMT0BST,M3.5.0/1,M10.5.0",
    "GB": "GMT0BST,M3.5.0/1,M10.5.0",
    "Europe/Dublin": "IST-1GMT0,M10.5.0,M3.5.0/1",
    "Europe/Lisbon": "WET0WEST,M3.5.0/1,M10.5.0",
    "Europe/Paris": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Berlin": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Amsterdam": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Brussels": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Madrid": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Rome": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Vienna": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Zurich": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Stockholm": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Oslo": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Copenhagen": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Warsaw": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Prague": "CET-1CEST,M3.5.0,M10.5.0/3",
    "CET": "CET-1CEST,M3.5.0,M10.5.0/3",
    "Europe/Athens": "EET-2EEST,M3.5.0/3,M10.5.0/4",
    "Europe/Helsinki": "EET-2EEST,M3.5.0/3,M10.5.0/4",
    "Europe/Bucharest": "EET-2EEST,M3.5.0/3,M10.5.0/4",
    "Europe/Kyiv": "EET-2EEST,M3.5.0/3,M10.5.0/4",
    "Europe/Kiev": "EET-2EEST,M3.5.0/3,M10.5.0/4",
    "Europe/Istanbul": "<+03>-3",
    "Europe/Moscow": "MSK-3",
    # Asia
    "Asia/Tokyo": "JST-9",
    "Japan": "JST-9",
    "Asia/Seoul": "KST-9",
    "Asia/Shanghai": "CST-8",
    "Asia/Chongqing": "CST-8",
    "Asia/Hong_Kong": "HKT-8",
    "Asia/Taipei": "CST-8",
    "Asia/Singapore": "<+08>-8",
    "Asia/Manila": "PST-8",
    "Asia/Kuala_Lumpur": "<+08>-8",
    "Asia/Bangkok": "<+07>-7",
    "Asia/Jakarta": "WIB-7",
    "Asia/Ho_Chi_Minh": "<+07>-7",
    "Asia/Kolkata": "IST-5:30",
    "Asia/Calcutta": "IST-5:30",
    "Asia/Colombo": "<+0530>-5:30",
    "Asia/Kathmandu": "<+0545>-5:45",
    "Asia/Dhaka": "<+06>-6",
    "Asia/Karachi": "PKT-5",
    "Asia/Dubai": "<+04>-4",
    "Asia/Riyadh": "<+03>-3",
    "Asia/Jerusalem": "IST-2IDT,M3.4.4/26,M10.5.0",
    "Asia/Tehran": "<+0330>-3:30",
    # Oceania
    "Australia/Sydney": "AEST-10AEDT,M10.1.0,M4.1.0/3",
    "Australia/Melbourne": "AEST-10AEDT,M10.1.0,M4.1.0/3",
    "Australia/Brisbane": "AEST-10",
    "Australia/Adelaide": "ACST-9:30ACDT,M10.1.0,M4.1.0/3",
    "Australia/Darwin": "ACST-9:30",
    "Australia/Perth": "AWST-8",
    "Pacific/Auckland": "NZST-12NZDT,M9.5.0,M4.1.0/3",
    # Africa
    "Africa/Cairo": "EET-2EEST,M4.5.5/0,M10.5.4/24",
    "Africa/Johannesburg": "SAST-2",
    "Africa/Lagos": "WAT-1",
    "Africa/Nairobi": "EAT-3",
    "Africa/Casablanca": "<+01>-1",
}


# ---------------------------------------------------------------------------
# TZPATH management
# ---------------------------------------------------------------------------

_DEFAULT_UNIX_TZPATH = (
    "/usr/share/zoneinfo",
    "/usr/lib/zoneinfo",
    "/usr/share/lib/zoneinfo",
    "/etc/zoneinfo",
)

TZPATH = ()
_tzpath_is_default = True


def _sync_tzpath():
    mod = sys.modules.get("zoneinfo")
    if mod is not None:
        mod.TZPATH = TZPATH


def _parse_tzpath_env(env_var):
    paths = []
    invalid = []
    for raw in env_var.split(os.pathsep):
        if not raw:
            continue
        if os.path.isabs(raw):
            paths.append(raw)
        else:
            invalid.append(raw)
    if invalid:
        warnings.warn(
            "Invalid paths specified in PYTHONTZPATH environment variable. "
            "Paths should be absolute but found the following relative paths:\n"
            f"    {os.pathsep.join(invalid)}",
            InvalidTZPathWarning,
        )
    return tuple(paths)


def reset_tzpath(to=None):
    """Reset the global TZPATH search path."""
    global TZPATH, _tzpath_is_default

    if to is not None:
        if isinstance(to, (str, bytes)):
            raise TypeError(
                f"tzpath type must be a sequence of paths, not {type(to).__name__}."
            )
        base_tzpath = []
        invalid = []
        for path in to:
            p = os.fspath(path)
            if not isinstance(p, str):
                raise TypeError(
                    f"tzpath entries must be str or PathLike[str], not {type(p).__name__}"
                )
            if not os.path.isabs(p):
                invalid.append(p)
            else:
                base_tzpath.append(p)
        if invalid:
            raise ValueError(
                f"zoneinfo paths must be absolute, got: {invalid}"
            )
        TZPATH = tuple(base_tzpath)
        _tzpath_is_default = False
    else:
        env_var = os.environ.get("PYTHONTZPATH")
        if env_var is not None:
            TZPATH = _parse_tzpath_env(env_var)
        elif sys.platform == "win32":
            TZPATH = ()
        else:
            TZPATH = _DEFAULT_UNIX_TZPATH
        _tzpath_is_default = True

    _sync_tzpath()


# Initialize TZPATH on module import
reset_tzpath()


def _posix_normpath(path):
    """Normalize a POSIX-style relative path (matching posixpath.normpath)."""
    if path == "":
        return "."
    initial_slashes = path.startswith("/")
    if initial_slashes and path.startswith("//") and not path.startswith("///"):
        initial_slashes = 2
    comps = path.split("/")
    new_comps = []
    for comp in comps:
        if comp in ("", "."):
            continue
        if (
            comp != ".."
            or (not initial_slashes and not new_comps)
            or (new_comps and new_comps[-1] == "..")
        ):
            new_comps.append(comp)
        elif new_comps:
            new_comps.pop()
    result = "/".join(new_comps)
    if initial_slashes:
        result = "/" * initial_slashes + result
    return result or "."


def _validate_tzfile_path(path, _base=""):
    if not isinstance(path, str):
        raise TypeError(
            f"ZoneInfo keys must be str, not {type(path).__name__}"
        )
    if "\x00" in path:
        raise ValueError(
            f"ZoneInfo keys may not contain NUL bytes, got: {path!r}"
        )
    if (
        path.startswith("/")
        or path.startswith("\\")
        or os.path.isabs(path)
        or os.path.splitdrive(path)[0]
    ):
        raise ValueError(
            f"ZoneInfo keys may not be absolute paths, got: {path}"
        )

    # Check for upward traversal before normalization mismatch
    if path == ".." or path.startswith("../") or path.startswith("..\\"):
        raise ValueError(
            f"ZoneInfo keys must refer to subdirectories of TZPATH, got: {path}"
        )

    # Reject backslashes or non-normalized paths (e.g. "", ".", "a/./b", "a/../b", "a//b")
    if "\\" in path:
        raise ValueError(
            f"ZoneInfo keys must be normalized relative paths, got: {path}"
        )

    norm = _posix_normpath(path)
    if norm == ".." or norm.startswith("../"):
        raise ValueError(
            f"ZoneInfo keys must refer to subdirectories of TZPATH, got: {path}"
        )
    if norm != path or path in ("", "."):
        raise ValueError(
            f"ZoneInfo keys must be normalized relative paths, got: {path}"
        )


def _find_tzfile(key):
    """Locate a TZif file on TZPATH; returns an open binary file or None."""
    _validate_tzfile_path(key)
    mod = sys.modules.get("zoneinfo")
    search_paths = mod.TZPATH if mod is not None else TZPATH
    parts = key.split("/")
    for base in search_paths:
        fpath = os.path.join(base, *parts)
        if os.path.isfile(fpath):
            return open(fpath, "rb")
    return None


def available_timezones():
    """Return a set of all valid IANA timezone keys available on TZPATH."""
    valid_zones = set()
    mod = sys.modules.get("zoneinfo")
    search_paths = mod.TZPATH if mod is not None else TZPATH

    skip_top_dirs = {"posix", "right"}
    skip_names = {"posixrules"}

    for tz_root in search_paths:
        if not os.path.isdir(tz_root):
            continue
        for root, dirs, files in os.walk(tz_root):
            if root == tz_root:
                dirs[:] = [d for d in dirs if d not in skip_top_dirs]
            for fname in files:
                fpath = os.path.join(root, fname)
                rel = os.path.relpath(fpath, tz_root)
                if os.sep != "/":
                    rel = rel.replace(os.sep, "/")
                if rel in skip_names:
                    continue
                top = rel.split("/", 1)[0]
                if top in skip_top_dirs:
                    continue
                try:
                    with open(fpath, "rb") as f:
                        magic = f.read(4)
                except OSError:
                    continue
                if magic == b"TZif":
                    valid_zones.add(rel)

    if not valid_zones and not search_paths and _tzpath_is_default:
        return set(_BUILTIN_ZONES.keys())

    return valid_zones


# ---------------------------------------------------------------------------
# Calendar & POSIX TZ string helpers
# ---------------------------------------------------------------------------


def _is_leap(year):
    return year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)


def _days_before_year(year):
    y = year - 1
    return y * 365 + y // 4 - y // 100 + y // 400


def _ymd_to_ordinal(year, month, day):
    d = _days_before_year(year) + _DAYS_BEFORE_MONTH[month] + day
    if month > 2 and _is_leap(year):
        d += 1
    return d


def _dt_to_epoch_secs(dt):
    return (
        (dt.toordinal() - _EPOCH_ORDINAL) * 86400
        + dt.hour * 3600
        + dt.minute * 60
        + dt.second
    )


class _Ttinfo:
    __slots__ = ("utoff", "dstoff", "utoff_td", "dstoff_td", "tzname")

    def __init__(self, utoff, dstoff, tzname):
        self.utoff = utoff
        self.dstoff = dstoff
        self.utoff_td = datetime.timedelta(seconds=utoff)
        self.dstoff_td = (
            _ZERO_TD if dstoff == 0 else datetime.timedelta(seconds=dstoff)
        )
        self.tzname = tzname


class _CalendarRule:
    """One POSIX TZ transition date/time specification."""

    __slots__ = ("kind", "a", "b", "c", "time_secs")

    def __init__(self, kind, a, b=0, c=0, time_secs=7200):
        self.kind = kind  # "M", "J", "n"
        self.a = a
        self.b = b
        self.c = c
        self.time_secs = time_secs

    def year_to_epoch(self, year):
        """Return wall-clock seconds since 1970-01-01 00:00:00 for this rule in `year`."""
        if self.kind == "M":
            month, week, weekday = self.a, self.b, self.c
            # weekday: 0=Sunday .. 6=Saturday
            # ordinal % 7: 0=Sunday, 1=Monday .. 6=Saturday
            first_ord = _ymd_to_ordinal(year, month, 1)
            first_wday = first_ord % 7
            day = 1 + (weekday - first_wday) % 7 + (week - 1) * 7
            dim = _DAYS_IN_MONTH[month] + (
                1 if month == 2 and _is_leap(year) else 0
            )
            if day > dim:
                day -= 7
            day_ord = first_ord + (day - 1)
        elif self.kind == "J":
            # 1..365 matching CPython _zoneinfo._DayOffset
            days_before_year = _days_before_year(year) - _EPOCH_ORDINAL
            jday = self.a
            if jday >= 59 and _is_leap(year):
                jday += 1
            return (days_before_year + jday) * 86400 + self.time_secs
        else:
            # 0..365 matching CPython _zoneinfo._DayOffset
            days_before_year = _days_before_year(year) - _EPOCH_ORDINAL
            return (days_before_year + self.a) * 86400 + self.time_secs

        return (day_ord - _EPOCH_ORDINAL) * 86400 + self.time_secs


class _TzRule:
    """Parsed POSIX TZ footer rule (fixed or annual DST)."""

    __slots__ = ("std", "dst", "start", "end")

    def __init__(self, std, dst=None, start=None, end=None):
        self.std = std
        self.dst = dst
        self.start = start
        self.end = end

    def transitions_for_year(self, year):
        """Return sorted list of (utc_epoch, ttinfo_after) for `year`."""
        if self.dst is None:
            return []
        t_start = self.start.year_to_epoch(year) - self.std.utoff
        t_end = self.end.year_to_epoch(year) - self.dst.utoff
        if t_start <= t_end:
            return [(t_start, self.dst), (t_end, self.std)]
        return [(t_end, self.std), (t_start, self.dst)]

    def ttinfo_before_first(self, year):
        if self.dst is None:
            return self.std
        t_start = self.start.year_to_epoch(year) - self.std.utoff
        t_end = self.end.year_to_epoch(year) - self.dst.utoff
        return self.std if t_start <= t_end else self.dst


def _parse_tz_abbr(s, pos):
    n = len(s)
    if pos >= n:
        raise ValueError(f"Invalid TZ string (missing abbreviation): {s!r}")
    if s[pos] == "<":
        end = s.find(">", pos + 1)
        if end < 0 or end == pos + 1:
            raise ValueError(f"Invalid quoted abbreviation in TZ string: {s!r}")
        return s[pos + 1 : end], end + 1
    start = pos
    while pos < n and s[pos] not in "+-0123456789,/":
        pos += 1
    if pos - start < 3:
        raise ValueError(f"Invalid abbreviation in TZ string: {s!r}")
    return s[start:pos], pos


def _parse_tz_hms(s, pos, max_hour=24, allow_negative_hours=False):
    n = len(s)
    if pos >= n:
        raise ValueError(f"Invalid offset in TZ string: {s!r}")
    sign = 1
    if s[pos] == "+":
        pos += 1
    elif s[pos] == "-":
        sign = -1
        pos += 1

    # Hours (1 to 3 digits for extended TZif v3)
    h_start = pos
    while pos < n and s[pos].isdigit():
        pos += 1
    if pos == h_start:
        raise ValueError(f"Invalid hours in TZ string: {s!r}")
    hours = int(s[h_start:pos])
    if hours > max_hour:
        raise ValueError(f"Hours out of range in TZ string: {s!r}")

    minutes = 0
    seconds = 0
    if pos < n and s[pos] == ":":
        pos += 1
        m_start = pos
        while pos < n and s[pos].isdigit():
            pos += 1
        if pos == m_start:
            raise ValueError(f"Invalid minutes in TZ string: {s!r}")
        minutes = int(s[m_start:pos])
        if minutes > 59:
            raise ValueError(f"Minutes out of range in TZ string: {s!r}")
        if pos < n and s[pos] == ":":
            pos += 1
            s_start = pos
            while pos < n and s[pos].isdigit():
                pos += 1
            if pos == s_start:
                raise ValueError(f"Invalid seconds in TZ string: {s!r}")
            seconds = int(s[s_start:pos])
            if seconds > 59:
                raise ValueError(f"Seconds out of range in TZ string: {s!r}")

    total = sign * (hours * 3600 + minutes * 60 + seconds)
    return total, pos


def _parse_tz_rule_date(s, pos):
    n = len(s)
    if pos >= n:
        raise ValueError(f"Invalid rule in TZ string: {s!r}")
    if s[pos] == "M":
        pos += 1
        end = pos
        while end < n and s[end] not in ",/":
            end += 1
        parts = s[pos:end].split(".")
        if len(parts) != 3:
            raise ValueError(f"Invalid M rule in TZ string: {s!r}")
        m, w, d = int(parts[0]), int(parts[1]), int(parts[2])
        if not (1 <= m <= 12 and 1 <= w <= 5 and 0 <= d <= 6):
            raise ValueError(f"M rule out of range in TZ string: {s!r}")
        kind, a, b, c = "M", m, w, d
        pos = end
    elif s[pos] == "J":
        pos += 1
        end = pos
        while end < n and s[end].isdigit():
            end += 1
        if end == pos:
            raise ValueError(f"Invalid J rule in TZ string: {s!r}")
        j = int(s[pos:end])
        if not (1 <= j <= 365):
            raise ValueError(f"J rule out of range in TZ string: {s!r}")
        kind, a, b, c = "J", j, 0, 0
        pos = end
    else:
        end = pos
        while end < n and s[end].isdigit():
            end += 1
        if end == pos:
            raise ValueError(f"Invalid day rule in TZ string: {s!r}")
        day = int(s[pos:end])
        if not (0 <= day <= 365):
            raise ValueError(f"Day rule out of range in TZ string: {s!r}")
        kind, a, b, c = "n", day, 0, 0
        pos = end

    time_secs = 7200  # Default 02:00:00
    if pos < n and s[pos] == "/":
        pos += 1
        time_secs, pos = _parse_tz_hms(
            s, pos, max_hour=167, allow_negative_hours=True
        )

    return _CalendarRule(kind, a, b, c, time_secs), pos


def _parse_tz_string(tz_str):
    """Parse an RFC 8536 / POSIX TZ string into a _TzRule."""
    if not tz_str:
        return None
    n = len(tz_str)
    std_abbr, pos = _parse_tz_abbr(tz_str, 0)
    # POSIX offsets have inverted sign: positive means WEST of UTC.
    std_posix_off, pos = _parse_tz_hms(tz_str, pos, max_hour=24)
    std_utoff = -std_posix_off
    std_tt = _Ttinfo(std_utoff, 0, std_abbr)

    if pos == n:
        return _TzRule(std_tt)

    dst_abbr, pos = _parse_tz_abbr(tz_str, pos)
    if pos < n and tz_str[pos] != ",":
        dst_posix_off, pos = _parse_tz_hms(tz_str, pos, max_hour=24)
        dst_utoff = -dst_posix_off
    else:
        dst_utoff = std_utoff + 3600

    dst_tt = _Ttinfo(dst_utoff, dst_utoff - std_utoff, dst_abbr)

    if pos >= n or tz_str[pos] != ",":
        raise ValueError(f"Missing DST transition rules in TZ string: {tz_str!r}")
    pos += 1
    start_rule, pos = _parse_tz_rule_date(tz_str, pos)
    if pos >= n or tz_str[pos] != ",":
        raise ValueError(f"Missing end transition rule in TZ string: {tz_str!r}")
    pos += 1
    end_rule, pos = _parse_tz_rule_date(tz_str, pos)
    if pos != n:
        raise ValueError(f"Trailing characters in TZ string: {tz_str!r}")

    return _TzRule(std_tt, dst_tt, start_rule, end_rule)


# ---------------------------------------------------------------------------
# Binary TZif v1 / v2 / v3 / v4 parser
# ---------------------------------------------------------------------------


def _read_exact(fobj, n):
    data = fobj.read(n)
    if len(data) != n:
        raise ValueError("Invalid TZif file: unexpected end of file")
    return data


def _read_tzif_header(fobj):
    magic_ver = fobj.read(20)
    if len(magic_ver) < 5 or magic_ver[:4] != b"TZif":
        raise ValueError("Invalid TZif file: magic not found")
    version = magic_ver[4:5]
    if version not in (b"\x00", b"2", b"3", b"4"):
        raise ValueError(f"Unsupported TZif version: {version!r}")
    isutcnt, isstdcnt, leapcnt, timecnt, typecnt, charcnt = struct.unpack(
        ">6I", fobj.read(24)
    )
    return version, isutcnt, isstdcnt, leapcnt, timecnt, typecnt, charcnt


def _load_tzif_data(fobj):
    """Parse a TZif binary stream into transition tables and optional POSIX footer."""
    version, isutcnt, isstdcnt, leapcnt, timecnt, typecnt, charcnt = (
        _read_tzif_header(fobj)
    )

    if version >= b"2":
        # Skip the v1 data block and parse the 64-bit v2+ block instead.
        v1_skip = (
            timecnt * 4
            + timecnt
            + typecnt * 6
            + charcnt
            + leapcnt * 8
            + isstdcnt
            + isutcnt
        )
        _read_exact(fobj, v1_skip)
        (
            version,
            isutcnt,
            isstdcnt,
            leapcnt,
            timecnt,
            typecnt,
            charcnt,
        ) = _read_tzif_header(fobj)
        trans_fmt = f">{timecnt}q"
        trans_bytes = timecnt * 8
        leap_size = 12
    else:
        trans_fmt = f">{timecnt}i"
        trans_bytes = timecnt * 4
        leap_size = 8

    if typecnt == 0:
        raise ValueError("Invalid TZif file: typecnt must be >= 1")

    trans_utc = (
        list(struct.unpack(trans_fmt, _read_exact(fobj, trans_bytes)))
        if timecnt
        else []
    )
    trans_idx = list(_read_exact(fobj, timecnt)) if timecnt else []

    raw_ttinfo = _read_exact(fobj, typecnt * 6)
    abbrevs_raw = _read_exact(fobj, charcnt)

    # Skip leap seconds, std/wall indicators, and UT/local indicators.
    tail_skip = leapcnt * leap_size + isstdcnt + isutcnt
    if tail_skip:
        _read_exact(fobj, tail_skip)

    # Parse raw (utoff, isdst, abbr) tuples
    raw_types = []
    for i in range(typecnt):
        utoff, isdst, abbrind = struct.unpack_from(">iBB", raw_ttinfo, i * 6)
        if abbrind < len(abbrevs_raw):
            end = abbrevs_raw.find(b"\x00", abbrind)
            if end < 0:
                end = len(abbrevs_raw)
            abbr = abbrevs_raw[abbrind:end].decode("ascii", "replace")
        else:
            abbr = ""
        raw_types.append((utoff, bool(isdst), abbr))

    for idx in trans_idx:
        if idx >= typecnt:
            raise ValueError("Invalid TZif file: transition index out of range")

    # Compute dstoff for each ttinfo entry using neighboring non-DST transitions,
    # matching RFC 8536 and CPython 3.12's zoneinfo._common._load_data.
    dstoffs = [0] * typecnt
    if timecnt:
        # Track computed dstoff per transition; also compute per type index
        trans_ttinfos = []
        for i, idx in enumerate(trans_idx):
            utoff, isdst, abbr = raw_types[idx]
            if not isdst:
                dstoff = 0
            else:
                # Search backward for nearest non-DST transition, then forward
                std_utoff = None
                for j in range(i - 1, -1, -1):
                    r_utoff, r_isdst, _ = raw_types[trans_idx[j]]
                    if not r_isdst:
                        std_utoff = r_utoff
                        break
                if std_utoff is None:
                    for j in range(i + 1, timecnt):
                        r_utoff, r_isdst, _ = raw_types[trans_idx[j]]
                        if not r_isdst:
                            std_utoff = r_utoff
                            break
                if std_utoff is None:
                    for r_utoff, r_isdst, _ in raw_types:
                        if not r_isdst:
                            std_utoff = r_utoff
                            break
                dstoff = (utoff - std_utoff) if std_utoff is not None else 3600
            dstoffs[idx] = dstoff
            trans_ttinfos.append(_Ttinfo(utoff, dstoff, abbr))
    else:
        trans_ttinfos = []

    # ttinfo_before: first non-DST ttinfo entry, else raw_types[0]
    ttinfo_before = None
    for idx, (utoff, isdst, abbr) in enumerate(raw_types):
        if not isdst:
            ttinfo_before = _Ttinfo(utoff, 0, abbr)
            break
    if ttinfo_before is None:
        utoff, isdst, abbr = raw_types[0]
        ttinfo_before = _Ttinfo(utoff, 3600 if isdst else 0, abbr)

    # Read optional POSIX TZ footer (v2+)
    tz_rule = None
    if version >= b"2":
        footer = fobj.read()
        if footer:
            if not (footer.startswith(b"\n") and footer.endswith(b"\n")):
                # Tolerate trailing bytes only if newline-delimited footer exists
                nl1 = footer.find(b"\n")
                nl2 = footer.find(b"\n", nl1 + 1) if nl1 >= 0 else -1
                if nl1 != 0 or nl2 < 0:
                    raise ValueError("Invalid TZif file: malformed TZ footer")
                tz_bytes = footer[1:nl2]
            else:
                tz_bytes = footer[1:-1]
            if tz_bytes:
                tz_rule = _parse_tz_string(tz_bytes.decode("ascii"))

    return trans_utc, trans_ttinfos, ttinfo_before, tz_rule


def _build_local_transition_tables(trans_utc, trans_ttinfos, ttinfo_before):
    """Build fold=0 and fold=1 local wall-clock transition timestamp arrays."""
    trans_local_0 = []
    trans_local_1 = []
    prev_off = ttinfo_before.utoff
    for t_utc, tt in zip(trans_utc, trans_ttinfos):
        curr_off = tt.utoff
        t_before = t_utc + prev_off
        t_after = t_utc + curr_off
        if t_before >= t_after:
            # Fall-back (fold) or no-op: fold=0 switches later, fold=1 switches earlier
            trans_local_0.append(t_before)
            trans_local_1.append(t_after)
        else:
            # Spring-forward (gap): fold=0 uses prev through gap, fold=1 uses curr
            trans_local_0.append(t_after)
            trans_local_1.append(t_before)
        prev_off = curr_off
    return trans_local_0, trans_local_1


# ---------------------------------------------------------------------------
# ZoneInfo class
# ---------------------------------------------------------------------------


class ZoneInfo(datetime.tzinfo):
    """Concrete datetime.tzinfo implementation representing an IANA time zone."""

    _strong_cache_size = 8
    _strong_cache = OrderedDict()
    _weak_cache = weakref.WeakValueDictionary()

    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        cls._strong_cache = OrderedDict()
        cls._weak_cache = weakref.WeakValueDictionary()

    def __new__(cls, key):
        _validate_tzfile_path(key)
        instance = cls._weak_cache.get(key)
        if instance is None:
            instance = cls._new_instance(key, from_cache=True)
            cls._weak_cache[key] = instance
        cls._strong_cache_insert(key, instance)
        return instance

    @classmethod
    def _strong_cache_insert(cls, key, instance):
        cache = cls._strong_cache
        if key in cache:
            cache.move_to_end(key)
        else:
            cache[key] = instance
            while len(cache) > cls._strong_cache_size:
                cache.popitem(last=False)

    @classmethod
    def _new_instance(cls, key, from_cache=False):
        fobj = _find_tzfile(key)
        if fobj is not None:
            try:
                data = _load_tzif_data(fobj)
            finally:
                fobj.close()
        else:
            rule_str = _BUILTIN_ZONES.get(key)
            if rule_str is None:
                raise ZoneInfoNotFoundError(
                    f"No time zone found with key {key}"
                )
            rule = _parse_tz_string(rule_str)
            data = ([], [], rule.std, rule)

        obj = cls._from_parsed(data, key=key, file_repr=None, from_file=False)
        obj._from_cache = from_cache
        return obj

    @classmethod
    def _from_parsed(cls, data, key, file_repr, from_file):
        trans_utc, trans_ttinfos, ttinfo_before, tz_rule = data
        obj = object.__new__(cls)
        obj._key = key
        obj._file_repr = file_repr
        obj._from_file = from_file
        obj._from_cache = False
        obj._trans_utc = trans_utc
        obj._trans_ttinfos = trans_ttinfos
        obj._ttinfo_before = ttinfo_before
        obj._tz_rule = tz_rule
        obj._trans_local_0, obj._trans_local_1 = _build_local_transition_tables(
            trans_utc, trans_ttinfos, ttinfo_before
        )
        obj._last_trans_utc = trans_utc[-1] if trans_utc else None
        obj._ttinfo_after_explicit = (
            trans_ttinfos[-1] if trans_ttinfos else ttinfo_before
        )
        return obj

    @classmethod
    def no_cache(cls, key):
        """Construct a fresh ZoneInfo object bypassing the instance cache."""
        _validate_tzfile_path(key)
        return cls._new_instance(key, from_cache=False)

    @classmethod
    def from_file(cls, file_obj, /, key=None):
        """Construct a ZoneInfo object from a binary file-like object."""
        if key is not None and not isinstance(key, str):
            raise TypeError(
                f"key must be str or None, not {type(key).__name__}"
            )
        data = _load_tzif_data(file_obj)
        return cls._from_parsed(
            data, key=key, file_repr=repr(file_obj), from_file=True
        )

    @classmethod
    def clear_cache(cls, *, only_keys=None):
        """Clear the ZoneInfo instance cache."""
        if only_keys is None:
            cls._strong_cache.clear()
            cls._weak_cache.clear()
        else:
            for key in only_keys:
                cls._strong_cache.pop(key, None)
                cls._weak_cache.pop(key, None)

    @classmethod
    def _unpickle(cls, key, from_cache):
        if from_cache:
            return cls(key)
        return cls.no_cache(key)

    def __reduce__(self):
        if self._from_file:
            raise pickle.PicklingError(
                "Cannot pickle a ZoneInfo file from a file stream."
            )
        return (_unpickle_zoneinfo, (type(self), self._key, self._from_cache))

    @property
    def key(self):
        return self._key

    def __repr__(self):
        if self._key is not None:
            return f"zoneinfo.{type(self).__name__}(key={self._key!r})"
        return f"zoneinfo.{type(self).__name__}.from_file({self._file_repr})"

    def __str__(self):
        if self._key is not None:
            return self._key
        return repr(self)

    def _rule_transitions_around(self, year):
        """Generate transitions around `year` from POSIX TZ footer, filtered past explicit table."""
        rule = self._tz_rule
        if rule is None or rule.dst is None:
            return [], [], [], (rule.std if rule is not None else self._ttinfo_after_explicit)

        years = [y for y in (year - 1, year, year + 1) if 1 <= y <= 9999]
        trans_utc = []
        trans_tt = []
        for y in years:
            for t_u, tt in rule.transitions_for_year(y):
                trans_utc.append(t_u)
                trans_tt.append(tt)

        first_year = years[0]
        tt_before = rule.ttinfo_before_first(first_year)

        # If explicit TZif transitions exist, ignore any POSIX rule transitions
        # at or before the last explicit transition.
        if self._last_trans_utc is not None:
            last_u = self._last_trans_utc
            filtered_u = []
            filtered_tt = []
            for t_u, tt in zip(trans_utc, trans_tt):
                if t_u > last_u:
                    filtered_u.append(t_u)
                    filtered_tt.append(tt)
            trans_utc = filtered_u
            trans_tt = filtered_tt
            tt_before = self._ttinfo_after_explicit

        t_loc_0, t_loc_1 = _build_local_transition_tables(
            trans_utc, trans_tt, tt_before
        )
        return trans_utc, trans_tt, (t_loc_0, t_loc_1), tt_before

    def _find_trans(self, dt):
        ts_local = _dt_to_epoch_secs(dt)
        fold = dt.fold
        trans_local = self._trans_local_1 if fold else self._trans_local_0
        n = len(trans_local)

        if n > 0:
            idx = bisect.bisect_right(trans_local, ts_local)
            if idx < n:
                return (
                    self._ttinfo_before
                    if idx == 0
                    else self._trans_ttinfos[idx - 1]
                )
            # Past the last explicit transition: consult POSIX TZ rule if present
            if self._tz_rule is None:
                return self._trans_ttinfos[-1]
            if self._tz_rule.dst is None:
                return self._tz_rule.std
        else:
            if self._tz_rule is None:
                return self._ttinfo_before
            if self._tz_rule.dst is None:
                return self._tz_rule.std

        # Evaluate POSIX TZ rule transitions around dt.year
        _, rule_tt, (r_loc_0, r_loc_1), tt_before = (
            self._rule_transitions_around(dt.year)
        )
        r_loc = r_loc_1 if fold else r_loc_0
        idx = bisect.bisect_right(r_loc, ts_local)
        if idx == 0:
            return tt_before
        return rule_tt[idx - 1]

    def utcoffset(self, dt):
        if dt is None:
            return None
        return self._find_trans(dt).utoff_td

    def dst(self, dt):
        if dt is None:
            return None
        return self._find_trans(dt).dstoff_td

    def tzname(self, dt):
        if dt is None:
            return None
        return self._find_trans(dt).tzname

    def fromutc(self, dt):
        if not isinstance(dt, datetime.datetime):
            raise TypeError("fromutc() requires a datetime argument")
        if dt.tzinfo is not self:
            raise ValueError("dt.tzinfo is not self")

        ts_utc = _dt_to_epoch_secs(dt)
        trans_utc = self._trans_utc
        n = len(trans_utc)
        fold = 0

        if n > 0 and (self._tz_rule is None or ts_utc < trans_utc[-1]):
            idx = bisect.bisect_right(trans_utc, ts_utc)
            tt = (
                self._ttinfo_before
                if idx == 0
                else self._trans_ttinfos[idx - 1]
            )
            if idx > 0:
                prev_tt = (
                    self._ttinfo_before
                    if idx == 1
                    else self._trans_ttinfos[idx - 2]
                )
                if (
                    tt.utoff < prev_tt.utoff
                    and (ts_utc - trans_utc[idx - 1])
                    < (prev_tt.utoff - tt.utoff)
                ):
                    fold = 1
        elif self._tz_rule is None:
            tt = self._ttinfo_before
        elif self._tz_rule.dst is None:
            tt = self._tz_rule.std
            if n > 0:
                prev_tt = self._trans_ttinfos[-1]
                if (
                    tt.utoff < prev_tt.utoff
                    and (ts_utc - trans_utc[-1]) < (prev_tt.utoff - tt.utoff)
                ):
                    fold = 1
        else:
            r_utc, r_tt, _, tt_before = self._rule_transitions_around(dt.year)
            idx = bisect.bisect_right(r_utc, ts_utc)
            if idx == 0:
                tt = tt_before
                # Also check fold against the last explicit transition if we are right after it
                if n > 0:
                    prev_tt = (
                        self._ttinfo_before
                        if n == 1
                        else self._trans_ttinfos[-2]
                    )
                    if (
                        tt.utoff < prev_tt.utoff
                        and (ts_utc - trans_utc[-1])
                        < (prev_tt.utoff - tt.utoff)
                    ):
                        fold = 1
            else:
                tt = r_tt[idx - 1]
                prev_tt = tt_before if idx == 1 else r_tt[idx - 2]
                if (
                    tt.utoff < prev_tt.utoff
                    and (ts_utc - r_utc[idx - 1]) < (prev_tt.utoff - tt.utoff)
                ):
                    fold = 1

        res = dt + tt.utoff_td
        if fold:
            res = res.replace(fold=1)
        return res


def _unpickle_zoneinfo(cls, key, from_cache):
    return cls._unpickle(key, from_cache)


_unpickle_zoneinfo.__module__ = "zoneinfo"
