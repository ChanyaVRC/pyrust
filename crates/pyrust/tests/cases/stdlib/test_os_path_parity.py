"""CPython 3.12 parity tests for os.path (issue #3206).

Covers:
- commonpath, commonprefix, expandvars
- getsize, getmtime, getatime, getctime
- islink, samefile, normcase, relpath, realpath(path, *, strict=False/True)
- PathLike acceptance across os.path functions
"""

import os
import os.path as p
import pathlib
import shutil
import tempfile

# ── 1. commonpath & commonprefix ─────────────────────────────────────────────
if os.name == "posix":
    assert p.commonpath(["/usr/lib", "/usr/local/lib"]) == "/usr"
    assert p.commonpath(["/usr/lib", "/usr/lib"]) == "/usr/lib"
    assert p.commonpath(["/a/b", "/c/d"]) == "/"
    assert p.commonpath(["a/b/c", "a/b/d"]) == "a/b"
    assert p.commonpath(["a/b", "c/d"]) == ""
    assert p.commonpath([pathlib.PurePosixPath("/x/y/1"), pathlib.PurePosixPath("/x/y/2")]) == "/x/y"
    try:
        p.commonpath(["/a/b", "a/b"])
        assert False
    except ValueError:
        pass
else:
    assert p.commonpath([r"C:\a\b", r"c:/a/c"]) == r"C:\a"
    assert p.commonpath([r"C:\a", r"C:\b"]) == "C:\\"
    assert p.commonpath(["a/b/c", "a\\b\\d"]) == r"a\b"
    assert p.commonpath([pathlib.PureWindowsPath(r"C:\x\1"), pathlib.PureWindowsPath(r"C:\x\2")]) == r"C:\x"
    try:
        p.commonpath([r"C:\a", r"D:\a"])
        assert False
    except ValueError:
        pass
    try:
        p.commonpath([r"C:\a", "a"])
        assert False
    except ValueError:
        pass

try:
    p.commonpath([])
    assert False
except ValueError:
    pass

assert p.commonprefix(["/usr/lib", "/usr/local"]) == "/usr/l"
assert p.commonprefix([]) == ""
assert p.commonprefix([b"abc", b"abd"]) == b"ab"

print("os.path commonpath/commonprefix ok")

# ── 2. expandvars ────────────────────────────────────────────────────────────
var_name = "PYRUST_TEST_EXPANDVARS_3206"
old_val = os.environ.get(var_name)
os.environ[var_name] = "hello_val"
try:
    assert p.expandvars(f"prefix/${var_name}/suffix") == "prefix/hello_val/suffix"
    assert p.expandvars(f"prefix/${{{var_name}}}/suffix") == "prefix/hello_val/suffix"
    assert p.expandvars("$PYRUST_UNDEFINED_VAR_999999/x") == "$PYRUST_UNDEFINED_VAR_999999/x"
    assert p.expandvars("${PYRUST_UNDEFINED_VAR_999999}/x") == "${PYRUST_UNDEFINED_VAR_999999}/x"
    if os.name == "nt":
        assert p.expandvars(f"prefix\\%{var_name}%\\suffix") == "prefix\\hello_val\\suffix"
        assert p.expandvars("%PYRUST_UNDEFINED_VAR_999999%") == "%PYRUST_UNDEFINED_VAR_999999%"
        assert p.expandvars("%%") == "%"
finally:
    if old_val is None:
        del os.environ[var_name]
    else:
        os.environ[var_name] = old_val

print("os.path expandvars ok")

# ── 3. normcase & relpath ────────────────────────────────────────────────────
if os.name == "posix":
    assert p.normcase("/A/B/c") == "/A/B/c"
    assert p.relpath("/a/b/c", "/a") == "b/c"
else:
    assert p.normcase("C:/Users/Alice") == r"c:\users\alice"
    assert p.relpath(r"C:\a\b\c", r"C:\a") == r"b\c"

# relpath with start=None defaults to curdir
assert p.relpath(os.getcwd(), None) == "."

print("os.path normcase/relpath ok")

# ── 4. getsize, getmtime, getatime, getctime, islink, samefile, realpath ────
tmpdir = tempfile.mkdtemp(prefix="pyrust_os_path_")
try:
    f1 = os.path.join(tmpdir, "sample.txt")
    f2 = os.path.join(tmpdir, "other.txt")
    with open(f1, "wb") as f:
        f.write(b"0123456789")
    with open(f2, "wb") as f:
        f.write(b"x")

    st = os.stat(f1)
    assert p.getsize(f1) == 10
    assert p.getsize(pathlib.Path(f1)) == 10
    assert isinstance(p.getmtime(f1), float)
    assert isinstance(p.getatime(f1), float)
    assert isinstance(p.getctime(f1), float)
    assert abs(p.getmtime(f1) - st.st_mtime) < 1.0
    assert abs(p.getatime(f1) - st.st_atime) < 1.0
    assert p.getctime(f1) > 0

    missing = os.path.join(tmpdir, "does_not_exist_3206")
    for fn in (p.getsize, p.getmtime, p.getatime, p.getctime):
        try:
            fn(missing)
            assert False
        except FileNotFoundError:
            pass

    assert p.islink(f1) is False
    assert p.islink(missing) is False
    assert p.samefile(f1, f1) is True
    assert p.samefile(pathlib.Path(f1), f1) is True
    assert p.samefile(f1, f2) is False

    # realpath strict=False & strict=True
    rp = p.realpath(f1)
    assert p.isabs(rp)
    assert not rp.startswith("\\\\?\\")
    assert p.realpath(f1, strict=True) == rp
    assert p.realpath(pathlib.Path(f1), strict=True) == rp

    rp_missing = p.realpath(missing, strict=False)
    assert p.isabs(rp_missing)
    assert rp_missing.endswith("does_not_exist_3206")

    try:
        p.realpath(missing, strict=True)
        assert False
    except FileNotFoundError:
        pass

    print("os.path stat/realpath ok")
finally:
    shutil.rmtree(tmpdir, ignore_errors=True)

print("test_os_path_parity ok")
