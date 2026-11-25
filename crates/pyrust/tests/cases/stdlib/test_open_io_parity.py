"""CPython 3.12 parity tests for open() and io (issue #3206).

Covers:
- io.UnsupportedOperation multiple inheritance from (OSError, ValueError)
- open() buffering (-1, 0 binary, 1 line-buffered text, >1, errors)
- open() errors (strict, ignore, replace, backslashreplace, surrogateescape)
- open() newline (None, '', '\n', '\r', '\r\n') for reading and writing
- open() integer file descriptors with closefd=True and closefd=False
- open() PathLike, exclusive creation ('x'), invalid modes, and attributes
"""

import io
import os
import pathlib
import shutil
import tempfile

# ── 1. io.UnsupportedOperation multiple inheritance ──────────────────────────
assert issubclass(io.UnsupportedOperation, OSError)
assert issubclass(io.UnsupportedOperation, ValueError)
assert io.UnsupportedOperation.__bases__ == (OSError, ValueError)

caught_val = False
try:
    raise io.UnsupportedOperation("not supported")
except ValueError as e:
    caught_val = isinstance(e, OSError) and isinstance(e, ValueError)
assert caught_val

caught_os = False
try:
    raise io.UnsupportedOperation("not supported")
except OSError as e:
    caught_os = isinstance(e, OSError) and isinstance(e, ValueError)
assert caught_os

# StringIO / BytesIO fileno raises UnsupportedOperation, catchable as ValueError too
try:
    io.StringIO("abc").fileno()
    assert False
except ValueError:
    pass

print("io.UnsupportedOperation MI ok")

