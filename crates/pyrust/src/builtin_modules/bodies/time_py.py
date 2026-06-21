# Python-level members of the `time` module (issue #2787).
#
# `struct_time` is the 9-field sequence returned by `gmtime` / `localtime`
# and accepted by `mktime` / `strftime`.  CPython implements it as a struct
# sequence whose constructor takes a single iterable of nine values
# (`struct_time((2020, 1, 1, 0, 0, 0, 0, 0, 0))`).  Here it is a small subclass
# of a `collections.namedtuple` that reproduces that single-iterable
# constructor while keeping the indexable + attribute-accessible surface
# (`t[0] == t.tm_year`, `len(t) == 9`) the documented API relies on.
#
# The native `time.rs` functions build instances by fetching this class off
# the imported module and calling it with a single nine-element tuple.
#
# `namedtuple` is pre-seeded into this module's exec namespace by
# `inject_python_members` (mirroring the `asyncio` bridge-helper seeding), so
# no top-level `import` is needed here.

_struct_time_base = namedtuple(
    "struct_time",
    [
        "tm_year",
        "tm_mon",
        "tm_mday",
        "tm_hour",
        "tm_min",
        "tm_sec",
        "tm_wday",
        "tm_yday",
        "tm_isdst",
    ],
)


class struct_time(_struct_time_base):
    """The time value sequence returned by gmtime(), localtime(), ...

    Constructed from a single iterable of nine values, matching CPython's
    `time.struct_time`.
    """

    __slots__ = ()

    def __new__(cls, iterable):
        values = tuple(iterable)
        if len(values) != 9:
            raise TypeError(
                "time.struct_time() takes an at least 9-sequence "
                "(%d-sequence given)" % len(values)
            )
        return _struct_time_base.__new__(cls, *values)
