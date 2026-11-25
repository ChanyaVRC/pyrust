import datetime
import io
import os
import pickle
import struct
import tempfile
import warnings
import zoneinfo
from zoneinfo import (
    InvalidTZPathWarning,
    ZoneInfo,
    ZoneInfoNotFoundError,
    available_timezones,
    reset_tzpath,
)


def build_tzif_v1(transitions, ttinfos, abbrevs):
    """Build a valid TZif v1 binary payload."""
    # ttinfos: list of (utoff, isdst, abbrind)
    # transitions: list of (utc_epoch_i32, type_idx)
    timecnt = len(transitions)
    typecnt = len(ttinfos)
    charcnt = len(abbrevs)
    hdr = b"TZif\x00" + b"\x00" * 15 + struct.pack(
        ">6I", typecnt, typecnt, 0, timecnt, typecnt, charcnt
    )
    body = b""
    if timecnt:
        body += struct.pack(f">{timecnt}i", *(t for t, _ in transitions))
        body += bytes(idx for _, idx in transitions)
    for utoff, isdst, abbrind in ttinfos:
        body += struct.pack(">iBB", utoff, isdst, abbrind)
    body += abbrevs
    body += b"\x00" * (typecnt * 2)
    return hdr + body


def build_tzif_v2(transitions, ttinfos, abbrevs, posix_tz=b"", version=b"2"):
    """Build a valid TZif v2/v3 binary payload."""
    timecnt = len(transitions)
    typecnt = len(ttinfos)
    charcnt = len(abbrevs)

    # v1 block (32-bit transitions; keep empty for simplicity when v2 carries all)
    hdr1 = b"TZif" + version + b"\x00" * 15 + struct.pack(
        ">6I", typecnt, typecnt, 0, 0, typecnt, charcnt
    )
    body1 = b""
    for utoff, isdst, abbrind in ttinfos:
        body1 += struct.pack(">iBB", utoff, isdst, abbrind)
    body1 += abbrevs + b"\x00" * (typecnt * 2)

    # v2+ block (64-bit transitions)
    hdr2 = b"TZif" + version + b"\x00" * 15 + struct.pack(
        ">6I", typecnt, typecnt, 0, timecnt, typecnt, charcnt
    )
    body2 = b""
    if timecnt:
        body2 += struct.pack(f">{timecnt}q", *(t for t, _ in transitions))
        body2 += bytes(idx for _, idx in transitions)
    for utoff, isdst, abbrind in ttinfos:
        body2 += struct.pack(">iBB", utoff, isdst, abbrind)
    body2 += abbrevs + b"\x00" * (typecnt * 2)
    footer = b"\n" + posix_tz + b"\n"
    return hdr1 + body1 + hdr2 + body2 + footer


# 1. Module exports & hierarchy
print("all:", sorted(zoneinfo.__all__))
print("is_pkg:", hasattr(zoneinfo, "__path__"), zoneinfo.__package__)
print("ZoneInfo is tzinfo:", issubclass(ZoneInfo, datetime.tzinfo))
print("NotFound is KeyError:", issubclass(ZoneInfoNotFoundError, KeyError))
print("InvalidWarn is RuntimeWarning:", issubclass(InvalidTZPathWarning, RuntimeWarning))

# 2. TZif v1 explicit transitions (spring-forward + fall-back in 2020)
# 2020-03-08 07:00:00 UTC (EST -> EDT: 02:00 -> 03:00) = 1583650800
# 2020-11-01 06:00:00 UTC (EDT -> EST: 02:00 -> 01:00) = 1604210400
v1_data = build_tzif_v1(
    [(1583650800, 1), (1604210400, 0)],
    [(-18000, 0, 0), (-14400, 1, 4)],
    b"EST\x00EDT\x00",
)
tz_v1 = ZoneInfo.from_file(io.BytesIO(v1_data), key="Test/V1")
print("v1 key/str/repr:", tz_v1.key, str(tz_v1), repr(tz_v1))

