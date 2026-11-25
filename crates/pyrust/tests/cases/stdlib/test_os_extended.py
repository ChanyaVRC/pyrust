"""CPython 3.12 cross-platform parity tests for extended `os` portable APIs
(issue #3245).

Covers:
- scandir / DirEntry / ScandirIterator (context manager, iterator, PathLike)
- open + O_* flags, fdopen, dup, dup2, pipe, lseek + SEEK_*, fstat, fsync,
  truncate, ftruncate, isatty, closerange, get_inheritable / set_inheritable,
  get_blocking / set_blocking (Unix)
- replace, link, removedirs, renames, utime, putenv / unsetenv,
  fsencode / fsdecode, getcwdb, get_exec_path, PathLike ABC, defpath, error,
  EX_OK (Unix), supports_* sets
- system, popen, kill, waitpid (Unix), waitstatus_to_exitcode, umask,
  getlogin, times / times_result
"""

import abc
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile

# ── 1. Constants, error alias, supports_* sets, PathLike ABC ─────────────────
assert os.error is OSError
assert os.defpath == (".;C:\\bin" if os.name == "nt" else "/bin:/usr/bin")
assert os.SEEK_SET == 0
assert os.SEEK_CUR == 1
assert os.SEEK_END == 2
if os.name == "posix":
    assert os.EX_OK == 0
if os.name == "nt":
    assert isinstance(os.O_BINARY, int)
    assert isinstance(os.O_TEXT, int)

for name in ("supports_dir_fd", "supports_effective_ids", "supports_fd", "supports_follow_symlinks"):
    assert isinstance(getattr(os, name), set), name

# os.PathLike ABC
assert issubclass(os.PathLike, abc.ABC)
assert isinstance(pathlib.Path("abc"), os.PathLike)
assert issubclass(pathlib.Path, os.PathLike)
assert not isinstance("abc", os.PathLike)
assert not isinstance(b"abc", os.PathLike)


class CustomPath:
    def __init__(self, p):
        self._p = p

    def __fspath__(self):
        return self._p


assert isinstance(CustomPath("x"), os.PathLike)
assert issubclass(CustomPath, os.PathLike)
assert os.fspath(CustomPath("x")) == "x"

try:
    os.PathLike()
    assert False, "PathLike should not be instantiable"
except TypeError:
    pass

# GenericAlias support on os.PathLike
pl_alias = os.PathLike[str]
assert pl_alias.__origin__ is os.PathLike
assert pl_alias.__args__ == (str,)

print("constants & PathLike ok")

# ── 2. fsencode, fsdecode, getcwdb, get_exec_path ────────────────────────────
assert os.fsencode("hello") == b"hello"
assert os.fsencode(b"hello") == b"hello"
assert os.fsencode(CustomPath("hello")) == b"hello"
assert os.fsdecode(b"hello") == "hello"
assert os.fsdecode("hello") == "hello"
assert os.fsdecode(CustomPath(b"hello")) == "hello"

cwdb = os.getcwdb()
assert isinstance(cwdb, bytes) and len(cwdb) > 0
assert os.fsdecode(cwdb) == os.getcwd()

sep = os.pathsep
assert os.get_exec_path({"PATH": f"dir1{sep}dir2"}) == ["dir1", "dir2"]
assert os.get_exec_path({}) == os.defpath.split(sep)
assert isinstance(os.get_exec_path(), list)
if os.name == "posix":
    assert os.get_exec_path({b"PATH": b"b1:b2"}) == ["b1", "b2"]
    try:
        os.get_exec_path({"PATH": "a", b"PATH": b"b"})
        assert False
    except ValueError:
        pass
else:
    assert os.get_exec_path({"PATH": "a", b"PATH": b"b"}) == ["a"]

print("encoding & exec_path ok")

