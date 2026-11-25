"""CPython 3.12 parity tests for pathlib (issue #3206).

Covers:
- PurePath, PurePosixPath, PureWindowsPath, Path, PosixPath, WindowsPath
- Inheritance hierarchy and cross-platform instantiation NotImplementedError
- .parents (len, indexing, negative indexing, slicing, iteration, in, repr)
- .suffixes, .with_stem, .relative_to(*other, walk_up=False/True), .is_relative_to
- .match, .rglob, .stat(follow_symlinks=...), .lstat
- .touch, .rename, .replace, .rmdir, .samefile, .is_symlink, .readlink
"""

import os
import pathlib
import shutil
import sys
import tempfile
from pathlib import (
    Path,
    PosixPath,
    PurePath,
    PurePosixPath,
    PureWindowsPath,
    WindowsPath,
)

# ── 1. Class hierarchy & platform instantiation ──────────────────────────────
assert issubclass(PurePosixPath, PurePath)
assert issubclass(PureWindowsPath, PurePath)
assert issubclass(Path, PurePath)
assert issubclass(PosixPath, Path)
assert issubclass(PosixPath, PurePosixPath)
assert issubclass(WindowsPath, Path)
assert issubclass(WindowsPath, PureWindowsPath)
assert PosixPath.__bases__ == (Path, PurePosixPath)
assert WindowsPath.__bases__ == (Path, PureWindowsPath)

if sys.platform == "win32":
    assert type(PurePath("a")) is PureWindowsPath
    assert type(Path("a")) is WindowsPath
    try:
        PosixPath("a")
        assert False, "expected NotImplementedError"
    except NotImplementedError:
        pass
else:
    assert type(PurePath("a")) is PurePosixPath
    assert type(Path("a")) is PosixPath
    try:
        WindowsPath("a")
        assert False, "expected NotImplementedError"
    except NotImplementedError:
        pass

# PurePosixPath & PureWindowsPath can be instantiated on ANY platform
pp = PurePosixPath("/a/b/c.tar.gz")
wp = PureWindowsPath(r"C:\Users\Alice\doc.tar.gz")
assert str(pp) == "/a/b/c.tar.gz"
assert str(wp) == r"C:\Users\Alice\doc.tar.gz"
assert wp.as_posix() == "C:/Users/Alice/doc.tar.gz"
assert repr(wp) == "PureWindowsPath('C:/Users/Alice/doc.tar.gz')"
assert wp.drive == "C:"
assert wp.root == "\\"
assert wp.anchor == "C:\\"
assert pp.drive == ""
assert pp.root == "/"
assert pp.anchor == "/"
assert pp.is_absolute() is True
assert wp.is_absolute() is True
assert PureWindowsPath(r"\only\root").is_absolute() is False
assert PureWindowsPath(r"C:rel\path").is_absolute() is False
assert PurePosixPath(".").parts == ()
assert PureWindowsPath(".").parts == ()
assert not hasattr(pp, "exists")
assert not hasattr(wp, "exists")

# Cross-flavor equality & ordering
assert PurePosixPath("a/b") != PureWindowsPath("a/b")
assert PureWindowsPath("A/B") == PureWindowsPath("a\\b")
assert hash(PureWindowsPath("A/B")) == hash(PureWindowsPath("a\\b"))
assert PurePosixPath("a/b") < PurePosixPath("a/c")
assert "a" / PurePosixPath("b/c") == PurePosixPath("a/b/c")

print("pathlib hierarchy ok")

# ── 2. .parents (len, indexing, negative indexing, slicing) ──────────────────
p = PurePosixPath("/a/b/c/d.txt")
assert len(p.parents) == 4
assert p.parents[0] == PurePosixPath("/a/b/c")
assert p.parents[1] == PurePosixPath("/a/b")
assert p.parents[2] == PurePosixPath("/a")
assert p.parents[3] == PurePosixPath("/")
assert p.parents[-1] == PurePosixPath("/")
assert p.parents[-2] == PurePosixPath("/a")
assert p.parents[-4] == PurePosixPath("/a/b/c")
assert p.parents[1:3] == (PurePosixPath("/a/b"), PurePosixPath("/a"))
assert p.parents[::2] == (PurePosixPath("/a/b/c"), PurePosixPath("/a"))
assert p.parents[::-1] == (
    PurePosixPath("/"),
    PurePosixPath("/a"),
    PurePosixPath("/a/b"),
    PurePosixPath("/a/b/c"),
)
assert PurePosixPath("/a/b") in p.parents
assert PurePosixPath("/z") not in p.parents
assert list(p.parents) == [
    PurePosixPath("/a/b/c"),
    PurePosixPath("/a/b"),
    PurePosixPath("/a"),
    PurePosixPath("/"),
]
assert repr(p.parents) == "<PurePosixPath.parents>"

