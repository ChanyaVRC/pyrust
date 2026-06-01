# bytes/bytearray split/rsplit accept `sep` and `maxsplit` by keyword (#1881)
# CPython 3.12 parity: positional and keyword forms must agree, for both types.

for ctor in (bytes, bytearray):
    name = ctor.__name__
    v = ctor(b"a b c")

    # maxsplit by keyword (whitespace split, sep defaults to None)
    print(name, "split kw:", v.split(maxsplit=1))
    print(name, "rsplit kw:", v.rsplit(maxsplit=1))

    # maxsplit by position (must be unchanged)
    print(name, "split pos:", v.split(b" ", 1))
    print(name, "rsplit pos:", v.rsplit(b" ", 1))

    # sep by keyword
    print(name, "sep kw:", v.split(sep=b" "))
    # sep + maxsplit by keyword
    print(name, "sep+max kw:", v.split(sep=b" ", maxsplit=1))
    print(name, "sep+max rkw:", v.rsplit(sep=b" ", maxsplit=1))

    # explicit separator with keyword maxsplit
    print(name, "explicit sep:", ctor(b"a-b-c").split(b"-", maxsplit=1))
    print(name, "explicit rsep:", ctor(b"a-b-c").rsplit(b"-", maxsplit=1))

    # maxsplit edge values
    print(name, "max=0:", v.split(maxsplit=0))
    print(name, "rmax=0:", v.rsplit(maxsplit=0))
    print(name, "max=-1:", v.split(maxsplit=-1))
    print(name, "default:", v.split())

    # Error paths
    for label, fn in [
        ("3pos", lambda: v.split(b" ", 1, 2)),
        ("bad-kw", lambda: v.split(foo=2)),
        ("extra-kw", lambda: v.split(b" ", maxsplit=1, foo=2)),
        ("dup-sep", lambda: v.split(b" ", sep=b" ")),
        ("dup-maxsplit", lambda: v.split(b" ", 1, maxsplit=1)),
    ]:
        try:
            fn()
            print(name, label, "NO ERROR")
        except TypeError as e:
            print(name, label, "TypeError:", e)
