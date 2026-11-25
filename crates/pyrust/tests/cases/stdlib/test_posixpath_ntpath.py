"""CPython 3.12 cross-platform parity tests for genericpath, posixpath, ntpath,
nturl2path, and os.path 3.12 symbols (issue #3245).

Runs identically on both Ubuntu (POSIX) and Windows (NT).
"""

import genericpath
import ntpath
import nturl2path
import os
import os.path
import pathlib
import posixpath
import shutil
import sys
import tempfile

# ── 1. posixpath constants & pure path operations (cross-platform) ───────────
assert posixpath.sep == "/"
assert posixpath.altsep is None
assert posixpath.extsep == "."
assert posixpath.pathsep == ":"
assert posixpath.defpath == "/bin:/usr/bin"
assert posixpath.devnull == "/dev/null"
assert posixpath.curdir == "."
assert posixpath.pardir == ".."
assert isinstance(posixpath.supports_unicode_filenames, bool)

assert posixpath.normcase("/Foo/Bar") == "/Foo/Bar"
assert posixpath.normcase(b"/Foo/Bar") == b"/Foo/Bar"
assert posixpath.isabs("/a/b") is True
assert posixpath.isabs("a/b") is False
assert posixpath.isabs(b"/a") is True
assert posixpath.isabs(b"a") is False

assert posixpath.join("a", "b", "c") == "a/b/c"
assert posixpath.join("/a", "b", "/c", "d") == "/c/d"
assert posixpath.join(b"/a", b"b", b"c") == b"/a/b/c"
assert posixpath.join("a/", "b") == "a/b"

assert posixpath.split("/a/b/c") == ("/a/b", "c")
assert posixpath.split("/") == ("/", "")
assert posixpath.split("//a") == ("//", "a")
assert posixpath.split("a/") == ("a", "")
assert posixpath.split("") == ("", "")
assert posixpath.split(b"/a/b") == (b"/a", b"b")

# Python 3.12 splitroot
assert posixpath.splitroot("") == ("", "", "")
assert posixpath.splitroot("/") == ("", "/", "")
assert posixpath.splitroot("//") == ("", "//", "")
assert posixpath.splitroot("///") == ("", "/", "//")
assert posixpath.splitroot("/a/b") == ("", "/", "a/b")
assert posixpath.splitroot("//a/b") == ("", "//", "a/b")
assert posixpath.splitroot("///a/b") == ("", "/", "//a/b")
assert posixpath.splitroot("a/b") == ("", "", "a/b")
assert posixpath.splitroot(b"//a/b") == (b"", b"//", b"a/b")

assert posixpath.splitext("/a/b.tar.gz") == ("/a/b.tar", ".gz")
assert posixpath.splitext("/a/.bashrc") == ("/a/.bashrc", "")
assert posixpath.splitext(b"foo.txt") == (b"foo", b".txt")

assert posixpath.splitdrive("/a/b") == ("", "/a/b")
assert posixpath.splitdrive("C:/a") == ("", "C:/a")
assert posixpath.splitdrive(b"/a") == (b"", b"/a")

assert posixpath.basename("/a/b/c") == "c"
assert posixpath.dirname("/a/b/c") == "/a/b"
assert posixpath.basename(b"/a/b/c") == b"c"
assert posixpath.dirname(b"/a/b/c") == b"/a/b"

assert posixpath.normpath("") == "."
assert posixpath.normpath("a/./b/../c") == "a/c"
assert posixpath.normpath("//a//b") == "//a/b"
assert posixpath.normpath("///a//b") == "/a/b"
assert posixpath.normpath(b"a/../b") == b"b"

assert posixpath.relpath("/a/b/c", "/a") == "b/c"
assert posixpath.relpath("/a/b", "/a/b") == "."
assert posixpath.relpath("/a/b", "/c/d") == "../../a/b"

assert posixpath.commonpath(["/usr/lib", "/usr/local/lib"]) == "/usr"
assert posixpath.commonpath(["a/b/c", "a/b/d"]) == "a/b"
assert posixpath.commonpath([b"/usr/lib", b"/usr/local/lib"]) == b"/usr"

assert posixpath.isjunction("/tmp") is False

print("posixpath ok")

# ── 2. ntpath constants & pure path operations (cross-platform) ──────────────
assert ntpath.sep == "\\"
assert ntpath.altsep == "/"
assert ntpath.extsep == "."
assert ntpath.pathsep == ";"
assert ntpath.defpath == ".;C:\\bin"
assert ntpath.devnull == "nul"
assert ntpath.curdir == "."
assert ntpath.pardir == ".."
assert ntpath.supports_unicode_filenames is True