for label, dt in [
    ("before_spring", datetime.datetime(2020, 3, 8, 1, 30, tzinfo=tz_v1)),
    ("gap_f0", datetime.datetime(2020, 3, 8, 2, 30, tzinfo=tz_v1, fold=0)),
    ("gap_f1", datetime.datetime(2020, 3, 8, 2, 30, tzinfo=tz_v1, fold=1)),
    ("after_spring", datetime.datetime(2020, 3, 8, 3, 30, tzinfo=tz_v1)),
    ("summer", datetime.datetime(2020, 7, 1, 12, 0, tzinfo=tz_v1)),
    ("ambig_f0", datetime.datetime(2020, 11, 1, 1, 30, tzinfo=tz_v1, fold=0)),
    ("ambig_f1", datetime.datetime(2020, 11, 1, 1, 30, tzinfo=tz_v1, fold=1)),
    ("after_fall", datetime.datetime(2020, 11, 1, 2, 30, tzinfo=tz_v1)),
]:
    print(
        "v1",
        label,
        dt.utcoffset(),
        dt.dst(),
        dt.tzname(),
        dt.isoformat(),
    )

# fromutc around v1 fall-back
for h in (4, 5, 6, 7):
    u = datetime.datetime(2020, 11, 1, h, 30, tzinfo=datetime.timezone.utc)
    loc = u.astimezone(tz_v1)
    print("v1 fromutc", h, loc.isoformat(), loc.fold, loc.tzname())

# 3. TZif v2 with explicit transitions + POSIX TZ footer (EST5EDT,M3.2.0,M11.1.0)
v2_ny_data = build_tzif_v2(
    [(1583650800, 1), (1604210400, 0)],
    [(-18000, 0, 0), (-14400, 1, 4)],
    b"EST\x00EDT\x00",
    posix_tz=b"EST5EDT,M3.2.0,M11.1.0",
    version=b"2",
)
tz_ny = ZoneInfo.from_file(io.BytesIO(v2_ny_data), key="Test/NY")

# Future dates evaluated via POSIX TZ footer (2024, 2035)
for label, dt in [
    ("2024_winter", datetime.datetime(2024, 1, 15, 12, 0, tzinfo=tz_ny)),
    ("2024_gap_f0", datetime.datetime(2024, 3, 10, 2, 30, tzinfo=tz_ny, fold=0)),
    ("2024_gap_f1", datetime.datetime(2024, 3, 10, 2, 30, tzinfo=tz_ny, fold=1)),
    ("2024_summer", datetime.datetime(2024, 7, 4, 12, 0, tzinfo=tz_ny)),
    ("2024_ambig_f0", datetime.datetime(2024, 11, 3, 1, 30, tzinfo=tz_ny, fold=0)),
    ("2024_ambig_f1", datetime.datetime(2024, 11, 3, 1, 30, tzinfo=tz_ny, fold=1)),
    ("2035_summer", datetime.datetime(2035, 6, 15, 10, 0, tzinfo=tz_ny)),
]:
    print("v2_ny", label, dt.utcoffset(), dt.dst(), dt.tzname(), dt.isoformat())

for h in (4, 5, 6, 7):
    u = datetime.datetime(2024, 11, 3, h, 30, tzinfo=datetime.timezone.utc)
    loc = u.astimezone(tz_ny)
    roundtrip = datetime.datetime.fromtimestamp(loc.timestamp(), tz_ny)
    print("v2_ny fromutc", h, loc.isoformat(), loc.fold, loc.tzname(), roundtrip == loc, roundtrip.fold == loc.fold)

# 4. TZif v3: Southern Hemisphere (Australia/Sydney), quoted abbrevs, extended hours, J/n rules
v3_syd_data = build_tzif_v2(
    [],
    [(36000, 0, 0)],
    b"AEST\x00",
    posix_tz=b"AEST-10AEDT,M10.1.0,M4.1.0/3",
    version=b"3",
)
tz_syd = ZoneInfo.from_file(io.BytesIO(v3_syd_data), key="Test/Sydney")
for label, dt in [
    ("jan_dst", datetime.datetime(2024, 1, 15, 12, 0, tzinfo=tz_syd)),
    ("apr_ambig_f0", datetime.datetime(2024, 4, 7, 2, 30, tzinfo=tz_syd, fold=0)),
    ("apr_ambig_f1", datetime.datetime(2024, 4, 7, 2, 30, tzinfo=tz_syd, fold=1)),
    ("jul_std", datetime.datetime(2024, 7, 15, 12, 0, tzinfo=tz_syd)),
    ("oct_gap_f0", datetime.datetime(2024, 10, 6, 2, 30, tzinfo=tz_syd, fold=0)),
    ("oct_gap_f1", datetime.datetime(2024, 10, 6, 2, 30, tzinfo=tz_syd, fold=1)),
    ("dec_dst", datetime.datetime(2024, 12, 25, 12, 0, tzinfo=tz_syd)),
]:
    print("syd", label, dt.utcoffset(), dt.dst(), dt.tzname())

