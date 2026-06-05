# str.expandtabs(tabsize=) / str.splitlines(keepends=) keyword-argument parity (#1999).
# The bytes/bytearray receivers were fixed in #1990; the str receiver had the same
# latent gap on a different code path.  CPython 3.12 accepts the single argument by
# keyword as well as by position; mixing/duplicating/unknown keywords raise TypeError
# with specific wording.


def show(fn):
    try:
        print(repr(fn()))
    except TypeError as e:
        print("TypeError:", e)


# --- expandtabs: positional + keyword + default ---
show(lambda: "a\tb".expandtabs())  # default tabsize=8
show(lambda: "a\tb".expandtabs(4))  # positional
show(lambda: "a\tb".expandtabs(tabsize=4))  # keyword
show(lambda: "a\tb".expandtabs(tabsize=0))  # tabsize 0 removes tabs
show(lambda: "a\tbc\td".expandtabs(tabsize=3))

# --- expandtabs: error parity ---
show(lambda: "a\tb".expandtabs(1, 2))  # too many positional
show(lambda: "a\tb".expandtabs(4, tabsize=4))  # name + position clash
show(lambda: "a\tb".expandtabs(tabsize=4, foo=1))  # overflow with unknown kw
show(lambda: "a\tb".expandtabs(bad=1))  # unknown keyword

# --- splitlines: positional + keyword + default ---
show(lambda: "a\nb\nc".splitlines())  # default keepends=False
show(lambda: "a\nb\nc".splitlines(True))  # positional
show(lambda: "a\nb\nc".splitlines(keepends=True))  # keyword
show(lambda: "a\nb\nc".splitlines(keepends=False))  # keyword False
show(lambda: "a\r\nb".splitlines(keepends=True))  # CRLF kept

# --- splitlines: error parity ---
show(lambda: "a\nb".splitlines(1, 2))  # too many positional
show(lambda: "a\nb".splitlines(True, keepends=True))  # name + position clash
show(lambda: "a\nb".splitlines(keepends=True, foo=1))  # overflow with unknown kw
show(lambda: "a\nb".splitlines(bad=1))  # unknown keyword