# ── 3. scandir / DirEntry / ScandirIterator ──────────────────────────────────
tmpdir = tempfile.mkdtemp(prefix="pyrust_os_ext_3245_")
try:
    sub = os.path.join(tmpdir, "subdir")
    os.mkdir(sub)
    f_alpha = os.path.join(tmpdir, "alpha.txt")
    with open(f_alpha, "wb") as fp:
        fp.write(b"abcdef")

    with os.scandir(tmpdir) as it:
        entries = list(it)
    by_name = {e.name: e for e in entries}
    assert sorted(by_name.keys()) == ["alpha.txt", "subdir"]

    e_file = by_name["alpha.txt"]
    e_dir = by_name["subdir"]
    assert isinstance(e_file, os.DirEntry)
    assert isinstance(e_file, os.PathLike)
    assert os.fspath(e_file) == e_file.path
    assert e_file.path == f_alpha
    assert e_file.is_file() is True
    assert e_file.is_file(follow_symlinks=False) is True
    assert e_file.is_dir() is False
    assert e_file.is_symlink() is False
    assert e_file.is_junction() is False
    assert e_file.stat().st_size == 6
    assert e_file.inode() == os.lstat(e_file.path).st_ino
    assert repr(e_file) == "<DirEntry 'alpha.txt'>"

    assert e_dir.is_dir() is True
    assert e_dir.is_file() is False
    assert e_dir.is_symlink() is False
    assert e_dir.is_junction() is False

    # bytes path -> bytes entry.name and entry.path
    with os.scandir(os.fsencode(tmpdir)) as bit:
        b_names = sorted(e.name for e in bit)
    assert b_names == [b"alpha.txt", b"subdir"]

    # explicit close stops iteration
    it2 = os.scandir(tmpdir)
    it2.close()
    assert list(it2) == []

    print("scandir ok")

    # ── 4. Low-level FD ops: open, read, write, lseek, fstat, fsync, ftruncate,
    #        truncate, dup, dup2, fdopen, isatty, inheritable, closerange, pipe
    fd_path = os.path.join(tmpdir, "fd_test.bin")
    flags = os.O_RDWR | os.O_CREAT | os.O_TRUNC
    if os.name == "nt":
        flags |= os.O_BINARY
    fd = os.open(fd_path, flags, 0o600)
    try:
        assert os.isatty(fd) is False
        assert os.get_inheritable(fd) is False
        os.set_inheritable(fd, True)
        assert os.get_inheritable(fd) is True
        os.set_inheritable(fd, False)
        assert os.get_inheritable(fd) is False

        if os.name == "posix":
            assert os.get_blocking(fd) is True
            os.set_blocking(fd, False)
            assert os.get_blocking(fd) is False
            os.set_blocking(fd, True)
            assert os.get_blocking(fd) is True

        n = os.write(fd, b"0123456789")
        assert n == 10
        os.fsync(fd)
        assert os.fstat(fd).st_size == 10
        assert os.lseek(fd, 0, os.SEEK_SET) == 0
        assert os.read(fd, 4) == b"0123"
        assert os.lseek(fd, 2, os.SEEK_CUR) == 6
        assert os.read(fd, 4) == b"6789"
        assert os.lseek(fd, -2, os.SEEK_END) == 8
        assert os.read(fd, 10) == b"89"

        # ftruncate
        os.ftruncate(fd, 5)
        assert os.fstat(fd).st_size == 5

        # dup
        fd_dup = os.dup(fd)
        try:
            assert fd_dup != fd
            assert os.get_inheritable(fd_dup) is False
            assert os.fstat(fd_dup).st_size == 5
        finally:
            os.close(fd_dup)

        # dup2
        fd_a = os.open(fd_path, os.O_RDONLY)
        fd_b = os.open(fd_path, os.O_RDONLY)
        try:
            res_fd = os.dup2(fd_a, fd_b)
            assert res_fd == fd_b
            assert os.fstat(fd_b).st_size == 5
        finally:
            os.close(fd_a)
            os.close(fd_b)
    finally:
        os.close(fd)

    # truncate by path
    os.truncate(fd_path, 3)
    assert os.stat(fd_path).st_size == 3

    # fdopen
    fd_r = os.open(fd_path, os.O_RDONLY)
    with os.fdopen(fd_r, "rb") as fobj:
        assert fobj.read() == b"012"

    # pipe
    r_fd, w_fd = os.pipe()
    try:
        assert os.write(w_fd, b"ping") == 4
        assert os.read(r_fd, 4) == b"ping"
    finally:
        os.close(r_fd)
        os.close(w_fd)

    # closerange
    c1 = os.open(fd_path, os.O_RDONLY)
    c2 = os.open(fd_path, os.O_RDONLY)
    lo, hi = min(c1, c2), max(c1, c2)
    os.closerange(lo, hi + 1)
    for closed_fd in (c1, c2):
        try:
            os.fstat(closed_fd)
            assert False
        except OSError:
            pass

    print("fd ops ok")

    # ── 5. Filesystem ops: replace, link, removedirs, renames, utime ─────────
    src_f = os.path.join(tmpdir, "src.txt")
    dst_f = os.path.join(tmpdir, "dst.txt")
    with open(src_f, "wb") as fp:
        fp.write(b"new-content")
    with open(dst_f, "wb") as fp:
        fp.write(b"old")
    os.replace(src_f, dst_f)
    assert not os.path.exists(src_f)
    with open(dst_f, "rb") as fp:
        assert fp.read() == b"new-content"

    # hard link
    lnk_f = os.path.join(tmpdir, "dst_link.txt")
    os.link(dst_f, lnk_f)
    assert os.path.samefile(dst_f, lnk_f) is True
    os.unlink(lnk_f)

    # utime (seconds & ns)
    os.utime(dst_f, (1700000000, 1700000100))
    st_u = os.stat(dst_f)
    assert abs(st_u.st_mtime - 1700000100.0) < 2.0
    os.utime(dst_f, ns=(1700000200_000_000_000, 1700000300_000_000_000))
    st_u2 = os.stat(dst_f)
    assert abs(st_u2.st_mtime - 1700000300.0) < 2.0
    os.utime(dst_f, None)
    try:
        os.utime(dst_f, (1, 2), ns=(1, 2))
        assert False
    except ValueError:
        pass

    # removedirs
    deep_a = os.path.join(tmpdir, "rm_a", "rm_b", "rm_c")
    os.makedirs(deep_a)
    os.removedirs(deep_a)
    assert not os.path.exists(os.path.join(tmpdir, "rm_a"))

    # renames (creates target parent dirs and cleans empty source dirs)
    ren_src = os.path.join(tmpdir, "old_d1", "old_d2", "item.txt")
    ren_dst = os.path.join(tmpdir, "new_d1", "new_d2", "item.txt")
    os.makedirs(os.path.dirname(ren_src))
    with open(ren_src, "w") as fp:
        fp.write("moved")
    os.renames(ren_src, ren_dst)
    assert os.path.isfile(ren_dst)
    assert not os.path.exists(os.path.join(tmpdir, "old_d1"))

    print("fs ops ok")