# Quoted abbreviation + half-hour offset (<+0530>-5:30)
v3_ist_data = build_tzif_v2(
    [],
    [(19800, 0, 0)],
    b"+0530\x00",
    posix_tz=b"<+0530>-5:30",
    version=b"3",
)
tz_ist = ZoneInfo.from_file(io.BytesIO(v3_ist_data), key="Test/IST")
dt_ist = datetime.datetime(2024, 6, 1, 12, 0, tzinfo=tz_ist)
print("ist:", dt_ist.utcoffset(), dt_ist.dst(), dt_ist.tzname())

# TZif v3 extended transition time (M3.4.4/26 -> Friday 02:00)
v3_ext_data = build_tzif_v2(
    [],
    [(7200, 0, 0)],
    b"IST\x00",
    posix_tz=b"IST-2IDT,M3.4.4/26,M10.5.0",
    version=b"3",
)
tz_ext = ZoneInfo.from_file(io.BytesIO(v3_ext_data), key="Test/Israel")
for d in (
    datetime.datetime(2024, 3, 29, 1, 30, tzinfo=tz_ext),
    datetime.datetime(2024, 3, 29, 3, 30, tzinfo=tz_ext),
    datetime.datetime(2024, 10, 27, 1, 30, tzinfo=tz_ext, fold=0),
    datetime.datetime(2024, 10, 27, 1, 30, tzinfo=tz_ext, fold=1),
):
    print("v3_ext:", d.isoformat(), d.dst(), d.tzname())

# J-day and 0-based n-day rules
v3_j_data = build_tzif_v2(
    [], [(0, 0, 0)], b"STD\x00", posix_tz=b"STD0DST,J60/2,J300/2", version=b"3"
)
tz_j = ZoneInfo.from_file(io.BytesIO(v3_j_data), key="Test/JRule")
for d in (
    datetime.datetime(2024, 2, 29, 12, 0, tzinfo=tz_j),
    datetime.datetime(2024, 3, 1, 1, 0, tzinfo=tz_j),
    datetime.datetime(2024, 3, 1, 4, 0, tzinfo=tz_j),
    datetime.datetime(2024, 10, 27, 12, 0, tzinfo=tz_j),
):
    print("jrule:", d.isoformat(), d.dst(), d.tzname())

v3_n_data = build_tzif_v2(
    [], [(0, 0, 0)], b"STD\x00", posix_tz=b"STD0DST,59/2,300/2", version=b"3"
)
tz_n = ZoneInfo.from_file(io.BytesIO(v3_n_data), key="Test/NRule")
for d in (
    datetime.datetime(2024, 2, 29, 1, 0, tzinfo=tz_n),
    datetime.datetime(2024, 2, 29, 4, 0, tzinfo=tz_n),
):
    print("nrule:", d.isoformat(), d.dst(), d.tzname())

# 5. None / invalid args for utcoffset / dst / tzname, invalid TZif bytes
print("none queries:", tz_ny.utcoffset(None), tz_ny.dst(None), tz_ny.tzname(None))
for method in ("utcoffset", "dst", "tzname"):
    try:
        getattr(tz_ny, method)(123)
    except AttributeError:
        print(method, "bad arg -> AttributeError")

for bad_buf in (b"", b"NOTZ"):
    try:
        ZoneInfo.from_file(io.BytesIO(bad_buf))
    except ValueError:
        print("bad tzif -> ValueError")
try:
    ZoneInfo.from_file(io.BytesIO(b"TZif2" + b"\x00" * 10))
except (ValueError, struct.error) as e:
    print("truncated tzif ->", type(e).__name__)