assert ntpath.normcase("C:/Users/Alice") == r"c:\users\alice"
assert ntpath.normcase(b"C:/Users/Alice") == br"c:\users\alice"
assert ntpath.isabs(r"C:\x") is True
assert ntpath.isabs("C:/") is True
assert ntpath.isabs(r"\\host\share") is True
assert ntpath.isabs("C:x") is False
assert ntpath.isabs("x") is False
assert ntpath.isabs("") is False

assert ntpath.join(r"C:\a", "b") == r"C:\a\b"
assert ntpath.join("a", r"C:\b") == r"C:\b"
assert ntpath.join("C:", "b") == "C:b"
assert ntpath.join("C:\\", "b") == r"C:\b"
assert ntpath.join("c:/a", "d:/b") == "d:/b"
assert ntpath.join(b"C:\\a", b"b") == br"C:\a\b"

assert ntpath.split(r"C:\a\b") == (r"C:\a", "b")
assert ntpath.split("C:/a/b") == ("C:/a", "b")
assert ntpath.split("C:\\") == ("C:\\", "")
assert ntpath.split(r"\\host\share\x\y") == (r"\\host\share\x", "y")
assert ntpath.split(b"C:\\a\\b") == (br"C:\a", b"b")

# Python 3.12 splitroot
assert ntpath.splitroot("") == ("", "", "")
assert ntpath.splitroot("C:") == ("C:", "", "")
assert ntpath.splitroot("C:\\") == ("C:", "\\", "")
assert ntpath.splitroot(r"C:\a") == ("C:", "\\", "a")
assert ntpath.splitroot("C:a") == ("C:", "", "a")
assert ntpath.splitroot(r"\a") == ("", "\\", "a")
assert ntpath.splitroot(r"\\host\share\a\b") == (r"\\host\share", "\\", r"a\b")
assert ntpath.splitroot(r"\\?\UNC\a\b\c") == (r"\\?\UNC\a\b", "\\", "c")
assert ntpath.splitroot("//a/b/c") == ("//a/b", "/", "c")
assert ntpath.splitroot(b"C:\\a") == (b"C:", b"\\", b"a")

assert ntpath.splitdrive("c:/x") == ("c:", "/x")
assert ntpath.splitdrive("//host/share/x") == ("//host/share", "/x")
assert ntpath.splitdrive("a/b") == ("", "a/b")
assert ntpath.splitdrive(b"C:\\a") == (b"C:", b"\\a")

assert ntpath.splitext(r"C:\a\b.txt") == (r"C:\a\b", ".txt")
assert ntpath.splitext(r"C:\a\.bashrc") == (r"C:\a\.bashrc", "")
assert ntpath.splitext(b"C:\\a\\b.txt") == (br"C:\a\b", b".txt")

assert ntpath.basename(r"C:\a\b") == "b"
assert ntpath.dirname(r"C:\a\b") == r"C:\a"

assert ntpath.normpath("a/b") == r"a\b"
assert ntpath.normpath("C:/a/../b") == r"C:\b"
assert ntpath.normpath("//host/share/a/../b") == r"\\host\share\b"
assert ntpath.normpath("") == "."
assert ntpath.normpath(b"C:/a/../b") == br"C:\b"

assert ntpath.relpath(r"C:\a\b\c", r"C:\a\d") == r"..\b\c"
assert ntpath.relpath(r"C:\a\b", r"C:\a\b") == "."

assert ntpath.commonpath([r"C:\a\b", "c:/a/c"]) == r"C:\a"
assert ntpath.commonpath([r"C:\a", r"C:\b"]) == "C:\\"
assert ntpath.commonpath(["a/b/c", r"a\b\d"]) == r"a\b"

assert ntpath.ismount("C:\\") is True
assert ntpath.ismount("C:/") is True
assert ntpath.ismount(r"\\server\share") is True
assert ntpath.ismount("\\\\server\\share\\") is True
assert ntpath.ismount(r"\\server\share\sub") is False
assert ntpath.isjunction("C:\\") is False
assert ntpath.isdevdrive("C:\\") is False

print("ntpath ok")

