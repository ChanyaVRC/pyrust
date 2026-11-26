# Parity fixture for #3318: a file object whose last reference drops is
# flushed and closed right away (CPython's `iobase_finalize`), so buffered
# writes reach the file and the handle is released. Also pins the method-call
# register reuse that releases a temporary receiver when its call returns.

import errno
import os
import tempfile


def read(path, mode="r"):
    with open(path, mode) as fh:
        return fh.read()


with tempfile.TemporaryDirectory() as d:
    fn = os.path.join(d, "f.txt")
    other = os.path.join(d, "g.txt")

    print("== issue reproducer ==")
    open(fn, "w").write("xyz")
    print(repr(read(fn)))

    def write_local():
        f = open(fn, "w")
        f.write("abc")

    write_local()
    print(repr(read(fn)))

    f = open(fn, "w")
    f.write("123")
    f = None
    print(repr(read(fn)))

    g = open(fn, "w")
    g.write("456")
    del g
    print(repr(read(fn)))

    print("== keyword arguments to open ==")
    open(fn, "w", encoding="utf-8").write("kw-module")
    print(repr(read(fn)))
    open(fn, mode="w").write("kw-mode")
    print(repr(read(fn)))

    def write_kw():
        open(fn, "w", encoding="utf-8").write("kw-func")
        return read(fn)

    print(repr(write_kw()))

    def write_then_read_same_function():
        open(fn, "w").write("same-func")
        return read(fn)

    print(repr(write_then_read_same_function()))

    print("== loops ==")
    for i in range(3):
        open(fn, "w").write(f"loop{i}")
        print(repr(read(fn)))

    print("== modes and buffering ==")
    open(fn, "wb").write(b"\x00\x01bin")
    print(read(fn, "rb"))
    open(fn, "w").write("a")
    open(fn, "a").write("b")
    open(fn, "a").write("c")
    print(repr(read(fn)))
    open(fn, "w").writelines(["l1\n", "l2\n"])
    print(repr(read(fn)))
    open(fn, "w", buffering=1).write("line-buffered")
    print(repr(read(fn)))
    open(fn, "w", buffering=1 << 16).write("big-buffer")
    print(repr(read(fn)))
    open(fn, "w").write("hello")
    rp = open(fn, "r+")
    rp.seek(0, 2)
    rp.write("!")
    rp = None
    print(repr(read(fn)))
    open(fn, "w", encoding="utf-16").write("utf16")
    print(read(fn, "rb")[:2] in (b"\xff\xfe", b"\xfe\xff"))
    with open(fn, encoding="utf-16") as fh:
        print(repr(fh.read()))

    print("== rebinding and containers ==")
    x = open(fn, "w")
    x.write("first")
    x = open(other, "w")
    print(repr(read(fn)))
    x.write("second")
    del x
    print(repr(read(other)))

    lst = [open(fn, "w")]
    lst[0].write("in-list")
    lst.clear()
    print(repr(read(fn)))

    class Holder:
        pass

    h = Holder()
    h.f = open(fn, "w")
    h.f.write("attr")
    del h
    print(repr(read(fn)))

    def make_writer():
        cell = open(fn, "w")

        def inner():
            cell.write("cell")

        return inner

    writer = make_writer()
    writer()
    writer = None
    print(repr(read(fn)))

    def boom():
        local = open(fn, "w")
        local.write("boom")
        raise ValueError("x")

    try:
        boom()
    except ValueError:
        pass
    print(repr(read(fn)))

    print("== explicit close is not repeated on drop ==")
    f = open(fn, "w")
    f.write("closed")
    f.close()
    del f
    print(repr(read(fn)))
    with open(fn, "w") as f:
        f.write("with")
    del f
    print(repr(read(fn)))

    print("== fd-backed files ==")
    fd = os.open(fn, os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
    w = os.fdopen(fd, "w")
    w.write("fdopen")
    del w
    print(repr(read(fn)))
    try:
        os.close(fd)
        print("fd still open")
    except OSError as e:
        print("fd closed by drop:", e.errno == errno.EBADF)

    fd = os.open(fn, os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
    w = open(fd, "w", closefd=False)
    w.write("no-closefd")
    del w
    print(repr(read(fn)))
    os.close(fd)
    print("closefd=False left the fd open")

    print("== handle released for remove / replace ==")
    p = os.path.join(d, "rm.txt")
    open(p, "w").write("rm")
    os.remove(p)
    print(os.path.exists(p))
    p = os.path.join(d, "mv.txt")
    open(p, "w").write("mv")
    os.replace(p, other)
    print(repr(read(other)))

print("== TemporaryDirectory cleanup ==")
print(os.path.exists(d))