try:
    _ = p.parents[4]
    assert False
except IndexError:
    pass
try:
    _ = p.parents[-5]
    assert False
except IndexError:
    pass

assert len(PurePosixPath("/").parents) == 0
assert len(PurePosixPath(".").parents) == 0
assert list(PurePosixPath("a/b").parents) == [PurePosixPath("a"), PurePosixPath(".")]
assert list(PureWindowsPath(r"C:\a\b").parents) == [
    PureWindowsPath(r"C:\a"),
    PureWindowsPath("C:\\"),
]
assert len(PureWindowsPath("C:\\").parents) == 0

print("pathlib parents ok")

# ── 3. .suffixes & .with_stem ────────────────────────────────────────────────
assert PurePosixPath("my/library.tar.gz").suffixes == [".tar", ".gz"]
assert PurePosixPath("my/library").suffixes == []
assert PurePosixPath(".bashrc").suffixes == []
assert PurePosixPath(".bashrc.bak").suffixes == [".bak"]
assert PurePosixPath("..").suffixes == []

assert PurePosixPath("/a/foo.tar.gz").with_stem("bar") == PurePosixPath("/a/bar.gz")
assert PurePosixPath("/a/foo").with_stem("bar") == PurePosixPath("/a/bar")
assert PureWindowsPath(r"C:\a\foo.txt").with_stem("bar") == PureWindowsPath(r"C:\a\bar.txt")

try:
    PurePosixPath("a.txt").with_stem("")
    assert False
except ValueError:
    pass
try:
    PurePosixPath("a.txt").with_stem("x/y")
    assert False
except ValueError:
    pass
try:
    PurePosixPath("/").with_stem("x")
    assert False
except ValueError:
    pass

print("pathlib suffixes & with_stem ok")

# ── 4. .relative_to & .is_relative_to ────────────────────────────────────────
base = PurePosixPath("/etc/passwd")
assert base.relative_to("/") == PurePosixPath("etc/passwd")
assert base.relative_to("/etc") == PurePosixPath("passwd")
assert base.relative_to("/etc/passwd") == PurePosixPath(".")
assert base.is_relative_to("/etc") is True
assert base.is_relative_to("/usr") is False
if sys.version_info < (3, 14):
    assert base.relative_to("/", "etc") == PurePosixPath("passwd")
    assert base.is_relative_to("/", "etc") is True

try:
    base.relative_to("/usr")
    assert False
except ValueError:
    pass

# walk_up=True
assert PurePosixPath("/etc/passwd").relative_to("/usr/bin", walk_up=True) == PurePosixPath(
    "../../etc/passwd"
)
assert PurePosixPath("/a/b/c").relative_to("/a/d", walk_up=True) == PurePosixPath("../b/c")
assert PurePosixPath("/a/b").relative_to("/a/b", walk_up=True) == PurePosixPath(".")
assert PureWindowsPath(r"C:\A\B\c.txt").relative_to(r"c:\a") == PureWindowsPath(r"B\c.txt")
assert PureWindowsPath(r"C:\A\B").is_relative_to(r"c:\a") is True

try:
    PurePosixPath("/a/b").relative_to("a", walk_up=True)
    assert False
except ValueError:
    pass

try:
    PurePosixPath("a/b").relative_to("a/..", walk_up=True)
    assert False
except ValueError:
    pass

print("pathlib relative_to ok")

