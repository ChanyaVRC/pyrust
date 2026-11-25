# Comprehensive CPython 3.12 parity tests for `filecmp` (issue #3230).

import filecmp
import io
import os
import sys
import tempfile


def capture(fn):
    buf = io.StringIO()
    old = sys.stdout
    sys.stdout = buf
    try:
        fn()
    finally:
        sys.stdout = old
    return buf.getvalue()


# 1. Module metadata and constants
print("in sys.modules:", "filecmp" in sys.modules)
print("__all__:", filecmp.__all__)
print("DEFAULT_IGNORES:", filecmp.DEFAULT_IGNORES)
print("BUFSIZE:", filecmp.BUFSIZE)
print("dircmp.__module__:", filecmp.dircmp.__module__)
print("GenericAlias:", filecmp.dircmp[str])

with tempfile.TemporaryDirectory() as tmp:
    d1 = os.path.join(tmp, "dir1")
    d2 = os.path.join(tmp, "dir2")
    os.mkdir(d1)
    os.mkdir(d2)

    def write_file(path, content):
        with open(path, "wb") as fp:
            fp.write(content)

    # 2. filecmp.cmp (identical, differing same-size, differing size, large > BUFSIZE, directory)
    fa = os.path.join(d1, "same.txt")
    fb = os.path.join(d2, "same.txt")
    fc = os.path.join(tmp, "diff_same_len_1.txt")
    fd = os.path.join(tmp, "diff_same_len_2.txt")
    fe = os.path.join(d1, "diff_len.txt")
    ff = os.path.join(d2, "diff_len.txt")
    big1 = os.path.join(d1, "big.bin")
    big2 = os.path.join(d2, "big.bin")

    write_file(fa, b"hello world\n")
    write_file(fb, b"hello world\n")
    write_file(fc, b"aaaa")
    write_file(fd, b"aaab")
    write_file(fe, b"short")
    write_file(ff, b"much longer content")
    write_file(big1, b"x" * (filecmp.BUFSIZE * 2 + 123))
    write_file(big2, b"x" * (filecmp.BUFSIZE * 2 + 123))

    filecmp.clear_cache()
    print("cmp same:", filecmp.cmp(fa, fb, shallow=False), filecmp.cmp(fa, fb, shallow=True))
    print("cmp self:", filecmp.cmp(fa, fa, shallow=True), filecmp.cmp(fa, fa, shallow=False))
    print("cmp diff same len:", filecmp.cmp(fc, fd, shallow=False))
    print("cmp diff len:", filecmp.cmp(fe, ff, shallow=False))
    print("cmp big:", filecmp.cmp(big1, big2, shallow=False))
    print("cmp dir:", filecmp.cmp(d1, d2))
    print("cmp dir vs file:", filecmp.cmp(d1, fa))

    # Cache hit + clear_cache
    print("cache non-empty:", len(filecmp._cache) > 0)
    filecmp.clear_cache()
    print("cache cleared:", len(filecmp._cache))

    # 3. filecmp.cmpfiles (match, mismatch, errors)
    match, mismatch, errors = filecmp.cmpfiles(
        d1,
        d2,
        ["same.txt", "diff_len.txt", "big.bin", "missing.txt"],
        shallow=False,
    )
    print("cmpfiles:", match, mismatch, errors)

    # 4. dircmp tree setup with left_only, right_only, common_dirs, common_funny, ignored, nested subdirs
    write_file(os.path.join(d1, "only_left.txt"), b"left")
    write_file(os.path.join(d2, "only_right.txt"), b"right")
    # Default ignored name
    os.mkdir(os.path.join(d1, "__pycache__"))
    os.mkdir(os.path.join(d2, "__pycache__"))
    # common_funny: file in d1, directory in d2
    write_file(os.path.join(d1, "funny_entry"), b"file")
    os.mkdir(os.path.join(d2, "funny_entry"))
    # Subdirectory with nested subdir
    os.mkdir(os.path.join(d1, "sub"))
    os.mkdir(os.path.join(d2, "sub"))
    write_file(os.path.join(d1, "sub", "s.txt"), b"same")
    write_file(os.path.join(d2, "sub", "s.txt"), b"same")
    write_file(os.path.join(d1, "sub", "d.txt"), b"1")
    write_file(os.path.join(d2, "sub", "d.txt"), b"22")
    os.mkdir(os.path.join(d1, "sub", "deep"))
    os.mkdir(os.path.join(d2, "sub", "deep"))
    write_file(os.path.join(d1, "sub", "deep", "only_d1.txt"), b"x")

    dc = filecmp.dircmp(d1, d2)
    # Accessing lazy attributes triggers phases on demand
    print("left_list:", dc.left_list)
    print("right_list:", dc.right_list)
    print("common:", sorted(dc.common))
    print("left_only:", sorted(dc.left_only))
    print("right_only:", sorted(dc.right_only))
    print("common_dirs:", sorted(dc.common_dirs))
    print("common_files:", sorted(dc.common_files))
    print("common_funny:", sorted(dc.common_funny))
    print("same_files:", sorted(dc.same_files))
    print("diff_files:", sorted(dc.diff_files))
    print("funny_files:", sorted(dc.funny_files))
    print("subdirs keys:", sorted(dc.subdirs.keys()))
    print("sub same/diff:", sorted(dc.subdirs["sub"].same_files), sorted(dc.subdirs["sub"].diff_files))
    print("deep left_only:", sorted(dc.subdirs["sub"].subdirs["deep"].left_only))

    # Unknown attribute raises AttributeError
    try:
        _ = dc.no_such_attr
        print("unknown attr: OK")
    except AttributeError as e:
        print("unknown attr:", type(e).__name__, str(e))

    # Custom ignore and hide
    dc_custom = filecmp.dircmp(d1, d2, ignore=[], hide=[])
    print("custom ignore includes __pycache__:", "__pycache__" in dc_custom.common_dirs)

    # Subclassing dircmp preserves subclass type in subdirs
    class MyDirCmp(filecmp.dircmp):
        pass

    mdc = MyDirCmp(d1, d2)
    print("subclass subdir type:", type(mdc.subdirs["sub"]) is MyDirCmp)

    # report / report_partial_closure / report_full_closure (normalize temp path)
    def norm(text):
        return text.replace(tmp, "<TMP>").replace("\\", "/")

    print("--- report ---")
    print(norm(capture(dc.report)), end="")
    print("--- report_partial_closure ---")
    print(norm(capture(dc.report_partial_closure)), end="")
    print("--- report_full_closure ---")
    print(norm(capture(dc.report_full_closure)), end="")