# ── 3. nturl2path (CPython 3.12 parity) ──────────────────────────────────────
assert nturl2path.url2pathname("///C|/foo/bar") == r"C:\foo\bar"
assert nturl2path.url2pathname("/C:/foo/bar") == r"C:\foo\bar"
assert nturl2path.url2pathname("///C:/foo%20bar/baz.txt") == r"C:\foo bar\baz.txt"
assert nturl2path.url2pathname("foo/bar") == r"foo\bar"
assert nturl2path.pathname2url(r"C:\foo\bar") == "///C:/foo/bar"
assert nturl2path.pathname2url(r"C:\foo bar\baz") == "///C:/foo%20bar/baz"
assert nturl2path.pathname2url(r"foo\bar") == "foo/bar"

if sys.version_info < (3, 14):
    assert nturl2path.url2pathname("////host/share/file") == r"\\host\share\file"
    assert nturl2path.pathname2url(r"\\host\share\file") == "////host/share/file"
    try:
        nturl2path.url2pathname("///1|/bad")
        assert False
    except OSError:
        pass
    try:
        nturl2path.pathname2url("CC:\\bad")
        assert False
    except OSError:
        pass

print("nturl2path ok")

# ── 4. genericpath & os.path 3.12 symbols (stat, samestat, sameopenfile, etc.) ─
assert genericpath.commonprefix(["/usr/lib", "/usr/local"]) == "/usr/l"
assert genericpath.commonprefix([]) == ""
assert genericpath.commonprefix([b"abc", b"abd"]) == b"ab"
assert genericpath.commonprefix([["a", "b", "c"], ["a", "b", "d"]]) == ["a", "b"]
assert genericpath._splitext("a/b.txt", "/", None, ".") == ("a/b", ".txt")

assert hasattr(os.path, "splitroot")
assert hasattr(os.path, "ismount")
assert hasattr(os.path, "isjunction")
if os.name == "nt":
    assert hasattr(os.path, "isdevdrive")
assert hasattr(os.path, "samestat")
assert hasattr(os.path, "sameopenfile")
assert hasattr(os.path, "defpath")
assert isinstance(os.path.supports_unicode_filenames, bool)
assert os.path.defpath == os.defpath

tmpdir = tempfile.mkdtemp(prefix="pyrust_path_3245_")
try:
    f1 = os.path.join(tmpdir, "a.txt")
    f2 = os.path.join(tmpdir, "b.txt")
    with open(f1, "wb") as fp:
        fp.write(b"hello")
    with open(f2, "wb") as fp:
        fp.write(b"world")

    assert genericpath.exists(f1) is True
    assert genericpath.isfile(f1) is True
    assert genericpath.isdir(tmpdir) is True
    assert genericpath.islink(f1) is False
    assert genericpath.getsize(f1) == 5
    assert isinstance(genericpath.getmtime(f1), float)
    assert isinstance(genericpath.getatime(f1), float)
    assert isinstance(genericpath.getctime(f1), float)

    st1 = os.stat(f1)
    st1_again = os.stat(f1)
    st2 = os.stat(f2)
    assert genericpath.samestat(st1, st1_again) is True
    assert genericpath.samestat(st1, st2) is False
    assert os.path.samestat(st1, st1_again) is True
    assert os.path.samestat(st1, st2) is False
    assert genericpath.samefile(f1, f1) is True
    assert genericpath.samefile(f1, f2) is False

    # sameopenfile with open file descriptors
    fd1 = os.open(f1, os.O_RDONLY)
    fd1_dup = os.dup(fd1)
    fd2 = os.open(f2, os.O_RDONLY)
    try:
        assert genericpath.sameopenfile(fd1, fd1_dup) is True
        assert os.path.sameopenfile(fd1, fd1_dup) is True
        assert genericpath.sameopenfile(fd1, fd2) is False
        assert os.path.sameopenfile(fd1, fd2) is False
    finally:
        os.close(fd1)
        os.close(fd1_dup)
        os.close(fd2)

    # isjunction & isdevdrive & ismount on regular files/dirs
    assert os.path.isjunction(f1) is False
    assert os.path.isjunction(tmpdir) is False
    assert os.path.isjunction(pathlib.Path(f1)) is False
    if os.name == "nt":
        assert os.path.isdevdrive(f1) is False
    assert os.path.ismount(f1) is False
    if os.name == "posix":
        assert os.path.ismount("/") is True
    else:
        drive, _ = os.path.splitdrive(os.path.abspath(tmpdir))
        assert os.path.ismount(drive + "\\") is True

    print("genericpath & os.path 3.12 ok")
finally:
    shutil.rmtree(tmpdir, ignore_errors=True)

print("test_posixpath_ntpath ok")
