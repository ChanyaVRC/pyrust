# io.StringIO / BytesIO — writelines, line iteration, capability predicates,
# flush/isatty/fileno, closed property, BytesIO readinto/read1 (issue #2008).

import io

# ── writelines (no separators added) ─────────────────────────────────────────
s = io.StringIO()
assert s.writelines(["a", "b", "c"]) is None
assert s.getvalue() == "abc"

b = io.BytesIO()
assert b.writelines([b"x", b"y", bytearray(b"z")]) is None
assert b.getvalue() == b"xyz"

# ── line iteration: __iter__ returns self, __next__ yields lines ─────────────
sio = io.StringIO("a\nb\nc")
assert iter(sio) is sio
assert list(io.StringIO("a\nb\nc")) == ["a\n", "b\n", "c"]
assert next(iter(io.StringIO("only"))) == "only"
assert list(io.BytesIO(b"a\nb\nc")) == [b"a\n", b"b\n", b"c"]

# StopIteration at EOF
it = iter(io.StringIO(""))
try:
    next(it)
    print("FAIL: expected StopIteration")
except StopIteration:
    pass

# ── capability predicates + flush + isatty ───────────────────────────────────
for make in (lambda: io.StringIO("x"), lambda: io.BytesIO(b"x")):
    s = make()
    assert s.seekable() is True
    assert s.readable() is True
    assert s.writable() is True
    assert s.flush() is None
    assert s.isatty() is False
    assert s.closed is False

# ── fileno raises io.UnsupportedOperation (open or closed) ───────────────────
for make in (lambda: io.StringIO("x"), lambda: io.BytesIO(b"x")):
    s = make()
    try:
        s.fileno()
        print("FAIL: expected UnsupportedOperation")
    except io.UnsupportedOperation as e:
        assert str(e) == "fileno"

# ── closed property reflects state ───────────────────────────────────────────
s = io.StringIO("x")
assert s.closed is False
s.close()
assert s.closed is True

# ── BytesIO.readinto fills the buffer and returns the count ──────────────────
b = io.BytesIO(b"abcde")
buf = bytearray(3)
assert b.readinto(buf) == 3
assert buf == bytearray(b"abc")
assert b.tell() == 3

b = io.BytesIO(b"ab")
buf = bytearray(5)
assert b.readinto(buf) == 2
assert bytes(buf) == b"ab\x00\x00\x00"

# readinto rejects a read-only / non-bytes-like target
try:
    io.BytesIO(b"ab").readinto(b"xx")
    print("FAIL: expected TypeError on read-only readinto target")
except TypeError:
    pass

# ── BytesIO.read1 behaves like read for an in-memory stream ──────────────────
assert io.BytesIO(b"abcdef").read1(2) == b"ab"
assert io.BytesIO(b"abcdef").read1() == b"abcdef"
assert io.BytesIO(b"abcdef").read1(-1) == b"abcdef"

# ── writelines type-checks each element ──────────────────────────────────────
try:
    io.StringIO().writelines([1])
    print("FAIL: expected TypeError (StringIO writelines non-str)")
except TypeError:
    pass
try:
    io.BytesIO().writelines([1])
    print("FAIL: expected TypeError (BytesIO writelines non-bytes)")
except TypeError:
    pass

# ── writelines is NOT atomic: the valid prefix is written before the bad
#    element raises (CPython delegates each item to write as it iterates) ──────
s = io.StringIO()
try:
    s.writelines(["a", "b", 1, "c"])
    print("FAIL: expected TypeError in writelines")
except TypeError:
    pass
assert s.getvalue() == "ab"
b = io.BytesIO()
try:
    b.writelines([b"a", b"b", 1])
    print("FAIL: expected TypeError in writelines")
except TypeError:
    pass
assert b.getvalue() == b"ab"

# ── closed-file error message: CPython's trailing-period quirk ────────────────
# StringIO: read/readline/tell/seek/getvalue/truncate/write/seekable/readable/
# writable/__next__ omit the period; readlines/writelines/isatty/iteration and
# __enter__ include it.  BytesIO uses the period everywhere.
def msg(fn):
    try:
        fn()
        return "NO RAISE"
    except ValueError as e:
        return str(e)

NO = "I/O operation on closed file"
DOT = "I/O operation on closed file."

s = io.StringIO("a\nb"); s.close()
assert msg(lambda: s.read()) == NO
assert msg(lambda: s.readline()) == NO
assert msg(lambda: s.readlines()) == DOT
assert msg(lambda: s.tell()) == NO
assert msg(lambda: s.seek(0)) == NO
assert msg(lambda: s.write("x")) == NO
assert msg(lambda: s.writelines(["x"])) == DOT
assert msg(lambda: s.__next__()) == NO
assert msg(lambda: list(s)) == DOT
assert msg(lambda: s.isatty()) == DOT

b = io.BytesIO(b"a\nb"); b.close()
for fn in (lambda: b.read(), lambda: b.readline(), lambda: b.readlines(),
           lambda: b.tell(), lambda: b.seek(0), lambda: b.write(b"x"),
           lambda: b.writelines([b"x"]), lambda: b.getvalue(),
           lambda: b.truncate(), lambda: b.readinto(bytearray(1)),
           lambda: b.read1(), lambda: b.__next__(), lambda: list(b),
           lambda: b.isatty(), lambda: b.flush()):
    assert msg(fn) == DOT, fn

# ── closed-stream predicates raise ValueError ────────────────────────────────
for make in (lambda: io.StringIO("x"), lambda: io.BytesIO(b"x")):
    c = make()
    c.close()
    for op in (c.seekable, c.readable, c.writable, c.isatty):
        try:
            op()
            print("FAIL: expected ValueError on closed predicate")
        except ValueError:
            pass
    # iteration on a closed stream raises ValueError
    try:
        list(c)
        print("FAIL: expected ValueError iterating closed stream")
    except ValueError:
        pass

print("io methods ok")