tmpdir = tempfile.mkdtemp(prefix="pyrust_open_io_")
try:
    # ── 2. open() newline parameter (writing & reading) ──────────────────────
    for nl_write, expected_raw in [
        ("", b"a\nb\nc"),
        ("\n", b"a\nb\nc"),
        ("\r", b"a\rb\rc"),
        ("\r\n", b"a\r\nb\r\nc"),
    ]:
        p = os.path.join(tmpdir, "nl_write.txt")
        with open(p, "w", encoding="utf-8", newline=nl_write) as f:
            n = f.write("a\nb\nc")
            assert n == 5, (nl_write, n)
        with open(p, "rb") as f:
            raw = f.read()
        assert raw == expected_raw, (nl_write, raw, expected_raw)

    # newline=None on write translates \n -> os.linesep
    p_def = os.path.join(tmpdir, "nl_default.txt")
    with open(p_def, "w", encoding="utf-8", newline=None) as f:
        f.write("x\ny\n")
    with open(p_def, "rb") as f:
        raw_def = f.read()
    assert raw_def == ("x" + os.linesep + "y" + os.linesep).encode("ascii")

    # Mixed line endings on read
    p_mixed = os.path.join(tmpdir, "nl_mixed.txt")
    with open(p_mixed, "wb") as f:
        f.write(b"a\nb\rc\r\nd")

    # newline=None: translates \r\n, \r, \n -> \n
    with open(p_mixed, "r", encoding="utf-8", newline=None) as f:
        assert f.read() == "a\nb\nc\nd"
    with open(p_mixed, "r", encoding="utf-8", newline=None) as f:
        assert f.readlines() == ["a\n", "b\n", "c\n", "d"]

    # newline="": preserves line endings untranslated, still splits on all 3
    with open(p_mixed, "r", encoding="utf-8", newline="") as f:
        assert f.read() == "a\nb\rc\r\nd"
    with open(p_mixed, "r", encoding="utf-8", newline="") as f:
        assert f.readlines() == ["a\n", "b\r", "c\r\n", "d"]

    # newline="\n": splits ONLY on \n
    with open(p_mixed, "r", encoding="utf-8", newline="\n") as f:
        assert f.readlines() == ["a\n", "b\rc\r\n", "d"]

    # newline="\r": splits ONLY on \r
    with open(p_mixed, "r", encoding="utf-8", newline="\r") as f:
        assert f.readlines() == ["a\nb\r", "c\r", "\nd"]

    # newline="\r\n": splits ONLY on \r\n
    with open(p_mixed, "r", encoding="utf-8", newline="\r\n") as f:
        assert f.readlines() == ["a\nb\rc\r\n", "d"]

    # Invalid newline value & binary+newline errors
    try:
        open(p_mixed, "r", newline="\n\r")
        assert False
    except ValueError:
        pass
    try:
        open(p_mixed, "rb", newline="\n")
        assert False
    except ValueError:
        pass

    print("open newline ok")

    # ── 3. open() errors parameter ───────────────────────────────────────────
    p_bad = os.path.join(tmpdir, "bad_utf8.bin")
    with open(p_bad, "wb") as f:
        f.write(b"hi\xff\xfe!")

    # strict raises UnicodeDecodeError
    try:
        with open(p_bad, "r", encoding="utf-8", errors="strict") as f:
            _ = f.read()
        assert False
    except UnicodeDecodeError:
        pass

    # ignore drops invalid bytes
    with open(p_bad, "r", encoding="utf-8", errors="ignore") as f:
        assert f.errors == "ignore"
        assert f.read() == "hi!"

    # replace uses U+FFFD
    with open(p_bad, "r", encoding="utf-8", errors="replace") as f:
        assert f.read() == "hi\ufffd\ufffd!"

    # backslashreplace escapes invalid bytes
    with open(p_bad, "r", encoding="utf-8", errors="backslashreplace") as f:
        assert f.read() == "hi\\xff\\xfe!"

    # surrogateescape round-trips arbitrary invalid bytes losslessly!
    with open(p_bad, "r", encoding="utf-8", errors="surrogateescape") as f:
        surr_text = f.read()
    assert len(surr_text) == 5
    p_rt = os.path.join(tmpdir, "surr_rt.bin")
    with open(p_rt, "w", encoding="utf-8", errors="surrogateescape", newline="") as f:
        written = f.write(surr_text)
        assert written == 5
    with open(p_rt, "rb") as f:
        assert f.read() == b"hi\xff\xfe!"

    # write-side errors: ascii + ignore / replace / backslashreplace
    p_enc = os.path.join(tmpdir, "enc_err.txt")
    with open(p_enc, "w", encoding="ascii", errors="ignore") as f:
        f.write("a\u00e9b")
    with open(p_enc, "rb") as f:
        assert f.read() == b"ab"

    with open(p_enc, "w", encoding="ascii", errors="replace") as f:
        f.write("a\u00e9b")
    with open(p_enc, "rb") as f:
        assert f.read() == b"a?b"

    with open(p_enc, "w", encoding="ascii", errors="backslashreplace") as f:
        f.write("a\u00e9b")
    with open(p_enc, "rb") as f:
        assert f.read() == b"a\\xe9b"

    # binary mode rejects errors kwarg
    try:
        open(p_bad, "rb", errors="ignore")
        assert False
    except ValueError:
        pass

    print("open errors ok")

    # ── 4. open() buffering parameter ────────────────────────────────────────
    p_buf = os.path.join(tmpdir, "buf.bin")
    # buffering=0 in text mode raises ValueError
    try:
        open(p_buf, "w", buffering=0)
        assert False
    except ValueError:
        pass

    # buffering=None raises TypeError
    try:
        open(p_buf, "w", buffering=None)
        assert False
    except TypeError:
        pass

    # buffering=0 in binary mode writes immediately
    with open(p_buf, "wb", buffering=0) as f:
        f.write(b"abc")
        with open(p_buf, "rb") as r:
            assert r.read() == b"abc"
    with open(p_buf, "rb", buffering=0) as f:
        assert f.read(2) == b"ab"
        assert f.read(2) == b"c"
        assert f.read(2) == b""

    # buffering=1 in text mode is line-buffered and flushes on \n
    p_lb = os.path.join(tmpdir, "linebuf.txt")
    with open(p_lb, "w", buffering=1, encoding="utf-8", newline="\n") as f:
        assert f.line_buffering is True
        f.write("line1\n")
        with open(p_lb, "r", encoding="utf-8") as r:
            assert r.read() == "line1\n"

    # buffering > 1
    with open(p_lb, "w", buffering=1024, encoding="utf-8") as f:
        assert f.line_buffering is False
        f.write("buffered")
    with open(p_lb, "r", buffering=1024, encoding="utf-8") as f:
        assert f.read() == "buffered"

    print("open buffering ok")

    # ── 5. open() integer file descriptors with closefd=True/False ───────────
    fd, fd_path = tempfile.mkstemp(dir=tmpdir)
    try:
        # Write via fd with closefd=False: fd remains open afterwards!
        with open(fd, "w", encoding="utf-8", newline="\n", closefd=False) as f:
            assert f.name == fd
            assert f.fileno() == fd
            assert f.writable() is True
            assert f.readable() is False
            f.write("hello fd\n")
        # Since closefd=False, fd is still valid and we can seek/read via open(fd, closefd=True)
        with open(fd, "r", encoding="utf-8", closefd=True) as f:
            assert f.name == fd
            assert f.fileno() == fd
            f.seek(0)
            assert f.read() == "hello fd\n"
        # Now fd was closed by closefd=True; closing again raises OSError
        try:
            os.close(fd)
            assert False, "expected OSError on already-closed fd"
        except OSError:
            pass
    finally:
        if os.path.exists(fd_path):
            os.unlink(fd_path)

    # Negative fd raises ValueError
    try:
        open(-1, "r")
        assert False
    except ValueError:
        pass

    print("open fd ok")

    # ── 6. open() PathLike & mode='x' & mode validation ──────────────────────
    p_obj = pathlib.Path(tmpdir) / "pathlike.txt"
    with open(p_obj, "x", encoding="utf-8") as f:
        f.write("created")
    try:
        open(p_obj, "x", encoding="utf-8")
        assert False
    except FileExistsError:
        pass
    with open(p_obj, "r", encoding="utf-8") as f:
        assert f.read() == "created"

    for bad_mode in ("", "rw", "bt", "rr", "z"):
        try:
            open(p_obj, bad_mode)
            assert False, bad_mode
        except ValueError:
            pass

    print("open pathlike & modes ok")
finally:
    shutil.rmtree(tmpdir, ignore_errors=True)

print("test_open_io_parity ok")