finally:
    shutil.rmtree(tmpdir, ignore_errors=True)

# ── 6. putenv / unsetenv, system, popen, waitstatus_to_exitcode, kill,
#        umask, getlogin, times / times_result ───────────────────────────────
env_key = "PYRUST_PUTENV_TEST_3245"
os.putenv(env_key, "hello_env_3245")
try:
    cmd = f"echo %{env_key}%" if os.name == "nt" else f"echo ${env_key}"
    with os.popen(cmd) as pipe:
        out = pipe.read().strip()
    assert out == "hello_env_3245", repr(out)
finally:
    os.unsetenv(env_key)

# system & popen
assert os.system(f"echo ok > {os.devnull}") == 0
with os.popen("echo popen_line") as p:
    assert p.read().strip() == "popen_line"
    assert p.readable() is True
    assert p.writable() is False
    assert isinstance(p.seekable(), bool)
assert p.closed is True

# waitstatus_to_exitcode
assert os.waitstatus_to_exitcode(0) == 0
assert os.waitstatus_to_exitcode(5 << 8) == 5

# kill (and waitpid on POSIX)
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"])
try:
    os.kill(child.pid, signal.SIGTERM)
    if os.name == "posix":
        wpid, wstatus = os.waitpid(child.pid, 0)
        assert wpid == child.pid
        assert os.waitstatus_to_exitcode(wstatus) == -signal.SIGTERM
        child.returncode = -signal.SIGTERM
    else:
        rc = child.wait(timeout=5)
        assert rc is not None
finally:
    if child.poll() is None:
        child.kill()
        child.wait(timeout=5)

# umask (round-trip)
old_mask = os.umask(0o022)
restored = os.umask(old_mask)
assert isinstance(old_mask, int) and isinstance(restored, int)

# getlogin (may raise OSError in headless Linux containers without a tty)
try:
    login = os.getlogin()
    assert isinstance(login, str) and len(login) > 0
except OSError:
    pass

# times & times_result
t = os.times()
assert isinstance(t, os.times_result)
assert isinstance(t, tuple)
assert len(t) == 5
assert os.times_result.n_fields == 5
assert os.times_result.n_sequence_fields == 5
assert isinstance(t.user, float) and t.user >= 0.0
assert isinstance(t.system, float) and t.system >= 0.0
assert isinstance(t.children_user, float) and t.children_user >= 0.0
assert isinstance(t.children_system, float) and t.children_system >= 0.0
assert isinstance(t.elapsed, float)
assert t[0] == t.user and t[1] == t.system

# abort callable exists
assert callable(os.abort)

print("process & system ok")
print("test_os_extended ok")
