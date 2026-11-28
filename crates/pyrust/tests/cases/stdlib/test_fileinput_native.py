# CPython 3.12.3 parity fixture for the native `fileinput` module (issue #3485).

import bz2
import collections.abc
import fileinput
import gzip
import io
import os
import pathlib
import sys
import tempfile


def expect(fn, *args, **kwargs):
    try:
        return ("OK", fn(*args, **kwargs))
    except BaseException as e:
        return (type(e).__name__, str(e))


def exc_name(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
        return "OK"
    except BaseException as e:
        return type(e).__name__


def write_bytes(path, data):
    with open(path, "wb") as fp:
        fp.write(data)


def read_bytes(path):
    with open(path, "rb") as fp:
        return fp.read().replace(b"\r\n", b"\n")


# ---------------------------------------------------------------------------
# 1. Module surface
# ---------------------------------------------------------------------------
print("__all__:", fileinput.__all__)
print("state initial:", fileinput._state)
print("FileInput alias:", fileinput.FileInput[str], fileinput.FileInput[int, str])
print("FileInput module:", fileinput.FileInput.__module__, fileinput.FileInput.__name__)
print("globals:", fileinput.os is os, fileinput.sys is sys, fileinput.io is io)
print("public names:", sorted(n for n in dir(fileinput) if not n.startswith("__")))
print("hook_encoded type:", callable(fileinput.hook_encoded("utf-8")))
print("doc head:", fileinput.__doc__.splitlines()[0])
print("iter protocol:", isinstance(fileinput.FileInput([]), collections.abc.Iterator))

# ---------------------------------------------------------------------------
# 2. Helpers with no active input()
# ---------------------------------------------------------------------------
fileinput.close()
print("close idle:", fileinput._state)
for name in (
    "nextfile",
    "filename",
    "lineno",
    "filelineno",
    "fileno",
    "isfirstline",
    "isstdin",
):
    print("idle", name, expect(getattr(fileinput, name)))
print("close extra arg:", expect(fileinput.close, 1))
print("nextfile extra arg:", expect(fileinput.nextfile, 1))
print("input extra positional:", expect(fileinput.input, [], False, "", 4))
print("input bad kw:", expect(fileinput.input, bogus=1))
print("hook_compressed missing:", expect(fileinput.hook_compressed, "x"))
print("hook_encoded missing:", expect(fileinput.hook_encoded))

# ---------------------------------------------------------------------------
# 3. Constructor validation
# ---------------------------------------------------------------------------
print("bad mode w:", expect(fileinput.FileInput, [], mode="w"))
print("bad mode U:", expect(fileinput.FileInput, [], mode="U"))
print("bad mode rU:", expect(fileinput.FileInput, [], mode="rU"))
print("bad mode rt:", expect(fileinput.FileInput, [], mode="rt"))
print("hook+inplace:", expect(fileinput.FileInput, [], inplace=True, openhook=fileinput.hook_encoded("utf-8")))
print("hook not callable:", expect(fileinput.FileInput, [], openhook=123))
print("hook falsy ok:", exc_name(fileinput.FileInput, [], openhook=0))
print("hook falsy + inplace:", exc_name(fileinput.FileInput, [], inplace=True, openhook=0))


class Quiet(fileinput.FileInput):
    def __del__(self):
        pass


print("init extra positional:", expect(Quiet, [], False, "", 4))
print("init bad kw:", expect(Quiet, [], bogus=1))
print("bad mode on subclass:", expect(Quiet, [], mode="x"))
print("rb with encoding ok:", exc_name(fileinput.FileInput, [], mode="rb", encoding="utf-8"))
print("rb with errors ok:", exc_name(fileinput.FileInput, [], mode="rb", errors="strict"))

fi = fileinput.FileInput([])
print("empty list ->", fi._files, fi.filename(), fi.lineno(), fi.filelineno(), fi.fileno(), fi.isfirstline(), fi.isstdin())
fi.close()
fi = fileinput.FileInput(iter(()))
print("empty iterator ->", fi._files)
print("empty iterator read:", fi.readline() == "", list(fi))
fi.close()
print("after close files:", fi._files)

old_argv = sys.argv
try:
    sys.argv = ["prog"]
    print("argv default:", fileinput.FileInput()._files)
    sys.argv = ["prog", "x.txt", "y.txt"]
    print("argv files:", fileinput.FileInput()._files)
    print("argv none:", fileinput.FileInput(None)._files)
finally:
    sys.argv = old_argv


class StrSub(str):
    pass


class PathLike:
    def __init__(self, p):
        self.p = p

    def __fspath__(self):
        return self.p


print("str files:", fileinput.FileInput("abc")._files)
print("str subclass:", fileinput.FileInput(StrSub("abc"))._files)
print("pathlike:", fileinput.FileInput(PathLike("abc"))._files)
print("pathlib:", fileinput.FileInput(pathlib.PurePosixPath("abc"))._files)
print("tuple files:", fileinput.FileInput(("a", "b"))._files)
print("generator files:", fileinput.FileInput(x for x in ("a", "b"))._files)
print("falsy files:", fileinput.FileInput(0)._files)
print("int files:", expect(Quiet, 5))
print("attr order:", list(vars(fileinput.FileInput(["a"]))))

with tempfile.TemporaryDirectory() as tmp:
    f1 = os.path.join(tmp, "a.txt")
    f2 = os.path.join(tmp, "empty.txt")
    f3 = os.path.join(tmp, "b.txt")
    f4 = os.path.join(tmp, "nonl.txt")
    write_bytes(f1, b"alpha\nbeta\n")
    write_bytes(f2, b"")
    write_bytes(f3, b"gamma\ndelta\n")
    write_bytes(f4, b"tail1\ntail2")

    def base(p):
        if p is None:
            return None
        if isinstance(p, bytes):
            return os.path.basename(p)
        return p if p.startswith("<") else os.path.basename(p)

    def snap(fi):
        return (base(fi.filename()), fi.lineno(), fi.filelineno(), fi.isfirstline(), fi.isstdin())

    # -----------------------------------------------------------------------
    # 4. Iteration over several files, an empty file, and no trailing newline
    # -----------------------------------------------------------------------
    rows = []
    with fileinput.FileInput([f1, f2, f3, f4], encoding="utf-8") as fi:
        print("before:", snap(fi), fi.fileno())
        for line in fi:
            rows.append(snap(fi) + (fi.fileno() >= 0, line))
        print("after:", snap(fi), fi.fileno())
    for row in rows:
        print("row:", row)
    print("closed after with:", fi._file, fi._files)

    # -----------------------------------------------------------------------
    # 5. Single path forms
    # -----------------------------------------------------------------------
    with fileinput.FileInput(f1, encoding="utf-8") as fi:
        print("single str:", list(fi), base(fi.filename()))
    with fileinput.FileInput(pathlib.Path(f1), encoding="utf-8") as fi:
        print("single Path:", list(fi), base(fi.filename()))
    with fileinput.FileInput(PathLike(f3), encoding="utf-8") as fi:
        print("single PathLike:", list(fi), base(fi.filename()))
    with fileinput.FileInput((pathlib.Path(f1), f3), encoding="utf-8") as fi:
        print("mixed:", [(base(str(fi.filename())), l) for l in fi])

    # -----------------------------------------------------------------------
    # 6. readline()
    # -----------------------------------------------------------------------
    with fileinput.FileInput([f1, f2, f4], encoding="utf-8") as fi:
        got = []
        while True:
            chunk = fi.readline()
            if not chunk:
                break
            got.append(snap(fi) + (chunk,))
        print("readline:", got, repr(fi.readline()), repr(fi.readline()))
        print("next after end:", expect(next, fi))

    # -----------------------------------------------------------------------
    # 7. Binary mode (str and bytes file names)
    # -----------------------------------------------------------------------
    with fileinput.FileInput([f1, f3], mode="rb") as fi:
        got = []
        for line in fi:
            got.append((base(fi.filename()), fi.lineno(), line))
        print("rb:", got)
    with fileinput.FileInput([os.fsencode(f1), os.fsencode(f4)], mode="rb") as fi:
        got = []
        for line in fi:
            got.append((base(fi.filename()), fi.lineno(), fi.filelineno(), line))
        print("rb bytes names:", got)
    with fileinput.FileInput([os.fsencode(f3)], encoding="utf-8") as fi:
        print("text bytes names:", list(fi))
    with fileinput.FileInput([f1], mode="rb") as fi:
        print("rb readline:", fi.readline(), fi.readline(), repr(fi.readline()))
    fi = fileinput.FileInput([], mode="rb")
    print("rb empty readline:", repr(fi.readline()) if False else "skipped")
    fi.close()
    fi = fileinput.FileInput(iter(()), mode="rb")
    print("rb no files:", repr(fi.readline()), list(fi))
    fi.close()
    fi = fileinput.FileInput(iter(()), mode="r")
    print("r no files:", repr(fi.readline()))
    fi.close()

    # -----------------------------------------------------------------------
    # 8. nextfile()
    # -----------------------------------------------------------------------
    skipped = []
    with fileinput.FileInput([f1, f3, f4], encoding="utf-8") as fi:
        print("nextfile before read:", fi.nextfile(), snap(fi))
        for line in fi:
            skipped.append(snap(fi) + (line,))
            if fi.isfirstline():
                fi.nextfile()
        print("nextfile after end:", fi.nextfile(), snap(fi))
    for row in skipped:
        print("skipped:", row)

    # -----------------------------------------------------------------------
    # 9. Module-level state
    # -----------------------------------------------------------------------
    print("state before:", fileinput._state)
    mod_rows = []
    fi = fileinput.input([f1, f3], encoding="utf-8")
    print("state is returned:", fileinput._state is fi, type(fi).__name__)
    print("nested input before read:", exc_name(fileinput.input, [f1]))
    for line in fi:
        mod_rows.append(
            (
                base(fileinput.filename()),
                fileinput.lineno(),
                fileinput.filelineno(),
                fileinput.isfirstline(),
                fileinput.isstdin(),
                fileinput.fileno() >= 0,
                line,
            )
        )
        if fileinput.lineno() == 2:
            fileinput.nextfile()
    for row in mod_rows:
        print("module row:", row)
    print("state after loop:", fileinput._state is fi)
    print("second input after exhaustion:", exc_name(fileinput.input, [f1]))
    fileinput.close()
    print("state cleared:", fileinput._state)
    print("closed twice:", fileinput._state, expect(fileinput.close))

    fi = fileinput.input(f1, encoding="utf-8")
    fi2 = None
    print("reuse before read:", exc_name(fileinput.input, f3))
    fileinput.close()
    fi.close()

    # input() with invalid arguments leaves the state untouched
    print("input bad mode:", expect(fileinput.input, [f1], mode="w"), fileinput._state)

    # `with fileinput.input()` closes but leaves _state set to the closed object
    with fileinput.input([f1], encoding="utf-8") as fi:
        print("with input lines:", list(fi))
    print("state after with:", fileinput._state is fi, fi._file)
    print("fresh input after with:", exc_name(fileinput.input, [f3], encoding="utf-8"))
    fileinput.close()

    # -----------------------------------------------------------------------
    # 10. stdin
    # -----------------------------------------------------------------------
    old_stdin = sys.stdin
    try:
        sys.stdin = io.StringIO("s1\ns2\n")
        rows = []
        with fileinput.FileInput(["-", f1], encoding="utf-8") as fi:
            for line in fi:
                rows.append(snap(fi) + (fi.fileno() >= 0, line))
        for row in rows:
            print("stdin row:", row)
        print("stdin open after close:", sys.stdin.closed)

        sys.stdin = io.StringIO("only\n")
        fi = fileinput.FileInput()
        sys.argv = ["prog"]
        fi = fileinput.FileInput()
        print("default stdin:", list(fi), fi.filename(), fi.isstdin())
        fi.close()
        sys.argv = old_argv

        sys.stdin = io.StringIO("x\ny\n")
        fi = fileinput.FileInput(["-"], inplace=True)
        print("stdin inplace line:", fi.readline() == "x\n", sys.stdout is sys.__stdout__ or sys.stdout is not None)
        fi.close()
        print("stdin inplace closed:", sys.stdin.closed)

        class FakeStdin:
            def __init__(self):
                self.buffer = io.BytesIO(b"b1\nb2\n")
                self.closed_flag = False

            def readline(self):
                return "text\n"

            def close(self):
                self.closed_flag = True

        fake = FakeStdin()
        sys.stdin = fake
        with fileinput.FileInput(["-"], mode="rb") as fi:
            print("stdin rb:", list(fi), fi.fileno() if fi.isstdin() else "n/a")
        print("stdin rb kept open:", fake.closed_flag, fake.buffer.closed)

        class NoBuffer:
            def readline(self):
                return ""

        sys.stdin = NoBuffer()
        with fileinput.FileInput(["-"], mode="rb") as fi:
            print("stdin rb no buffer:", list(fi))
    finally:
        sys.stdin = old_stdin
        sys.argv = old_argv

    # -----------------------------------------------------------------------
    # 11. inplace
    # -----------------------------------------------------------------------
    real_stdout = sys.stdout
    in1 = os.path.join(tmp, "inplace1.txt")
    in2 = os.path.join(tmp, "inplace2.txt")
    in3 = os.path.join(tmp, "inplace3.txt")
    write_bytes(in1, b"one\ntwo\n")
    write_bytes(in2, b"three\nfour\n")
    write_bytes(in3, b"five\nsix\n")

    with fileinput.FileInput([in1], inplace=True, encoding="utf-8") as fi:
        print("stdout swapped before read:", sys.stdout is real_stdout)
        for line in fi:
            print("redirected:", sys.stdout is not real_stdout, file=real_stdout)
            print(line.strip().upper())
    print("stdout restored:", sys.stdout is real_stdout)
    print("inplace1:", read_bytes(in1), os.path.exists(in1 + ".bak"))

    with fileinput.FileInput([in2], inplace=True, backup=".orig", encoding="utf-8") as fi:
        for line in fi:
            print("PREFIX:" + line.strip())
    print("inplace2:", read_bytes(in2), read_bytes(in2 + ".orig"), os.path.exists(in2 + ".bak"))

    # multiple files, nextfile in the middle (rest of the first file is dropped)
    write_bytes(in1, b"a1\na2\na3\n")
    with fileinput.FileInput([in1, in3], inplace=True, backup=".bk", encoding="utf-8") as fi:
        for line in fi:
            sys.stdout.write(line.upper())
            if fi.filelineno() == 2 and base(fi.filename()) == "inplace1.txt":
                fi.nextfile()
    print("inplace multi:", read_bytes(in1), read_bytes(in3), read_bytes(in1 + ".bk"), read_bytes(in3 + ".bk"))
    print("stdout restored again:", sys.stdout is real_stdout)

    # binary inplace
    write_bytes(in1, b"x\ny\n")
    with fileinput.FileInput([in1], inplace=True, mode="rb") as fi:
        for line in fi:
            sys.stdout.write(line * 2)
    print("inplace rb:", read_bytes(in1), os.path.exists(in1 + ".bak"))

    # bytes name inplace
    write_bytes(in1, b"p\nq\n")
    fi = fileinput.FileInput([os.fsencode(in1)], inplace=True, backup=".orig", mode="rb")
    print("bytes name str backup:", exc_name(fi.readline), read_bytes(in1))
    fi.close()
    with fileinput.FileInput([os.fsencode(in1)], inplace=True, backup=b".orig", mode="rb") as fi:
        for line in fi:
            sys.stdout.write(line.upper())
    print("inplace bytes name:", read_bytes(in1), read_bytes(in1 + ".orig"))

    # a stale backup is replaced
    write_bytes(in1, b"new\n")
    write_bytes(in1 + ".bak", b"stale\n")
    with fileinput.FileInput([in1], inplace=True) as fi:
        for line in fi:
            sys.stdout.write("[" + line.strip() + "]\n")
    print("stale backup:", read_bytes(in1), os.path.exists(in1 + ".bak"))

    # missing file in inplace mode
    missing = os.path.join(tmp, "missing.txt")
    fi = fileinput.FileInput([missing, in1], inplace=True)
    print("inplace missing:", exc_name(fi.readline), sys.stdout is real_stdout, fi._files == (in1,))
    fi.close()

    # abandoned inplace input: nextfile via close() puts stdout back
    write_bytes(in1, b"keep\nme\n")
    fi = fileinput.FileInput([in1], inplace=True)
    first = fi.readline()
    print("swapped mid-read:", sys.stdout is not real_stdout, file=real_stdout)
    fi.close()
    print("restored by close:", sys.stdout is real_stdout, repr(first))
    print("data lost after close:", read_bytes(in1), os.path.exists(in1 + ".bak"))

    # fileno of the input while in place
    write_bytes(in1, b"z\n")
    with fileinput.FileInput([in1], inplace=True) as fi:
        fi.readline()
        number = fi.fileno()
        print("inplace fileno ok:", isinstance(number, int) and number >= 0, file=real_stdout)
    print("inplace fileno after:", fi.fileno())

    # via module-level input()
    write_bytes(in1, b"m1\nm2\n")
    for line in fileinput.input([in1], inplace=True, backup=".mod"):
        print(line.strip() + "!", end="\n")
    print("module inplace:", read_bytes(in1), read_bytes(in1 + ".mod"), fileinput._state is not None)
    fileinput.close()

    # -----------------------------------------------------------------------
    # 12. Hooks
    # -----------------------------------------------------------------------
    enc_file = os.path.join(tmp, "latin1.txt")
    write_bytes(enc_file, "caf\xe9\nna\xefve\n".encode("latin-1"))
    with fileinput.FileInput([enc_file], openhook=fileinput.hook_encoded("latin-1")) as fi:
        print("hook_encoded:", list(fi))
    with fileinput.FileInput([enc_file], openhook=fileinput.hook_encoded("ascii", "replace")) as fi:
        print("hook_encoded replace:", list(fi))
    with fileinput.FileInput([enc_file], openhook=fileinput.hook_encoded("ascii")) as fi:
        print("hook_encoded strict:", expect(list, fi))
    hook = fileinput.hook_encoded("utf-8")
    with hook(f1, "r") as fp:
        print("hook direct:", fp.read().count("\n"), expect(hook, f1))
    print("hook binary:", exc_name(hook, f1, "rb"))

    calls = []

    def logging_hook(name, mode):
        calls.append(("old", base(name), mode))
        return open(name, mode, encoding="utf-8")

    def kw_hook(name, mode, encoding=None, errors=None):
        calls.append(("kw", base(name), mode, encoding, errors))
        return open(name, mode, encoding=encoding, errors=errors)

    with fileinput.FileInput([f1, f3], openhook=logging_hook) as fi:
        print("custom hook:", list(fi), calls)
    del calls[:]
    with fileinput.FileInput([f1], openhook=kw_hook, encoding="utf-8", errors="strict") as fi:
        print("kw hook:", list(fi), calls)
    del calls[:]
    with fileinput.FileInput([f1], mode="rb", openhook=kw_hook, encoding="utf-8") as fi:
        print("kw hook rb:", expect(list, fi), calls)
    del calls[:]
    with fileinput.FileInput(["-", f1], openhook=logging_hook, encoding=None) as fi:
        old = sys.stdin
        sys.stdin = io.StringIO("")
        try:
            print("hook skipped for stdin:", list(fi), calls)
        finally:
            sys.stdin = old

    gz_file = os.path.join(tmp, "data.gz")
    bz_file = os.path.join(tmp, "data.bz2")
    with gzip.open(gz_file, "wb") as fp:
        fp.write(b"gz1\ngz2\n")
    with bz2.open(bz_file, "wb") as fp:
        fp.write(b"bz1\nbz2\n")

    with fileinput.FileInput([gz_file, bz_file, f1], openhook=fileinput.hook_compressed, encoding="utf-8") as fi:
        print("compressed text:", [(base(fi.filename()), fi.lineno(), fi.filelineno(), l) for l in fi])
    with fileinput.FileInput([gz_file, bz_file, f1], mode="rb", openhook=fileinput.hook_compressed) as fi:
        print("compressed binary:", [(base(fi.filename()), l) for l in fi])
    with fileinput.hook_compressed(gz_file, "rb") as fp:
        print("hook_compressed gz rb:", type(fp).__name__ == "GzipFile", fp.read())
    with fileinput.hook_compressed(bz_file, "r", encoding="utf-8") as fp:
        print("hook_compressed bz2 r:", fp.read())
    with fileinput.hook_compressed(f1, "r", encoding="utf-8") as fp:
        print("hook_compressed plain:", fp.read())
    with fileinput.hook_compressed(f1, "rb") as fp:
        print("hook_compressed plain rb:", fp.read())
    print("hook_compressed bytes ext:", exc_name(fileinput.hook_compressed, os.fsencode(gz_file), "rb"))
    print("hook_compressed missing:", exc_name(fileinput.hook_compressed, missing, "r"))
    print("hook_compressed bad kw:", expect(fileinput.hook_compressed, f1, "r", "utf-8"))

    # -----------------------------------------------------------------------
    # 13. Errors while reading
    # -----------------------------------------------------------------------
    fi = fileinput.FileInput([missing, f1], encoding="utf-8")
    print("open missing:", exc_name(fi.readline), snap(fi)[1:3], len(fi._files))
    print("continue after error:", fi.readline(), snap(fi)[1:])
    fi.close()
    print("iter missing:", exc_name(list, fileinput.FileInput([missing])))

    write_bytes(os.path.join(tmp, "bad.bin"), b"\xff\xfe\n")
    with fileinput.FileInput([os.path.join(tmp, "bad.bin")], encoding="utf-8") as fi:
        print("decode error:", exc_name(list, fi))
    with fileinput.FileInput([os.path.join(tmp, "bad.bin")], encoding="utf-8", errors="replace") as fi:
        print("errors replace:", list(fi))

    # -----------------------------------------------------------------------
    # 14. Subclassing and instance state
    # -----------------------------------------------------------------------
    class Tracing(fileinput.FileInput):
        def __init__(self, *a, **k):
            self.trace = []
            super().__init__(*a, **k)

        def nextfile(self):
            self.trace.append(("nextfile", self._filename is not None))
            return super().nextfile()

        def lineno(self):
            self.trace.append("lineno")
            return super().lineno()

        def close(self):
            self.trace.append("close")
            return super().close()

    t = Tracing([f1, f3], encoding="utf-8")
    print("subclass lines:", list(t))
    print("subclass trace:", t.trace)
    t.close()
    print("subclass vars:", sorted(k for k in vars(t) if k != "trace"))
    print("readline shadow gone:", "_readline" not in vars(t))
    t = Tracing([f1], encoding="utf-8")
    t.readline()
    print("readline shadow set:", "_readline" in vars(t), t._file is not None)
    t.close()
    print("readline shadow removed:", "_readline" not in vars(t), t._file, t._files)

    class Upper(fileinput.FileInput):
        def readline(self):
            return super().readline().upper()

    with Upper([f1], encoding="utf-8") as u:
        print("override readline:", u.readline(), list(u))

    # attribute surface of an unread instance
    fi = fileinput.FileInput([f1], encoding="utf-8")
    print(
        "unread attrs:",
        fi._inplace,
        repr(fi._backup),
        fi._savestdout,
        fi._output,
        fi._filename,
        fi._startlineno,
        fi._filelineno,
        fi._file,
        fi._isstdin,
        fi._backupfilename,
        fi._encoding,
        fi._errors,
        fi._mode,
        fi._write_mode,
        fi._openhook,
    )
    fi.readline()
    print("read attrs:", base(fi._filename), fi._startlineno, fi._filelineno, fi._file is not None, fi._backupfilename)
    fi.close()
    print("method errors:", expect(fi.nextfile, 1), expect(fileinput.FileInput.nextfile))
    print("enter/exit:", fi.__enter__() is fi, fi.__exit__(None, None, None), iter(fi) is fi)
    print("exit arity:", expect(fi.__exit__))

    # context manager closes on error
    try:
        with fileinput.FileInput([f1], encoding="utf-8") as fi:
            fi.readline()
            raise KeyError("boom")
    except KeyError as e:
        print("closed on error:", e, fi._file, fi._files)

    # abandoned inplace instance restores stdout when finalized
    write_bytes(in1, b"gc1\ngc2\n")

    def abandon():
        fi = fileinput.FileInput([in1], inplace=True)
        fi.readline()
        return sys.stdout is not real_stdout

    print("abandoned swapped:", abandon(), file=real_stdout)
    print("finalized restores stdout:", sys.stdout is real_stdout)

    # -----------------------------------------------------------------------
    # 15. fileno edge cases
    # -----------------------------------------------------------------------
    class BadFile:
        def __init__(self, exc):
            self.exc = exc

        def readline(self):
            return ""

        def fileno(self):
            raise self.exc

        def close(self):
            pass

        def __bool__(self):
            return True

    def bad_hook(exc):
        return lambda name, mode: BadFile(exc)

    for exc in (ValueError("v"), OSError("o"), io.UnsupportedOperation("u"), AttributeError("a")):
        with fileinput.FileInput([f1], openhook=bad_hook(exc)) as fi:
            fi.readline()
            print("fileno", type(exc).__name__, expect(fi.fileno))
            fi.nextfile()

    with fileinput.FileInput([f1], encoding="utf-8") as fi:
        print("fileno before:", fi.fileno())
        fi.readline()
        print("fileno open:", fi.fileno() >= 0)
        fi.nextfile()
        print("fileno closed:", fi.fileno())

    # -----------------------------------------------------------------------
    # 16. _test()
    # -----------------------------------------------------------------------
    cwd = os.getcwd()
    os.chdir(tmp)
    try:
        write_bytes(os.path.join(tmp, "t1.txt"), b"one\r\ntwo\n")
        sys.argv = ["fileinput", "t1.txt", "a.txt"]
        fileinput._test()
        print("test state:", fileinput._state is not None)
        fileinput.close()
        sys.argv = ["fileinput", "-i", "-b", ".sav", "t1.txt"]
        buf = io.StringIO()
        fileinput._test()
        print("test inplace:", read_bytes("t1.txt"), read_bytes("t1.txt.sav"))
        fileinput.close()
    finally:
        sys.argv = old_argv
        os.chdir(cwd)

print("done")
