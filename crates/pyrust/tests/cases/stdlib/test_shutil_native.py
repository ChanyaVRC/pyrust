# CPython 3.12.3 parity tests for the native `shutil` port (issue #3486).
#
# Every check prints values that are identical between CPython and PyRust on
# the same platform: exception types, messages without OS-localised text,
# relative paths (the temporary root is replaced by "<root>"), and booleans.

import errno
import io
import os
import pathlib
import shutil
import stat
import sys
import tarfile
import tempfile
import zipfile

IS_WINDOWS = os.name == "nt"


def show(label, *values):
    print(label, *values)


def err_name(exc):
    return type(exc).__name__


# ===========================================================================
# 1. Module surface
# ===========================================================================
print("=== 1. module surface ===")
show("all", shutil.__all__)
show("all-has-disk_usage", "disk_usage" in shutil.__all__)
names = set(dir(shutil))
for name in (
    "COPY_BUFSIZE", "Error", "ExecError", "ReadError", "RegistryError",
    "SameFileError", "SpecialFileError", "_ARCHIVE_FORMATS", "_UNPACK_FORMATS",
    "_WINDOWS", "_WIN_DEFAULT_PATHEXT", "_samefile", "_basename",
    "_destinsrc", "_make_tarball", "_make_zipfile", "_unpack_tarfile",
    "_unpack_zipfile", "_find_unpack_format", "_copytree", "_rmtree_unsafe",
    "_use_fd_functions", "os", "sys", "stat", "fnmatch", "errno",
    "chown", "copy", "copy2", "copyfile", "copyfileobj", "copymode",
    "copystat", "copytree", "disk_usage", "get_archive_formats",
    "get_terminal_size", "get_unpack_formats", "ignore_patterns",
    "make_archive", "move", "register_archive_format",
    "register_unpack_format", "rmtree", "unpack_archive",
    "unregister_archive_format", "unregister_unpack_format", "which",
):
    if name not in names:
        show("missing-name", name)
show("no-usage-attr", hasattr(shutil, "usage"))
show("COPY_BUFSIZE", shutil.COPY_BUFSIZE == (1024 * 1024 if IS_WINDOWS else 64 * 1024))
show("_WINDOWS", shutil._WINDOWS == IS_WINDOWS)
show("_WIN_DEFAULT_PATHEXT", shutil._WIN_DEFAULT_PATHEXT)
show("supported", shutil._ZLIB_SUPPORTED, shutil._BZ2_SUPPORTED, shutil._LZMA_SUPPORTED)
show("avoids_symlink_attacks", shutil.rmtree.avoids_symlink_attacks == (not IS_WINDOWS))
show("use_fd_functions", shutil._use_fd_functions == (not IS_WINDOWS))
show("supports_root_dir", shutil._make_tarball.supports_root_dir, shutil._make_zipfile.supports_root_dir)
show("module-os", shutil.os is os, shutil.sys is sys, shutil.stat is stat)
show("doc-module", shutil.__doc__.splitlines()[0])
for fn in (shutil.copy, shutil.copyfile, shutil.rmtree, shutil.move, shutil.which,
           shutil.make_archive, shutil.get_terminal_size, shutil.copyfileobj):
    show("doc", fn.__name__, fn.__doc__.splitlines()[0])
show("doc-none", shutil._samefile.__doc__, shutil.unregister_archive_format.__doc__)
show("names", shutil.copy.__name__, shutil.move.__name__, shutil.copytree.__qualname__)


# ===========================================================================
# 2. Exceptions
# ===========================================================================
print("=== 2. exceptions ===")
show("Error<OSError", issubclass(shutil.Error, OSError))
show("SameFileError<Error", issubclass(shutil.SameFileError, shutil.Error))
show("SameFileError<OSError", issubclass(shutil.SameFileError, OSError))
show("SpecialFileError<OSError", issubclass(shutil.SpecialFileError, OSError),
     issubclass(shutil.SpecialFileError, shutil.Error))
show("ExecError<OSError", issubclass(shutil.ExecError, OSError))
show("ReadError<OSError", issubclass(shutil.ReadError, OSError))
show("RegistryError<Exception", issubclass(shutil.RegistryError, Exception),
     issubclass(shutil.RegistryError, OSError))
for cls in (shutil.Error, shutil.SameFileError, shutil.SpecialFileError,
            shutil.ExecError, shutil.ReadError, shutil.RegistryError):
    show("exc", cls.__name__, cls.__module__, cls.__qualname__,
         [c.__name__ for c in cls.__mro__])
show("exc-doc", shutil.Error.__doc__, repr(shutil.SameFileError.__doc__))
show("exc-doc-registry", repr(shutil.RegistryError.__doc__))
e = shutil.Error([("a", "b", "why")])
show("Error-args", e.args, isinstance(e, OSError))
try:
    raise shutil.SameFileError("x and y")
except shutil.Error as exc:
    show("catch-SameFileError-as-Error", err_name(exc), str(exc))


# ===========================================================================
# 3. Argument binding errors (CPython `def` wording)
# ===========================================================================
print("=== 3. argument errors ===")