# from_file without key & pickle rejection
tz_nokey = ZoneInfo.from_file(io.BytesIO(v3_ist_data))
print("nokey key:", tz_nokey.key, "from_file in repr:", "from_file(" in repr(tz_nokey))
for z in (tz_nokey, tz_ny):
    try:
        pickle.dumps(z)
    except pickle.PicklingError:
        print("pickle from_file -> PicklingError")

# 6. Key validation
for bad_key in ("", "/UTC", "../UTC", "America/../UTC", "UTC/..", "a/./b", 123):
    try:
        ZoneInfo(bad_key)
    except (ValueError, TypeError) as e:
        print("bad_key", repr(bad_key), "->", type(e).__name__)

# 7. TZPATH, reset_tzpath, available_timezones, caching, no_cache, clear_cache, pickle
orig_env = os.environ.get("PYTHONTZPATH")
try:
    with tempfile.TemporaryDirectory() as tmpdir:
        test_dir = os.path.join(tmpdir, "Test")
        posix_dir = os.path.join(tmpdir, "posix")
        os.makedirs(test_dir)
        os.makedirs(posix_dir)
        with open(os.path.join(test_dir, "Alpha"), "wb") as f:
            f.write(v2_ny_data)
        with open(os.path.join(test_dir, "Beta"), "wb") as f:
            f.write(v3_syd_data)
        with open(os.path.join(test_dir, "NotTZif"), "wb") as f:
            f.write(b"hello world")
        with open(os.path.join(posix_dir, "Ignored"), "wb") as f:
            f.write(v3_ist_data)
        with open(os.path.join(tmpdir, "localtime"), "wb") as f:
            f.write(v3_ist_data)

        reset_tzpath([tmpdir])
        print("TZPATH set:", zoneinfo.TZPATH == (tmpdir,))
        print("available:", sorted(available_timezones()))

        a1 = ZoneInfo("Test/Alpha")
        a2 = ZoneInfo("Test/Alpha")
        b1 = ZoneInfo("Test/Beta")
        a_nc = ZoneInfo.no_cache("Test/Alpha")
        print("cache hit:", a1 is a2)
        print("no_cache fresh:", a_nc is not a1, a_nc.key == a1.key)
        print("repr/str:", repr(a1), str(a1))

        # Pickle roundtrip preserves cache vs no_cache semantics
        p_cached = pickle.loads(pickle.dumps(a1))
        p_nocache = pickle.loads(pickle.dumps(a_nc))
        print("pickle cached is a1:", p_cached is a1)
        print("pickle nocache is not a1:", p_nocache is not a1, p_nocache.key == "Test/Alpha")

        # Selective clear_cache
        ZoneInfo.clear_cache(only_keys=["Test/Alpha"])
        a3 = ZoneInfo("Test/Alpha")
        b2 = ZoneInfo("Test/Beta")
        print("after selective clear:", a3 is not a1, b2 is b1)

        # Full clear_cache
        ZoneInfo.clear_cache()
        b3 = ZoneInfo("Test/Beta")
        print("after full clear:", b3 is not b1)

        # Missing key raises ZoneInfoNotFoundError
        try:
            ZoneInfo("Test/DoesNotExist")
        except ZoneInfoNotFoundError as e:
            print("missing -> ZoneInfoNotFoundError:", isinstance(e, KeyError))

        # Empty TZPATH
        reset_tzpath(())
        print("empty TZPATH:", zoneinfo.TZPATH, available_timezones())

        # Invalid reset_tzpath arguments
        try:
            reset_tzpath("/some/path")
        except TypeError:
            print("reset_tzpath(str) -> TypeError")
        try:
            reset_tzpath(["relative/path"])
        except ValueError:
            print("reset_tzpath(rel) -> ValueError")

        # PYTHONTZPATH warning on relative entry
        os.environ["PYTHONTZPATH"] = os.pathsep.join([tmpdir, "rel_bad_path"])
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            reset_tzpath()
        print(
            "env warning:",
            len(caught) == 1 and issubclass(caught[0].category, InvalidTZPathWarning),
            zoneinfo.TZPATH == (tmpdir,),
        )
finally:
    if orig_env is None:
        if "PYTHONTZPATH" in os.environ:
            del os.environ["PYTHONTZPATH"]
    else:
        os.environ["PYTHONTZPATH"] = orig_env
    ZoneInfo.clear_cache()
    reset_tzpath()