# ── 5. .match ────────────────────────────────────────────────────────────────
assert PurePosixPath("a/b.py").match("*.py") is True
assert PurePosixPath("/a/b/c.py").match("b/*.py") is True
assert PurePosixPath("/a/b/c.py").match("/a/*/*.py") is True
assert PurePosixPath("/a/b/c.py").match("a/*.py") is False
assert PurePosixPath("/a/b/c.py").match("*.PY") is False
assert PureWindowsPath(r"C:\a\b\c.py").match("*.PY") is True
assert PureWindowsPath(r"C:\a\b\c.py").match(r"c:\a\*\*.py") is True
assert PureWindowsPath(r"C:\a\b\c.py").match(r"d:\a\*\*.py") is False
assert PureWindowsPath(r"C:\a\b\c.py").match("/a/*/*.py") is False

try:
    PurePosixPath("a/b.py").match("")
    assert False
except ValueError:
    pass

print("pathlib match ok")

# ── 6. Filesystem methods (cross-platform) ───────────────────────────────────
tmpdir = Path(tempfile.mkdtemp(prefix="pyrust_pathlib_"))
try:
    # touch
    f1 = tmpdir / "file1.txt"
    assert not f1.exists()
    f1.touch()
    assert f1.exists()
    assert f1.is_file()
    f1.touch(exist_ok=True)
    try:
        f1.touch(exist_ok=False)
        assert False
    except FileExistsError:
        pass

    # write_text / read_text / stat / lstat
    f1.write_text("hello world", encoding="utf-8")
    assert f1.read_text(encoding="utf-8") == "hello world"
    st = f1.stat()
    lst = f1.lstat()
    st_nofollow = f1.stat(follow_symlinks=False)
    assert st.st_size == 11
    assert lst.st_size == 11
    assert st_nofollow.st_size == 11
    assert st.st_mtime > 0

    # samefile
    f2 = tmpdir / "file2.txt"
    f2.write_text("other", encoding="utf-8")
    assert f1.samefile(f1) is True
    assert f1.samefile(str(f1)) is True
    assert f1.samefile(f2) is False
    try:
        f1.samefile(tmpdir / "missing.txt")
        assert False
    except FileNotFoundError:
        pass

    # rename & replace
    f1_renamed = f1.rename(tmpdir / "file1_moved.txt")
    assert isinstance(f1_renamed, Path)
    assert f1_renamed == tmpdir / "file1_moved.txt"
    assert not f1.exists()
    assert f1_renamed.read_text(encoding="utf-8") == "hello world"

    f1_replaced = f1_renamed.replace(f2)
    assert f1_replaced == f2
    assert not f1_renamed.exists()
    assert f2.read_text(encoding="utf-8") == "hello world"

    # rmdir
    sub = tmpdir / "sub" / "nested"
    sub.mkdir(parents=True)
    assert sub.is_dir()
    sub.rmdir()
    assert not sub.exists()
    # Non-empty dir raises OSError
    (tmpdir / "sub" / "keep.py").write_text("x=1", encoding="utf-8")
    try:
        (tmpdir / "sub").rmdir()
        assert False
    except OSError:
        pass

    # rglob
    deep = tmpdir / "sub" / "deep"
    deep.mkdir()
    (deep / "a.py").write_text("a", encoding="utf-8")
    (deep / "b.txt").write_text("b", encoding="utf-8")
    (tmpdir / "top.py").write_text("t", encoding="utf-8")
    found_py = sorted(p.name for p in tmpdir.rglob("*.py"))
    assert found_py == ["a.py", "keep.py", "top.py"], found_py
    found_sub_py = sorted(p.name for p in tmpdir.rglob("deep/*.py"))
    assert found_sub_py == ["a.py"], found_sub_py

    # is_symlink & readlink (on Windows without Developer Mode, os.symlink may raise OSError)
    assert f2.is_symlink() is False
    assert (tmpdir / "nonexistent_link").is_symlink() is False
    link_path = tmpdir / "sym_to_f2"
    symlink_created = False
    try:
        os.symlink(str(f2), str(link_path))
        symlink_created = True
    except OSError:
        pass
    if symlink_created:
        assert link_path.is_symlink() is True
        assert link_path.readlink() == f2
        assert link_path.samefile(f2) is True
        assert link_path.stat().st_size == 11
        assert link_path.lstat().st_mode != 0

    print("pathlib filesystem ok")
finally:
    shutil.rmtree(str(tmpdir), ignore_errors=True)

print("test_pathlib_parity ok")