def call_error(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except TypeError as exc:
        show("TypeError", str(exc))
    else:
        show("no-error", fn.__name__)


call_error(shutil.copy)
call_error(shutil.copyfile, "a")
call_error(shutil.copy2, "a", "b", True)
call_error(shutil.copyfile, "a", "b", follow=False)
call_error(shutil.copyfileobj, io.BytesIO(), io.BytesIO(), 1, 2)
call_error(shutil.rmtree)
call_error(shutil.rmtree, "x", path="y")
call_error(shutil.move, "a")
call_error(shutil.which)
call_error(shutil.disk_usage)
call_error(shutil.ignore_patterns, "*.py", key=1)
call_error(shutil.copytree, "a")
call_error(shutil.make_archive, "a")
call_error(shutil.unpack_archive)
call_error(shutil.get_terminal_size, (1, 2), 3)
call_error(shutil.get_archive_formats, 1)
call_error(shutil.register_unpack_format, "x", [".x"])


# ===========================================================================
# 4. copyfileobj
# ===========================================================================
print("=== 4. copyfileobj ===")
src = io.BytesIO(b"0123456789" * 10)
dst = io.BytesIO()
shutil.copyfileobj(src, dst, 7)
show("bytes", dst.getvalue() == b"0123456789" * 10)
dst = io.StringIO()
shutil.copyfileobj(io.StringIO("alpha\nbeta\n"), dst)
show("text", repr(dst.getvalue()))


class Recorder:
    def __init__(self, data):
        self.data = data
        self.reads = []
        self.writes = []

    def read(self, n):
        self.reads.append(n)
        chunk, self.data = self.data[:n], self.data[n:]
        return chunk

    def write(self, chunk):
        self.writes.append(chunk)


rec = Recorder(b"abcdefgh")
shutil.copyfileobj(rec, rec, 3)
show("reads", rec.reads, rec.writes)
rec = Recorder(b"xy")
shutil.copyfileobj(rec, rec)
show("default-length", rec.reads == [shutil.COPY_BUFSIZE, shutil.COPY_BUFSIZE], rec.writes)
rec = Recorder(b"xy")
shutil.copyfileobj(rec, rec, 0)
show("zero-length", rec.reads[0] == shutil.COPY_BUFSIZE)


# ===========================================================================
# Filesystem tests
# ===========================================================================
ROOT = tempfile.mkdtemp()
ROOT_REAL = os.path.realpath(ROOT)
CWD = os.getcwd()


def rel(path):
    if isinstance(path, bytes):
        path = os.fsdecode(path)
    path = os.fspath(path)
    for prefix in (ROOT_REAL, ROOT):
        if path.startswith(prefix):
            path = "<root>" + path[len(prefix):]
            break
    return path.replace("\\", "/")


def scrub(text):
    text = str(text)
    for prefix in (ROOT_REAL, ROOT):
        text = text.replace(prefix.replace("\\", "\\\\"), "<root>")
        text = text.replace(prefix, "<root>")
        text = text.replace(prefix.replace("\\", "/"), "<root>")
    text = text.replace(CWD, "<cwd>")
    return text.replace("\\\\", "/").replace("\\", "/")


def write(path, data=b"data"):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def read(path):
    with open(path, "rb") as f:
        return f.read()


def tree(base):
    out = []
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames.sort()
        r = os.path.relpath(dirpath, base).replace("\\", "/")
        for name in sorted(filenames):
            out.append((r + "/" + name, read(os.path.join(dirpath, name))))
        if not filenames and not dirnames:
            out.append((r + "/", None))
    return sorted(out)


def symlinks_supported():
    target = os.path.join(ROOT, "symlink_probe_target")
    link = os.path.join(ROOT, "symlink_probe_link")
    write(target)
    try:
        os.symlink(target, link)
    except (OSError, NotImplementedError, AttributeError):
        return False
    finally:
        os.unlink(target)
    os.unlink(link)
    return True


CAN_SYMLINK = symlinks_supported()

try:
    # =======================================================================
    # 5. copyfile
    # =======================================================================
    print("=== 5. copyfile ===")
    base = os.path.join(ROOT, "copyfile")
    src = os.path.join(base, "src.bin")
    write(src, b"payload\x00\xff")
    dst = os.path.join(base, "dst.bin")
    ret = shutil.copyfile(src, dst)
    show("ret-is-dst", ret is dst, read(dst) == b"payload\x00\xff")
    pdst = pathlib.Path(base) / "pathlike.bin"
    ret = shutil.copyfile(pathlib.Path(src), pdst)
    show("ret-pathlike", ret is pdst, read(pdst) == b"payload\x00\xff")
    bdst = os.fsencode(os.path.join(base, "bytes.bin"))
    ret = shutil.copyfile(os.fsencode(src), bdst)
    show("ret-bytes", type(ret).__name__, ret is bdst)
    # Overwrite an existing file.
    write(dst, b"old contents that are longer")
    shutil.copyfile(src, dst)
    show("overwrite", read(dst) == b"payload\x00\xff")
    # Empty file.
    empty = os.path.join(base, "empty")
    write(empty, b"")
    shutil.copyfile(empty, os.path.join(base, "empty2"))
    show("empty", read(os.path.join(base, "empty2")))
    try:
        shutil.copyfile(src, src)
    except shutil.SameFileError as exc:
        show("SameFileError", err_name(exc), isinstance(exc, shutil.Error), scrub(exc))
    try:
        shutil.copyfile(pathlib.Path(src), src)
    except shutil.SameFileError as exc:
        show("SameFileError-pathlike", scrub(exc).replace("WindowsPath", "Path").replace("PosixPath", "Path"))
    try:
        shutil.copyfile(os.path.join(base, "missing"), dst)
    except OSError as exc:
        show("missing-src", err_name(exc), rel(exc.filename))
    try:
        shutil.copyfile(src, os.path.join(base, "no_dir", "x"))
    except OSError as exc:
        show("missing-dst-dir", err_name(exc), rel(exc.filename))
    try:
        shutil.copyfile(src, base)
    except OSError as exc:
        show("dst-is-dir", err_name(exc), isinstance(exc, OSError))
    try:
        shutil.copyfile(base, dst)
    except OSError as exc:
        show("src-is-dir", err_name(exc), isinstance(exc, OSError))

    # =======================================================================
    # 6. copymode / copystat / copy / copy2
    # =======================================================================
    print("=== 6. copymode / copystat ===")
    base = os.path.join(ROOT, "meta")
    src = os.path.join(base, "src.txt")
    write(src, b"meta")
    dst = os.path.join(base, "dst.txt")
    write(dst, b"other")
    os.utime(src, (1_000_000_000, 1_100_000_000))
    if IS_WINDOWS:
        os.chmod(src, stat.S_IREAD)
        shutil.copymode(src, dst)
        show("copymode-readonly", not (os.stat(dst).st_mode & stat.S_IWRITE))
        os.chmod(dst, stat.S_IREAD | stat.S_IWRITE)
        os.chmod(src, stat.S_IREAD | stat.S_IWRITE)
    else:
        os.chmod(src, 0o640)
        os.chmod(dst, 0o600)
        shutil.copymode(src, dst)
        show("copymode-posix", oct(stat.S_IMODE(os.stat(dst).st_mode)))
        os.chmod(src, 0o604)
    ret = shutil.copystat(src, dst)
    show("copystat-ret", ret)
    st = os.stat(dst)
    show("copystat-times", int(st.st_atime), int(st.st_mtime))
    if not IS_WINDOWS:
        show("copystat-mode", oct(stat.S_IMODE(st.st_mode)))
    show("copymode-ret", shutil.copymode(src, dst))
    try:
        shutil.copymode(os.path.join(base, "missing"), dst)
    except OSError as exc:
        show("copymode-missing", err_name(exc), rel(exc.filename))
    try:
        shutil.copystat(os.path.join(base, "missing"), dst)
    except OSError as exc:
        show("copystat-missing", err_name(exc), rel(exc.filename))

    print("=== 6b. copy / copy2 ===")
    ddir = os.path.join(base, "into")
    os.mkdir(ddir)
    ret = shutil.copy(src, ddir)
    show("copy-into-dir", rel(ret), read(ret) == b"meta")
    show("copy-no-times", int(os.stat(ret).st_mtime) != 1_100_000_000)
    ret = shutil.copy2(src, os.path.join(base, "copy2.txt"))
    show("copy2-times", rel(ret), int(os.stat(ret).st_mtime))
    ret = shutil.copy2(pathlib.Path(src), pathlib.Path(ddir))
    show("copy2-pathlike-dir", type(ret).__name__, rel(ret))
    ret = shutil.copy(os.fsencode(src), os.fsencode(os.path.join(base, "b.txt")))
    show("copy-bytes", type(ret).__name__, rel(ret))
    ret = shutil.copy2(os.fsencode(src), os.fsencode(ddir))
    show("copy2-bytes-dir", type(ret).__name__, rel(ret))
    pdst = pathlib.Path(base) / "p.txt"
    ret = shutil.copy(src, pdst)
    show("copy-pathlike-ret", ret is pdst)
    # (CPython's Windows copy2 goes through _winapi.CopyFile2, whose error
    # paths differ; only copy() is checked for the same-file case.)
    try:
        shutil.copy(src, src)
    except shutil.SameFileError as exc:
        show("copy-same", err_name(exc))
    try:
        shutil.copy(src, ddir + os.sep)
    except shutil.SameFileError as exc:
        show("copy-same-in-dir", err_name(exc))
    else:
        show("copy-into-dir-again", True)

    # =======================================================================
    # 7. Symlinks (copyfile / copymode / copystat / copytree / rmtree)
    # =======================================================================
    print("=== 7. symlinks ===")
    if CAN_SYMLINK:
        base = os.path.join(ROOT, "links")
        target = os.path.join(base, "target.txt")
        write(target, b"target")
        link = os.path.join(base, "link.txt")
        os.symlink(target, link)
        out = os.path.join(base, "copied_link.txt")
        shutil.copyfile(link, out, follow_symlinks=False)
        show("copyfile-nofollow", os.path.islink(out), os.readlink(out) == target)
        out2 = os.path.join(base, "copied_target.txt")
        shutil.copyfile(link, out2)
        show("copyfile-follow", os.path.islink(out2), read(out2))
        out3 = shutil.copy2(link, os.path.join(base, "copy2_link.txt"), follow_symlinks=False)
        show("copy2-nofollow", os.path.islink(out3))
        shutil.copymode(link, out, follow_symlinks=False)
        shutil.copystat(link, out, follow_symlinks=False)
        show("copystat-nofollow-ok", True)
        lsrc = os.path.join(base, "tree")
        write(os.path.join(lsrc, "real.txt"), b"real")
        os.symlink(os.path.join(lsrc, "real.txt"), os.path.join(lsrc, "ln.txt"))
        os.symlink(os.path.join(base, "nowhere"), os.path.join(lsrc, "dangling"))
        ltree = shutil.copytree(lsrc, os.path.join(base, "tree_links"), symlinks=True)
        show("copytree-symlinks", sorted(os.listdir(ltree)),
             os.path.islink(os.path.join(ltree, "ln.txt")))
        try:
            shutil.copytree(lsrc, os.path.join(base, "tree_follow"))
        except shutil.Error as exc:
            show("copytree-dangling", [(rel(a), rel(b)) for a, b, _ in exc.args[0]])
        ftree = shutil.copytree(lsrc, os.path.join(base, "tree_follow2"),
                                ignore_dangling_symlinks=True)
        show("copytree-ignore-dangling", sorted(os.listdir(ftree)),
             os.path.islink(os.path.join(ftree, "ln.txt")))
        ldir = os.path.join(base, "dirlink")
        os.symlink(lsrc, ldir, target_is_directory=True)
        try:
            shutil.rmtree(ldir)
        except OSError as exc:
            show("rmtree-symlink", err_name(exc), str(exc))
        calls = []
        shutil.rmtree(ldir, onexc=lambda f, p, e: calls.append((f is os.path.islink, rel(p), str(e))))
        show("rmtree-symlink-onexc", calls, os.path.isdir(lsrc))
        os.unlink(ldir)
        # move a symlink
        moved = shutil.move(link, os.path.join(base, "moved_link.txt"))
        show("move-symlink", os.path.islink(moved), os.path.exists(link))
    else:
        show("symlinks", "unavailable")

    # =======================================================================
    # 8. ignore_patterns / copytree
    # =======================================================================
    print("=== 8. ignore_patterns ===")
    ign = shutil.ignore_patterns("*.pyc", "tmp*")
    result = ign("somewhere", ["a.py", "a.pyc", "tmpfile", "b.txt", "tmp", "c.pyc"])
    show("ignore-result", type(result).__name__, sorted(result))
    show("ignore-empty", shutil.ignore_patterns()("x", ["a"]))
    show("ignore-name", ign.__name__, ign.__qualname__)
    show("ignore-callable", callable(ign))

    print("=== 8b. copytree ===")
    base = os.path.join(ROOT, "trees")
    src = os.path.join(base, "src")
    write(os.path.join(src, "a.txt"), b"a")
    write(os.path.join(src, "b.pyc"), b"b")
    write(os.path.join(src, "sub", "c.txt"), b"c")
    write(os.path.join(src, "sub", "deep", "d.txt"), b"d")
    os.makedirs(os.path.join(src, "emptydir"))
    dst = os.path.join(base, "dst")
    ret = shutil.copytree(src, dst)
    show("copytree-ret", ret is dst, tree(dst) == tree(src))
    show("copytree-tree", tree(dst))
    try:
        shutil.copytree(src, dst)
    except FileExistsError as exc:
        show("copytree-exists", err_name(exc), rel(exc.filename))
    write(os.path.join(dst, "extra.txt"), b"extra")
    write(os.path.join(src, "a.txt"), b"A2")
    ret = shutil.copytree(src, dst, dirs_exist_ok=True)
    show("copytree-dirs-exist-ok", read(os.path.join(dst, "a.txt")),
         os.path.exists(os.path.join(dst, "extra.txt")))
    seen = []

    def record_ignore(path, names):
        seen.append((rel(path), type(path).__name__, sorted(names)))
        return {"b.pyc", "deep"}

    shutil.copytree(pathlib.Path(src), pathlib.Path(base) / "ign", ignore=record_ignore)
    show("copytree-ignore-calls", sorted(seen))
    show("copytree-ignore-tree", tree(os.path.join(base, "ign")))
    shutil.copytree(src, os.path.join(base, "pat"), ignore=shutil.ignore_patterns("*.pyc", "deep"))
    show("copytree-ignore-patterns", tree(os.path.join(base, "pat")))
    copied = []

    def custom_copy(s, d):
        copied.append((type(s).__name__, rel(s), rel(d)))
        return shutil.copy(s, d)

    shutil.copytree(src, os.path.join(base, "custom"), copy_function=custom_copy)
    show("copytree-custom", sorted(copied))
    failures = []

    def failing_copy(s, d):
        if s.endswith("c.txt"):
            raise OSError("cannot copy " + os.path.basename(s))
        if s.endswith("d.txt"):
            raise shutil.Error([("x", "y", "nested failure")])
        return shutil.copy2(s, d)

    try:
        shutil.copytree(src, os.path.join(base, "failing"), copy_function=failing_copy)
    except shutil.Error as exc:
        errors = exc.args[0]
        show("copytree-errors", type(errors).__name__,
             sorted((rel(a), rel(b), scrub(w)) for a, b, w in errors))
    try:
        shutil.copytree(os.path.join(base, "missing"), os.path.join(base, "never"))
    except OSError as exc:
        show("copytree-missing", err_name(exc), rel(exc.filename))
    show("copytree-missing-no-dst", os.path.exists(os.path.join(base, "never")))
    ret = shutil.copytree(os.fsencode(src), os.fsencode(os.path.join(base, "bytes")))
    show("copytree-bytes", type(ret).__name__, tree(os.path.join(base, "bytes")) == tree(src))
    entries = list(os.scandir(src))
    ret = shutil._copytree(entries, src, os.path.join(base, "private"), False, None,
                           shutil.copy2, False)
    show("_copytree", rel(ret), tree(ret) == tree(src))

    # =======================================================================
    # 9. rmtree
    # =======================================================================
    print("=== 9. rmtree ===")
    FUNCS = {}
    for name in ("lstat", "open", "scandir", "unlink", "rmdir", "close"):
        FUNCS[name] = getattr(os, name)

    def func_name(f):
        if f is os.path.islink:
            return "os.path.islink"
        for name, fn in FUNCS.items():
            if f is fn:
                return "os." + name
        return repr(f)

    base = os.path.join(ROOT, "rm")
    victim = os.path.join(base, "victim")
    write(os.path.join(victim, "a.txt"))
    write(os.path.join(victim, "sub", "b.txt"))
    os.makedirs(os.path.join(victim, "sub", "empty"))
    show("rmtree-ret", shutil.rmtree(victim), os.path.exists(victim))
    write(os.path.join(victim, "a.txt"))
    shutil.rmtree(pathlib.Path(victim))
    show("rmtree-pathlike", os.path.exists(victim))
    write(os.path.join(victim, "a.txt"))
    shutil.rmtree(os.fsencode(victim))
    show("rmtree-bytes", os.path.exists(victim))

    missing = os.path.join(base, "missing")
    try:
        shutil.rmtree(missing)
    except FileNotFoundError as exc:
        show("rmtree-missing", err_name(exc), rel(exc.filename))
    shutil.rmtree(missing, ignore_errors=True)
    show("rmtree-ignore-errors", True)
    calls = []
    shutil.rmtree(missing, onexc=lambda f, p, e: calls.append((func_name(f), rel(p), err_name(e))))
    show("rmtree-onexc", calls)
    calls = []
    shutil.rmtree(pathlib.Path(missing),
                  onexc=lambda f, p, e: calls.append((type(p).__name__, err_name(e))))
    show("rmtree-onexc-pathlike", [(t.replace("Windows", "").replace("Posix", ""), e) for t, e in calls])
    calls = []

    def onerror(f, p, exc_info):
        calls.append((func_name(f), rel(p), exc_info[0].__name__,
                      exc_info[0] is type(exc_info[1]),
                      exc_info[2] is exc_info[1].__traceback__))

    shutil.rmtree(missing, onerror=onerror)
    show("rmtree-onerror", calls)
    calls = []
    shutil.rmtree(missing, onerror=lambda *a: calls.append("onerror"),
                  onexc=lambda *a: calls.append("onexc"))
    show("rmtree-onexc-wins", calls)
    calls = []
    shutil.rmtree(missing, ignore_errors=True, onexc=lambda *a: calls.append("onexc"))
    show("rmtree-ignore-wins", calls)

    afile = os.path.join(base, "plain.txt")
    write(afile)
    calls = []
    shutil.rmtree(afile, onexc=lambda f, p, e: calls.append((func_name(f), rel(p), err_name(e))))
    show("rmtree-file", calls, os.path.exists(afile))
    try:
        shutil.rmtree(afile)
    except OSError as exc:
        show("rmtree-file-raise", err_name(exc))

    # Errors raised by the hook propagate.
    def boom(f, p, e):
        raise RuntimeError("hook says no")

    try:
        shutil.rmtree(missing, onexc=boom)
    except RuntimeError as exc:
        show("rmtree-hook-raises", str(exc))

    # Read-only file: Windows refuses to unlink it; the hook clears the bit.
    ro = os.path.join(base, "ro")
    write(os.path.join(ro, "locked.txt"))
    os.chmod(os.path.join(ro, "locked.txt"), stat.S_IREAD)
    handled = []

    def make_writable(f, p, e):
        handled.append((func_name(f), rel(p), err_name(e)))
        os.chmod(p, stat.S_IWRITE | stat.S_IREAD)
        f(p)

    shutil.rmtree(ro, onexc=make_writable)
    # (Whether Windows refuses to unlink a read-only file is the os module's
    # business; only the hook's arguments are checked here.)
    show("rmtree-readonly",
         all(h == ("os.unlink", "<root>/rm/ro/locked.txt", "PermissionError") for h in handled),
         os.path.exists(ro))

    if IS_WINDOWS:
        try:
            shutil.rmtree(missing, dir_fd=0)
        except NotImplementedError as exc:
            show("rmtree-dir_fd", err_name(exc), str(exc))
    else:
        write(os.path.join(base, "fdtree", "x", "y.txt"))
        fd = os.open(base, os.O_RDONLY)
        try:
            shutil.rmtree("fdtree", dir_fd=fd)
        finally:
            os.close(fd)
        show("rmtree-dir_fd", os.path.exists(os.path.join(base, "fdtree")))

    calls = []
    unsafe_dir = os.path.join(base, "unsafe")
    write(os.path.join(unsafe_dir, "f.txt"))
    shutil._rmtree_unsafe(unsafe_dir, lambda *a: calls.append(a))
    show("_rmtree_unsafe", calls, os.path.exists(unsafe_dir))
    show("_rmtree_islink", bool(shutil._rmtree_islink(base)), bool(shutil._rmtree_islink(missing)))

    # =======================================================================
    # 10. move
    # =======================================================================
    print("=== 10. move ===")
    base = os.path.join(ROOT, "mv")
    a = os.path.join(base, "a.txt")
    write(a, b"A")
    b = os.path.join(base, "b.txt")
    ret = shutil.move(a, b)
    show("move-file", ret is b, os.path.exists(a), read(b))
    into = os.path.join(base, "into")
    os.mkdir(into)
    ret = shutil.move(b, into)
    show("move-into-dir", rel(ret), os.path.exists(b))
    write(b, b"B2")
    try:
        shutil.move(b, into)
    except shutil.Error as exc:
        show("move-exists", err_name(exc), scrub(exc))
    d1 = os.path.join(base, "d1")
    write(os.path.join(d1, "x.txt"), b"x")
    ret = shutil.move(d1, os.path.join(base, "d2"))
    show("move-dir", rel(ret), tree(ret))
    ret = shutil.move(os.path.join(base, "d2"), into + os.sep)
    show("move-dir-into-dir", rel(ret), sorted(os.listdir(into)))
    ret = shutil.move(pathlib.Path(b), pathlib.Path(base) / "c.txt")
    show("move-pathlike", isinstance(ret, pathlib.Path), os.path.exists(os.path.join(base, "c.txt")))
    ret = shutil.move(os.fsencode(os.path.join(base, "c.txt")), os.fsencode(os.path.join(base, "d.txt")))
    show("move-bytes", type(ret).__name__)
    self_dir = os.path.join(base, "self")
    write(os.path.join(self_dir, "f.txt"))
    try:
        shutil.move(self_dir, os.path.join(self_dir, "inner"))
    except shutil.Error as exc:
        show("move-into-self", err_name(exc), scrub(exc))
    show("_destinsrc", shutil._destinsrc(self_dir, os.path.join(self_dir, "x")),
         shutil._destinsrc(self_dir, base))
    show("_basename", shutil._basename(os.path.join("x", "y", "")), shutil._basename("z"))

    # Cross-device fallback: make os.rename fail.
    real_rename = os.rename
    copies = []

    def no_rename(s, d):
        raise OSError(errno.EXDEV, "Invalid cross-device link")

    def tracking_copy(s, d):
        copies.append((rel(s), rel(d)))
        return shutil.copy2(s, d)

    os.rename = no_rename
    try:
        f1 = os.path.join(base, "cross.txt")
        write(f1, b"cross")
        ret = shutil.move(f1, os.path.join(base, "crossed.txt"), copy_function=tracking_copy)
        show("move-cross-file", rel(ret), os.path.exists(f1), read(ret))
        cdir = os.path.join(base, "crossdir")
        write(os.path.join(cdir, "in.txt"), b"in")
        ret = shutil.move(cdir, os.path.join(base, "crosseddir"), copy_function=tracking_copy)
        show("move-cross-dir", rel(ret), os.path.exists(cdir), tree(ret))
        show("move-cross-copies", copies)
    finally:
        os.rename = real_rename

    # =======================================================================
    # 11. disk_usage
    # =======================================================================
    print("=== 11. disk_usage ===")
    usage = shutil.disk_usage(ROOT)
    show("usage-type", type(usage).__name__, type(usage).__module__, usage._fields)
    show("usage-tuple", isinstance(usage, tuple), len(usage))
    total, used, free = usage
    show("usage-fields", usage.total == total, usage.used == used, usage.free == free)
    show("usage-ints", all(type(v) is int for v in usage))
    show("usage-order", total > 0, total >= used, total >= free, used >= 0, free >= 0)
    show("usage-repr", repr(usage).startswith("usage(total="))
    show("usage-asdict", list(usage._asdict()))
    show("usage-replace", usage._replace(total=1).total)
    fpath = pathlib.Path(ROOT) / "probe.txt"
    write(str(fpath))
    show("usage-file", shutil.disk_usage(fpath).total > 0)
    show("usage-bytes", shutil.disk_usage(os.fsencode(ROOT)).total > 0)
    show("usage-ntuple", type(usage) is shutil._ntuple_diskusage)
    try:
        shutil.disk_usage(os.path.join(ROOT, "missing_dir_3486"))
    except FileNotFoundError as exc:
        show("usage-missing", err_name(exc))

    # =======================================================================
    # 12. which
    # =======================================================================
    print("=== 12. which ===")
    base = os.path.join(ROOT, "bin")
    os.mkdir(base)
    other = os.path.join(ROOT, "bin2")
    os.mkdir(other)
    if IS_WINDOWS:
        saved_pathext = os.environ.get("PATHEXT")
        os.environ["PATHEXT"] = ".EXE;.BAT"
        try:
            write(os.path.join(base, "tool.EXE"), b"exe")
            write(os.path.join(base, "tool.BAT"), b"bat")
            write(os.path.join(base, "plain"), b"no ext")
            write(os.path.join(other, "tool.BAT"), b"bat")
            show("which-ext", rel(shutil.which("tool", path=base)))
            show("which-explicit-ext", rel(shutil.which("tool.bat", path=base)))
            show("which-noext-exec", rel(shutil.which("plain", path=base)))
            show("which-noext-fok", rel(shutil.which("plain", mode=os.F_OK, path=base)))
            show("which-pathsep", rel(shutil.which("tool", path=other + os.pathsep + base)))
            show("which-bytes", rel(shutil.which(b"tool", path=os.fsencode(base))),
                 type(shutil.which(b"tool", path=os.fsencode(base))).__name__)
            show("which-direct", rel(shutil.which(os.path.join(base, "tool"))))
            show("which-pathlike-path", rel(shutil.which("tool", path=pathlib.Path(base))))
            # The current directory is searched first unless
            # NoDefaultCurrentDirectoryInExePath is set (and always for a
            # non-executable lookup).
            nodefault = "NoDefaultCurrentDirectoryInExePath"
            saved_nodefault = os.environ.get(nodefault)
            here = os.getcwd()
            os.chdir(other)
            try:
                if nodefault in os.environ:
                    del os.environ[nodefault]
                show("which-curdir-first", rel(shutil.which("tool", path=base)))
                os.environ[nodefault] = "1"
                show("which-no-curdir", rel(shutil.which("tool", path=base)))
                show("which-no-curdir-fok", rel(shutil.which("tool.BAT", mode=os.F_OK, path=base)))
                show("_win_path_needs_curdir", shutil._win_path_needs_curdir("tool", os.X_OK),
                     shutil._win_path_needs_curdir("tool", os.F_OK))
            finally:
                os.chdir(here)
                if saved_nodefault is None:
                    if nodefault in os.environ:
                        del os.environ[nodefault]
                else:
                    os.environ[nodefault] = saved_nodefault
        finally:
            if saved_pathext is None:
                del os.environ["PATHEXT"]
            else:
                os.environ["PATHEXT"] = saved_pathext
    else:
        tool = os.path.join(base, "tool")
        write(tool, b"#!/bin/sh\n")
        os.chmod(tool, 0o644)
        show("which-not-exec", shutil.which("tool", path=base))
        show("which-fok", rel(shutil.which("tool", mode=os.F_OK, path=base)))
        os.chmod(tool, 0o755)
        show("which-exec", rel(shutil.which("tool", path=base)))
        write(os.path.join(other, "tool"), b"#!/bin/sh\n")
        os.chmod(os.path.join(other, "tool"), 0o755)
        show("which-pathsep", rel(shutil.which("tool", path=other + os.pathsep + base)))
        show("which-bytes", rel(shutil.which(b"tool", path=os.fsencode(base))),
             type(shutil.which(b"tool", path=os.fsencode(base))).__name__)
        show("which-direct", rel(shutil.which(tool)))
        show("which-pathlike-path", rel(shutil.which("tool", path=pathlib.Path(base))))
        here = os.getcwd()
        os.chdir(base)
        try:
            show("which-relative-direct", shutil.which("./tool"))
            show("which-no-curdir", rel(shutil.which("tool", path=other)))
        finally:
            os.chdir(here)
    os.mkdir(os.path.join(base, "adir"))
    show("which-dir", shutil.which("adir", mode=os.F_OK, path=base))
    show("which-empty-path", shutil.which("tool", path=""))
    show("which-missing", shutil.which("no_such_tool_3486", path=base))
    show("which-missing-direct", shutil.which(os.path.join(base, "nope")))

    # =======================================================================
    # 13. chown
    # =======================================================================
    print("=== 13. chown ===")
    target = os.path.join(ROOT, "owned.txt")
    write(target)
    try:
        shutil.chown(target)
    except ValueError as exc:
        show("chown-none", str(exc))
    try:
        shutil.chown(target, user="no_such_user_3486")
    except LookupError as exc:
        show("chown-user", err_name(exc), str(exc))
    try:
        shutil.chown(target, group="no_such_group_3486")
    except LookupError as exc:
        show("chown-group", err_name(exc), str(exc))
    if hasattr(os, "chown"):
        st = os.stat(target)
        show("chown-self", shutil.chown(target, user=st.st_uid, group=st.st_gid))
        show("chown-self-group", shutil.chown(pathlib.Path(target), group=st.st_gid))
    else:
        try:
            shutil.chown(target, user=0)
        except AttributeError as exc:
            show("chown-no-os-chown", err_name(exc))
    show("_get_uid-none", shutil._get_uid(None), shutil._get_gid(None))
    show("_get_uid-missing", shutil._get_uid("no_such_user_3486"),
         shutil._get_gid("no_such_group_3486"))

    # =======================================================================
    # 14. Archive registries
    # =======================================================================
    print("=== 14. registries ===")
    show("archive-formats", shutil.get_archive_formats())
    show("unpack-formats", shutil.get_unpack_formats())
    show("registry-dicts", list(shutil._ARCHIVE_FORMATS), list(shutil._UNPACK_FORMATS))
    show("registry-entries", shutil._ARCHIVE_FORMATS["tar"][0] is shutil._make_tarball,
         shutil._ARCHIVE_FORMATS["zip"][0] is shutil._make_zipfile,
         shutil._ARCHIVE_FORMATS["gztar"][1], shutil._ARCHIVE_FORMATS["zip"][1],
         shutil._UNPACK_FORMATS["zip"][1] is shutil._unpack_zipfile,
         shutil._UNPACK_FORMATS["xztar"][0])
    for value, label in ((3, "int"), ("text", "str")):
        try:
            shutil.register_archive_format("bad", value)
        except TypeError as exc:
            show("register-not-callable", label, str(exc))
    try:
        shutil.register_archive_format("bad", len, extra_args=5)
    except TypeError as exc:
        show("register-extra-args", str(exc))
    try:
        shutil.register_archive_format("bad", len, extra_args=[("a", 1, 2)])
    except TypeError as exc:
        show("register-extra-pair", str(exc))
    try:
        shutil.register_archive_format("bad", len, extra_args=["ab"])
    except TypeError as exc:
        show("register-extra-str", str(exc))
    show("bad-not-registered", "bad" in dict(shutil.get_archive_formats()))

    def fake_archiver(base_name, base_dir, **kwargs):
        return ("archived", rel(base_name), base_dir, sorted(kwargs))

    shutil.register_archive_format("fake", fake_archiver, [("flavour", "x")], "Fake format")
    show("fake-listed", dict(shutil.get_archive_formats())["fake"],
         shutil._ARCHIVE_FORMATS["fake"][1])
    shutil._ARCHIVE_FORMATS["direct"] = (fake_archiver, [], "added directly")
    show("direct-listed", dict(shutil.get_archive_formats()).get("direct"))
    del shutil._ARCHIVE_FORMATS["direct"]
    show("fake-make", shutil.make_archive(os.path.join(ROOT, "fake"), "fake", dry_run=1))
    shutil.unregister_archive_format("fake")
    try:
        shutil.unregister_archive_format("fake")
    except KeyError as exc:
        show("unregister-missing", err_name(exc), str(exc))

    try:
        shutil.register_unpack_format("dupe", [".zip"], len)
    except shutil.RegistryError as exc:
        show("register-dupe", str(exc))
    try:
        shutil.register_unpack_format("nc", [".nc"], 42)
    except TypeError as exc:
        show("register-unpack-not-callable", str(exc))
    unpacked = []

    def fake_unpacker(filename, extract_dir, **kwargs):
        unpacked.append((rel(filename), rel(extract_dir), sorted(kwargs.items())))

    shutil.register_unpack_format("fakeu", [".fku", ".fk2"], fake_unpacker, [("mode", 1)], "Fake unpack")
    show("fakeu-listed", [f for f in shutil.get_unpack_formats() if f[0] == "fakeu"])
    show("find-format", shutil._find_unpack_format("a.tar.gz"), shutil._find_unpack_format("x.fk2"),
         shutil._find_unpack_format("x.unknown"))
    shutil.unpack_archive(os.path.join(ROOT, "x.fku"), os.path.join(ROOT, "out"))
    shutil.unpack_archive(os.path.join(ROOT, "x.fk2"), pathlib.Path(ROOT) / "out2", filter="data")
    shutil.unpack_archive(os.path.join(ROOT, "x.bin"), os.path.join(ROOT, "out3"), format="fakeu")
    here = os.getcwd()
    os.chdir(ROOT)
    try:
        shutil.unpack_archive("y.fku")
    finally:
        os.chdir(here)
    show("fakeu-calls", unpacked)
    shutil.unregister_unpack_format("fakeu")
    try:
        shutil.unregister_unpack_format("fakeu")
    except KeyError as exc:
        show("unregister-unpack-missing", err_name(exc))
    try:
        shutil._check_unpack_options([".tar"], len, [])
    except shutil.RegistryError as exc:
        show("_check_unpack_options", str(exc))

    # =======================================================================
    # 15. make_archive / unpack_archive round trips
    # =======================================================================
    print("=== 15. archives ===")
    root_dir = os.path.join(ROOT, "arc_src")
    write(os.path.join(root_dir, "pkg", "hello.txt"), b"hello archives\n")
    write(os.path.join(root_dir, "pkg", "sub", "data.bin"), bytes(range(32)))
    write(os.path.join(root_dir, "top.txt"), b"top")
    expected = tree(root_dir)
    expected_pkg = tree(os.path.join(root_dir, "pkg"))
    out_dir = os.path.join(ROOT, "arc_out")

    class Logger:
        def __init__(self):
            self.lines = []

        def info(self, msg, *args):
            self.lines.append("info: " + scrub(msg % args))

        def debug(self, msg, *args):
            self.lines.append("debug: " + scrub(msg % args))

    for fmt, ext in (("zip", ".zip"), ("tar", ".tar"), ("gztar", ".tar.gz"),
                     ("bztar", ".tar.bz2"), ("xztar", ".tar.xz")):
        name = shutil.make_archive(os.path.join(out_dir, fmt, "pkg_" + fmt), fmt,
                                   root_dir=root_dir, base_dir="pkg")
        show(fmt, "name", rel(name), name.endswith(ext), os.path.isabs(name))
        if fmt == "zip":
            with zipfile.ZipFile(name) as zf:
                show(fmt, "members", sorted(zf.namelist()))
        else:
            with tarfile.open(name) as tf:
                show(fmt, "members", sorted(tf.getnames()))
        # (`_unpack_zipfile` takes no `filter`.)
        filter_kw = {} if fmt == "zip" else {"filter": "data"}
        dest = os.path.join(ROOT, "unpacked_" + fmt)
        shutil.unpack_archive(name, dest, **filter_kw)
        show(fmt, "roundtrip", tree(os.path.join(dest, "pkg")) == expected_pkg)
        dest2 = os.path.join(ROOT, "unpacked2_" + fmt)
        shutil.unpack_archive(pathlib.Path(name), pathlib.Path(dest2), format=fmt, **filter_kw)
        show(fmt, "roundtrip-format", tree(os.path.join(dest2, "pkg")) == expected_pkg)
    try:
        shutil.unpack_archive(os.path.join(out_dir, "zip", "pkg_zip.zip"),
                              os.path.join(ROOT, "zip_filter"), filter="data")
    except TypeError as exc:
        show("zip-filter", str(exc))

    # Whole root_dir (base_dir defaults to os.curdir)
    log = Logger()
    name = shutil.make_archive(os.path.join(out_dir, "whole"), "zip", root_dir=root_dir, logger=log)
    with zipfile.ZipFile(name) as zf:
        show("whole-zip", sorted(zf.namelist()))
    show("whole-zip-log", sorted(log.lines))
    log = Logger()
    name = shutil.make_archive(os.path.join(out_dir, "whole"), "gztar", root_dir=root_dir, logger=log)
    with tarfile.open(name) as tf:
        show("whole-tar", sorted(tf.getnames()))
    show("whole-tar-log", log.lines)
    dest = os.path.join(ROOT, "whole_unpacked")
    shutil.unpack_archive(name, dest, filter="data")
    show("whole-roundtrip", tree(dest) == expected)

    # Relative base_name with a cwd change (no root_dir).
    here = os.getcwd()
    os.chdir(root_dir)
    try:
        name = shutil.make_archive("rel_archive", "tar", base_dir="pkg")
        show("relative", name, os.path.exists(os.path.join(root_dir, name)))
        os.unlink(name)
        name = shutil.make_archive(os.path.join("nested_out", "rel"), "zip", base_dir="pkg")
        show("relative-nested", name.replace("\\", "/"), os.path.isdir("nested_out"))
        shutil.rmtree("nested_out")
    finally:
        os.chdir(here)

    # dry_run creates nothing.
    log = Logger()
    name = shutil.make_archive(os.path.join(out_dir, "dry", "never"), "gztar", root_dir=root_dir,
                               base_dir="pkg", dry_run=True, logger=log)
    show("dry-run", rel(name), os.path.exists(name), os.path.exists(os.path.join(out_dir, "dry")))
    show("dry-run-log", log.lines)

    # Custom archiver without supports_root_dir: make_archive chdirs.
    seen_cwd = []

    def chdir_archiver(base_name, base_dir, **kwargs):
        seen_cwd.append((rel(os.getcwd()), rel(base_name), base_dir, sorted(kwargs)))
        return "custom-result"

    shutil.register_archive_format("chdir", chdir_archiver, [], "chdir")
    log = Logger()
    before = os.getcwd()
    show("chdir-ret", shutil.make_archive(pathlib.Path(ROOT) / "c", "chdir", root_dir=root_dir, logger=log))
    show("chdir-seen", seen_cwd, os.getcwd() == before)
    show("chdir-log", log.lines)

    def rootdir_archiver(base_name, base_dir, root_dir=None, **kwargs):
        return (type(base_name).__name__, rel(base_name), rel(root_dir), sorted(kwargs))

    rootdir_archiver.supports_root_dir = True
    shutil.register_archive_format("rootdir", rootdir_archiver)
    show("rootdir-ret", shutil.make_archive(pathlib.Path(ROOT) / "r", "rootdir", root_dir=root_dir))
    shutil.unregister_archive_format("chdir")
    shutil.unregister_archive_format("rootdir")

    # Error paths.
    try:
        shutil.make_archive(os.path.join(ROOT, "x"), "nope")
    except ValueError as exc:
        show("make-unknown", str(exc))
    try:
        shutil.make_archive(os.path.join(ROOT, "x"), "zip", root_dir=os.path.join(root_dir, "top.txt"))
    except NotADirectoryError as exc:
        show("make-root-not-dir", err_name(exc), exc.errno == errno.ENOTDIR, exc.strerror, rel(exc.filename))
    try:
        shutil.make_archive(os.path.join(ROOT, "x"), "zip", root_dir=os.path.join(ROOT, "nope"))
    except FileNotFoundError as exc:
        show("make-root-missing", err_name(exc))
    try:
        shutil._make_tarball(os.path.join(ROOT, "x"), "pkg", compress="lz4")
    except ValueError as exc:
        show("tarball-compress", str(exc))
    try:
        shutil.unpack_archive(os.path.join(ROOT, "archive.unknown"))
    except shutil.ReadError as exc:
        show("unpack-unknown-ext", err_name(exc), scrub(exc))
    try:
        shutil.unpack_archive(os.path.join(ROOT, "archive.zip"), format="nope")
    except ValueError as exc:
        show("unpack-unknown-format", str(exc))
    bad_zip = os.path.join(ROOT, "bad.zip")
    write(bad_zip, b"this is not a zip file")
    try:
        shutil.unpack_archive(bad_zip, os.path.join(ROOT, "bad_out"))
    except shutil.ReadError as exc:
        show("unpack-bad-zip", err_name(exc), scrub(exc))
    bad_tar = os.path.join(ROOT, "bad.tar")
    write(bad_tar, b"\x00" * 10)
    try:
        shutil.unpack_archive(bad_tar, os.path.join(ROOT, "bad_out"))
    except shutil.ReadError as exc:
        show("unpack-bad-tar", err_name(exc), scrub(exc))

    # A zip with unsafe member names: absolute and parent paths are skipped.
    unsafe_zip = os.path.join(ROOT, "unsafe.zip")
    with zipfile.ZipFile(unsafe_zip, "w") as zf:
        zf.writestr("ok/file.txt", b"fine")
        zf.writestr("/abs.txt", b"absolute")
        zf.writestr("../escape.txt", b"escape")
        zf.writestr("dir/", b"")
    dest = os.path.join(ROOT, "unsafe_out")
    shutil.unpack_archive(unsafe_zip, dest)
    show("unsafe-zip", tree(dest), os.path.exists(os.path.join(ROOT, "escape.txt")))

    # =======================================================================
    # 16. get_terminal_size
    # =======================================================================
    print("=== 16. get_terminal_size ===")
    saved = {k: os.environ.get(k) for k in ("COLUMNS", "LINES")}
    try:
        for k in ("COLUMNS", "LINES"):
            if k in os.environ:
                del os.environ[k]
        size = shutil.get_terminal_size(fallback=(91, 37))
        show("fallback", type(size).__name__, size, size.columns, size.lines)
        show("default-type", isinstance(shutil.get_terminal_size(), os.terminal_size))
        os.environ["COLUMNS"] = "120"
        os.environ["LINES"] = "40"
        show("env", shutil.get_terminal_size())
        os.environ["COLUMNS"] = " 99 "
        del os.environ["LINES"]
        show("env-columns-only", shutil.get_terminal_size(fallback=(1, 2)))
        os.environ["COLUMNS"] = "0"
        os.environ["LINES"] = "-3"
        show("env-non-positive", shutil.get_terminal_size(fallback=(70, 20)))
        os.environ["COLUMNS"] = "wide"
        os.environ["LINES"] = "12"
        show("env-invalid", shutil.get_terminal_size(fallback=(66, 11)))
        del os.environ["COLUMNS"]
        del os.environ["LINES"]
        try:
            shutil.get_terminal_size(fallback=5)
        except TypeError as exc:
            show("bad-fallback", err_name(exc))
        show("list-fallback", shutil.get_terminal_size(fallback=[50, 10]))
    finally:
        for k, v in saved.items():
            if v is None:
                if k in os.environ:
                    del os.environ[k]
            else:
                os.environ[k] = v
finally:
    def _force(func, path, exc):
        os.chmod(path, stat.S_IWRITE | stat.S_IREAD)
        func(path)

    shutil.rmtree(ROOT, onexc=_force)

print("root-removed", not os.path.exists(ROOT))
