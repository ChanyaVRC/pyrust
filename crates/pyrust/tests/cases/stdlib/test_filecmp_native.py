# CPython 3.12.3 parity tests for the native `filecmp` module (issue #3471).
#
# Everything printed here must be identical under CPython 3.12.3 on Linux and
# Windows: paths are normalised, only errno values (not messages) are shown,
# and no symlinks or permission bits are used.

import errno
import filecmp
import io
import itertools
import os
import pathlib
import stat
import sys
import tempfile
import types


def capture(fn, *args):
    buf = io.StringIO()
    old = sys.stdout
    sys.stdout = buf
    try:
        fn(*args)
    finally:
        sys.stdout = old
    return buf.getvalue()


def err(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except BaseException as e:
        return "%s: %s" % (type(e).__name__, e)
    return "no error"


# 1. Module surface -----------------------------------------------------------
print("__all__:", filecmp.__all__)
print("BUFSIZE:", filecmp.BUFSIZE)
print("DEFAULT_IGNORES:", filecmp.DEFAULT_IGNORES)
print("_cache:", filecmp._cache, type(filecmp._cache).__name__)
print("module doc head:", filecmp.__doc__.splitlines()[0])
print("public names:", sorted(n for n in dir(filecmp) if not n.startswith("__")))
print("os/stat/filterfalse/GenericAlias:",
      filecmp.os is os, filecmp.stat is stat,
      filecmp.filterfalse is itertools.filterfalse,
      filecmp.GenericAlias is types.GenericAlias)
for name in ("cmp", "cmpfiles", "clear_cache", "_sig", "_do_cmp", "_cmp", "_filter", "demo"):
    fn = getattr(filecmp, name)
    print("func:", name, fn.__name__, fn.__module__, callable(fn))
print("dircmp:", filecmp.dircmp.__name__, filecmp.dircmp.__module__,
      filecmp.dircmp.__doc__.splitlines()[0])
print("dircmp mro:", [c.__name__ for c in filecmp.dircmp.__mro__])
print("methodmap keys:", sorted(filecmp.dircmp.methodmap))
mm = filecmp.dircmp.methodmap
print("methodmap values:",
      {k: v.__name__ for k, v in sorted(mm.items())})
print("methodmap is class phases:",
      all(v is getattr(filecmp.dircmp, v.__name__) for v in mm.values()))
print("dircmp[str]:", filecmp.dircmp[str])
alias = filecmp.dircmp[int, str]
print("dircmp[int, str]:", alias, alias.__origin__ is filecmp.dircmp, alias.__args__)

# 2. Signature errors -----------------------------------------------------------
print("cmp():", err(filecmp.cmp))
print("cmp(1):", err(filecmp.cmp, "a"))
print("cmp too many:", err(filecmp.cmp, "a", "b", True, 1))
print("cmp kw:", err(filecmp.cmp, "a", "b", nope=1))
print("clear_cache(1):", err(filecmp.clear_cache, 1))
print("cmpfiles():", err(filecmp.cmpfiles, "a", "b"))
print("dircmp():", err(filecmp.dircmp, "a"))
print("dircmp shallow:", err(filecmp.dircmp, "a", "b", shallow=True))
print("dircmp 5 pos:", err(filecmp.dircmp, "a", "b", None, None, True))
print("_filter():", err(filecmp._filter, []))
print("_sig():", err(filecmp._sig))

with tempfile.TemporaryDirectory() as tmp:
    d1 = os.path.join(tmp, "dir1")
    d2 = os.path.join(tmp, "dir2")
    os.mkdir(d1)
    os.mkdir(d2)

    def write_file(path, content, mtime=1_700_000_000):
        with open(path, "wb") as fp:
            fp.write(content)
        os.utime(path, (mtime, mtime))

    def norm(text):
        return text.replace(tmp, "<TMP>").replace("\\", "/")

    # 3. _sig / _filter / _cmp helpers -------------------------------------------
    fa = os.path.join(d1, "same.txt")
    fb = os.path.join(d2, "same.txt")
    write_file(fa, b"hello world\n")
    write_file(fb, b"hello world\n")
    sig = filecmp._sig(os.stat(fa))
    print("_sig:", type(sig).__name__, len(sig), sig[0] == stat.S_IFREG, sig[1],
          type(sig[2]).__name__, sig[2] == 1_700_000_000)
    print("_sig dir type:", filecmp._sig(os.stat(d1))[0] == stat.S_IFDIR)
    print("_filter:", filecmp._filter(["a", "b", "c", "b"], ["b"]),
          filecmp._filter(iter("abc"), "b"), filecmp._filter([], []))
    print("_cmp:", filecmp._cmp(fa, fb, False), filecmp._cmp(fa, fb, True),
          filecmp._cmp(fa, os.path.join(d1, "nope"), True),
          filecmp._cmp(fa, d1, False))
    print("_cmp custom abs/cmp:",
          filecmp._cmp("x", "y", 0, abs, lambda a, b, s: (a, b, s) == ("x", "y", 0)),
          filecmp._cmp("x", "y", 0, lambda v: 0, lambda a, b, s: True),
          filecmp._cmp("x", "y", 0, abs, lambda a, b, s: (_ for _ in ()).throw(OSError("x"))))
    print("_cmp ValueError propagates:",
          err(filecmp._cmp, "x", "y", 0, abs, lambda a, b, s: (_ for _ in ()).throw(ValueError("v"))))

    # 4. cmp(): contents, sizes, shallow, non-regular files ----------------------
    same_len_1 = os.path.join(tmp, "same_len_1.txt")
    same_len_2 = os.path.join(tmp, "same_len_2.txt")
    diff_len_1 = os.path.join(tmp, "diff_len_1.txt")
    diff_len_2 = os.path.join(tmp, "diff_len_2.txt")
    empty_1 = os.path.join(tmp, "empty_1")
    empty_2 = os.path.join(tmp, "empty_2")
    big_1 = os.path.join(tmp, "big_1.bin")
    big_2 = os.path.join(tmp, "big_2.bin")
    big_3 = os.path.join(tmp, "big_3.bin")
    other_mtime = os.path.join(tmp, "other_mtime.txt")
    other_mtime_diff = os.path.join(tmp, "other_mtime_diff.txt")
    write_file(same_len_1, b"aaaa")
    write_file(same_len_2, b"aaab")
    write_file(diff_len_1, b"short")
    write_file(diff_len_2, b"much longer content")
    write_file(empty_1, b"")
    write_file(empty_2, b"")
    size = filecmp.BUFSIZE * 3 + 7
    write_file(big_1, b"x" * size)
    write_file(big_2, b"x" * size)
    write_file(big_3, b"x" * (size - 1) + b"y")
    write_file(other_mtime, b"hello world\n", mtime=1_600_000_000)
    write_file(other_mtime_diff, b"hello w0rld\n", mtime=1_600_000_000)

    filecmp.clear_cache()
    print("cmp same:", filecmp.cmp(fa, fb), filecmp.cmp(fa, fb, shallow=False),
          filecmp.cmp(fa, fb, True), filecmp.cmp(fa, fb, 0))
    print("cmp self:", filecmp.cmp(fa, fa), filecmp.cmp(fa, fa, False))
    print("cmp same size diff content:",
          filecmp.cmp(same_len_1, same_len_2), filecmp.cmp(same_len_1, same_len_2, False))
    print("cmp diff size:", filecmp.cmp(diff_len_1, diff_len_2),
          filecmp.cmp(diff_len_1, diff_len_2, False))
    print("cmp empty:", filecmp.cmp(empty_1, empty_2), filecmp.cmp(empty_1, empty_2, False))
    print("cmp big:", filecmp.cmp(big_1, big_2), filecmp.cmp(big_1, big_2, False),
          filecmp.cmp(big_1, big_3), filecmp.cmp(big_1, big_3, False))
    print("cmp mtime differs:", filecmp.cmp(fa, other_mtime), filecmp.cmp(fa, other_mtime, False),
          filecmp.cmp(fa, other_mtime_diff), filecmp.cmp(fa, other_mtime_diff, False))
    print("cmp dirs:", filecmp.cmp(d1, d2), filecmp.cmp(d1, d1, False),
          filecmp.cmp(d1, fa), filecmp.cmp(fa, d1, False))
    print("cmp bool results:", type(filecmp.cmp(fa, fb)).__name__,
          type(filecmp.cmp(fa, d1)).__name__, type(filecmp.cmp(big_1, big_3, False)).__name__)

    # Errors from os.stat propagate out of cmp().
    missing = os.path.join(tmp, "missing")
    for args in ((fa, missing), (missing, fa)):
        try:
            filecmp.cmp(*args)
        except FileNotFoundError as e:
            print("cmp missing:", type(e).__name__, e.errno == errno.ENOENT)
    print("cmp bad type:", err(filecmp.cmp, None, fa).split(":")[0])
    print("cmp embedded NUL:", err(filecmp.cmp, fa + "\0x", fb).split(":")[0])

    # os.PathLike and bytes inputs.
    pa, pb = pathlib.Path(fa), pathlib.Path(fb)
    print("cmp Path:", filecmp.cmp(pa, pb), filecmp.cmp(pa, pb, False),
          filecmp.cmp(pa, fb, shallow=False), filecmp.cmp(fa, pathlib.Path(big_1), False))
    print("cmp bytes:", filecmp.cmp(os.fsencode(fa), os.fsencode(fb), False),
          filecmp.cmp(os.fsencode(same_len_1), os.fsencode(same_len_2), False))

    # 5. _cache: key shape, hits, eviction, clear_cache ---------------------------
    filecmp.clear_cache()
    print("cache after clear:", len(filecmp._cache))
    filecmp.cmp(fa, fb)  # shallow hit: nothing cached
    filecmp.cmp(fa, d1, False)  # non-regular: nothing cached
    filecmp.cmp(diff_len_1, diff_len_2, False)  # size mismatch: nothing cached
    print("cache after shortcuts:", len(filecmp._cache))
    filecmp.cmp(same_len_1, same_len_2, False)
    filecmp.cmp(big_1, big_2, False)
    print("cache size:", len(filecmp._cache))
    key, value = next(iter(filecmp._cache.items()))
    print("cache key shape:", len(key), key[0] == same_len_1, key[1] == same_len_2,
          key[2] == filecmp._sig(os.stat(same_len_1)), key[3] == filecmp._sig(os.stat(same_len_2)),
          value)
    print("cache values:", list(filecmp._cache.values()))

    # A cached verdict is returned without re-reading the files.
    filecmp._cache[key] = "poison"
    print("cache hit:", filecmp.cmp(same_len_1, same_len_2, False))
    calls = []
    real_do_cmp = filecmp._do_cmp
    filecmp._do_cmp = lambda a, b: calls.append((a, b)) or real_do_cmp(a, b)
    try:
        print("cache hit calls:", len(calls))
        filecmp.cmp(same_len_1, same_len_2, False)
        print("after poisoned hit:", len(calls))
        filecmp.cmp(big_1, big_2, False)
        print("after cached big:", len(calls))
        filecmp.cmp(big_1, big_3, False)
        print("after new pair:", len(calls), calls[0][0] == big_1, calls[0][1] == big_3)
        filecmp.cmp(big_1, big_3, False)
        print("after repeated:", len(calls))
        filecmp.clear_cache()
        filecmp.cmp(big_1, big_3, False)
        print("after clear:", len(calls))
    finally:
        filecmp._do_cmp = real_do_cmp

    # The key includes the stat signature: touching a file invalidates it.
    filecmp.clear_cache()
    filecmp.cmp(big_1, big_2, False)
    n = len(filecmp._cache)
    os.utime(big_2, (1_700_000_100, 1_700_000_100))
    filecmp.cmp(big_1, big_2, False)
    print("cache invalidated by mtime:", n, len(filecmp._cache))
    os.utime(big_2, (1_700_000_000, 1_700_000_000))

    # The cache is cleared once it holds more than 100 entries.
    filecmp.clear_cache()
    sizes = []
    for i in range(105):
        os.utime(big_2, (1_700_001_000 + i, 1_700_001_000 + i))
        filecmp.cmp(big_1, big_2, False)
        sizes.append(len(filecmp._cache))
    print("eviction sizes:", sizes[:3], sizes[98:105], max(sizes))
    os.utime(big_2, (1_700_000_000, 1_700_000_000))
    filecmp.clear_cache()

    # clear_cache() clears the live dict in place and returns None.
    live = filecmp._cache
    filecmp.cmp(big_1, big_2, False)
    print("clear_cache:", len(live), filecmp.clear_cache(), len(live), live is filecmp._cache)

    # _cache is looked up as a module global at call time.
    replacement = {}
    saved_cache = filecmp._cache
    filecmp._cache = replacement
    try:
        filecmp.cmp(big_1, big_2, False)
        print("replaced cache:", len(replacement), len(saved_cache))
        filecmp.clear_cache()
        print("replaced cache cleared:", len(replacement))
    finally:
        filecmp._cache = saved_cache

    # 6. cmpfiles ---------------------------------------------------------------
    write_file(os.path.join(d1, "diff_len.txt"), b"short")
    write_file(os.path.join(d2, "diff_len.txt"), b"much longer content")
    write_file(os.path.join(d1, "diff_same_len.txt"), b"aaaa")
    write_file(os.path.join(d2, "diff_same_len.txt"), b"aaab")
    write_file(os.path.join(d1, "big.bin"), b"x" * size)
    write_file(os.path.join(d2, "big.bin"), b"x" * size)
    os.mkdir(os.path.join(d1, "adir"))
    os.mkdir(os.path.join(d2, "adir"))
    write_file(os.path.join(d1, "left_missing_right"), b"l")
    names = ["same.txt", "diff_len.txt", "diff_same_len.txt", "big.bin", "missing.txt",
             "adir", "left_missing_right"]
    res = filecmp.cmpfiles(d1, d2, names, shallow=False)
    print("cmpfiles:", type(res).__name__, [type(x).__name__ for x in res], res)
    print("cmpfiles shallow:", filecmp.cmpfiles(d1, d2, names))
    print("cmpfiles empty:", filecmp.cmpfiles(d1, d2, []))
    print("cmpfiles generator:", filecmp.cmpfiles(d1, d2, (n for n in names[:3]), False))
    print("cmpfiles tuple/Path:", filecmp.cmpfiles(pathlib.Path(d1), pathlib.Path(d2),
                                                   tuple(names[:2]), False))
    # shallow=True trusts equal (type, size, mtime) signatures even if contents differ.
    write_file(os.path.join(d1, "same_sig.txt"), b"aaaa", mtime=1_650_000_000)
    write_file(os.path.join(d2, "same_sig.txt"), b"bbbb", mtime=1_650_000_000)
    print("cmpfiles same signature:", filecmp.cmpfiles(d1, d2, ["same_sig.txt"], True),
          filecmp.cmpfiles(d1, d2, ["same_sig.txt"], False))
    print("cmpfiles bad common:", err(filecmp.cmpfiles, d1, d2, 5))
    print("cmpfiles bad name:", err(filecmp.cmpfiles, d1, d2, [None]).split(":")[0])
    print("cmpfiles ValueError is not swallowed:",
          err(filecmp.cmpfiles, d1, d2, ["same.txt\0"]).split(":")[0])

    # os.stat failures other than OSError propagate; OSErrors become "errors".
    real_stat = os.stat

    def flaky_stat(path, *args, **kwargs):
        if str(path).endswith("locked"):
            raise PermissionError(errno.EACCES, "denied")
        return real_stat(path, *args, **kwargs)

    write_file(os.path.join(d1, "locked"), b"a")
    write_file(os.path.join(d2, "locked"), b"a")
    os.stat = flaky_stat
    try:
        print("cmpfiles unreadable:", filecmp.cmpfiles(d1, d2, ["same.txt", "locked"], False))
        print("cmp unreadable:", err(filecmp.cmp, os.path.join(d1, "locked"), fa).split(":")[0])
    finally:
        os.stat = real_stat

    # 7. dircmp: construction and lazy attributes -------------------------------
    write_file(os.path.join(d1, "only_left.txt"), b"left")
    write_file(os.path.join(d2, "only_right.txt"), b"right")
    os.mkdir(os.path.join(d1, "__pycache__"))
    os.mkdir(os.path.join(d2, "__pycache__"))
    write_file(os.path.join(d1, ".git"), b"ignored")
    write_file(os.path.join(d2, "tags"), b"ignored")
    write_file(os.path.join(d1, "funny_entry"), b"file")
    os.mkdir(os.path.join(d2, "funny_entry"))
    os.mkdir(os.path.join(d1, "sub"))
    os.mkdir(os.path.join(d2, "sub"))
    write_file(os.path.join(d1, "sub", "s.txt"), b"same")
    write_file(os.path.join(d2, "sub", "s.txt"), b"same")
    write_file(os.path.join(d1, "sub", "d.txt"), b"1")
    write_file(os.path.join(d2, "sub", "d.txt"), b"22")
    os.mkdir(os.path.join(d1, "sub", "deep"))
    os.mkdir(os.path.join(d2, "sub", "deep"))
    write_file(os.path.join(d1, "sub", "deep", "only_d1.txt"), b"x")
    write_file(os.path.join(d1, "sub", "deep", "same.txt"), b"y")
    write_file(os.path.join(d2, "sub", "deep", "same.txt"), b"y")
    os.mkdir(os.path.join(d1, "only_left_dir"))

    dc = filecmp.dircmp(d1, d2)
    print("init attrs:", sorted(vars(dc)), norm(dc.left), norm(dc.right), dc.hide,
          dc.ignore is filecmp.DEFAULT_IGNORES)
    print("no shallow attr:", hasattr(dc, "shallow"))
    print("lazy: before:", [a for a in filecmp.dircmp.methodmap if a in vars(dc)])
    print("left_list:", dc.left_list)
    print("lazy: after left_list:", sorted(a for a in filecmp.dircmp.methodmap if a in vars(dc)))
    print("right_list:", dc.right_list)
    print("common:", dc.common)
    print("lazy: after common:", sorted(a for a in filecmp.dircmp.methodmap if a in vars(dc)))
    print("left_only:", dc.left_only)
    print("right_only:", dc.right_only)
    print("common_dirs:", dc.common_dirs)
    print("lazy: after phase2:", sorted(a for a in filecmp.dircmp.methodmap if a in vars(dc)))
    print("common_files:", dc.common_files)
    print("common_funny:", dc.common_funny)
    print("same_files:", dc.same_files)
    print("diff_files:", dc.diff_files)
    print("funny_files:", dc.funny_files)
    print("lazy: after phase3:", sorted(a for a in filecmp.dircmp.methodmap if a in vars(dc)))
    print("subdirs:", sorted(dc.subdirs), [type(v).__name__ for v in dc.subdirs.values()])
    print("lazy: all:", sorted(a for a in filecmp.dircmp.methodmap if a in vars(dc)))
    print("sub:", dc.subdirs["sub"].same_files, dc.subdirs["sub"].diff_files,
          dc.subdirs["sub"].common_dirs, dc.subdirs["sub"].ignore is dc.ignore,
          dc.subdirs["sub"].hide is dc.hide)
    print("deep:", dc.subdirs["sub"].subdirs["deep"].left_only,
          dc.subdirs["sub"].subdirs["deep"].same_files)
    print("sub paths:", norm(dc.subdirs["sub"].left), norm(dc.subdirs["sub"].right))

    # The computed lists are plain, mutable lists.
    print("list types:", type(dc.left_list).__name__, type(dc.common).__name__,
          type(dc.subdirs).__name__, dc.common is dc.common)

    # Unknown attributes.
    print("unknown attr:", err(getattr, dc, "no_such_attr"))
    print("hasattr:", hasattr(dc, "no_such_attr"), hasattr(dc, "common"), hasattr(dc, "left"))
    print("dunder attr:", err(getattr, dc, "__wrapped__"))
    print("getattr default:", getattr(dc, "no_such_attr", "dflt"))
    print("direct __getattr__:", err(dc.__getattr__, "nope"),
          dc.__getattr__("left_list") == dc.left_list)

    # Custom ignore / hide.
    dc_custom = filecmp.dircmp(d1, d2, ignore=[], hide=[])
    print("custom empty:", dc_custom.left_list, dc_custom.right_list)
    dc_hide = filecmp.dircmp(d1, d2, ignore=["sub"], hide=["same.txt", "only_left.txt"])
    print("custom lists:", dc_hide.left_list, dc_hide.right_list, dc_hide.common_dirs)
    dc_kw = filecmp.dircmp(a=d1, b=d2, hide=["sub"], ignore=filecmp.DEFAULT_IGNORES)
    print("kw args:", "sub" in dc_kw.left_list, dc_kw.hide, norm(dc_kw.left))
    # hide/ignore are added with `+`, so tuples and lists must not be mixed.
    dc_tuple = filecmp.dircmp(d1, d2, ignore=("sub",), hide=("same.txt",))
    print("tuple lists:", dc_tuple.left_list)
    dc_mixed = filecmp.dircmp(d1, d2, ignore=("sub",))
    print("mixed lists:", err(getattr, dc_mixed, "left_list"))
    dc_str_hide = filecmp.dircmp(d1, d2, ignore="sub", hide="same.txt")
    print("str lists:", dc_str_hide.left_list)

    # Missing directories surface the listdir error on first use only.
    dc_missing = filecmp.dircmp(os.path.join(tmp, "nope1"), d2)
    print("missing dir:", type(dc_missing).__name__,
          err(getattr, dc_missing, "left_list").split(":")[0])
    dc_file = filecmp.dircmp(fa, d2)
    print("file as dir:", err(getattr, dc_file, "common").split(":")[0])

    # Path inputs.
    dc_path = filecmp.dircmp(pathlib.Path(d1), pathlib.Path(d2))
    print("Path inputs:", dc_path.left_only, dc_path.same_files, isinstance(dc_path.left, pathlib.Path),
          sorted(dc_path.subdirs), isinstance(dc_path.subdirs["sub"].left, str))
    print("Path report:", norm(capture(dc_path.report)).splitlines()[0])
    dc_bytes = filecmp.dircmp(os.fsencode(d1), os.fsencode(d2), ignore=[b"__pycache__"], hide=[])
    print("bytes inputs:", dc_bytes.left_only, dc_bytes.same_files, dc_bytes.common_dirs)

    # 8. Reports ---------------------------------------------------------------
    dc = filecmp.dircmp(d1, d2)
    print("--- report ---")
    print(norm(capture(dc.report)), end="")
    print("--- report (empty dirs) ---")
    e1 = os.path.join(tmp, "e1")
    e2 = os.path.join(tmp, "e2")
    os.mkdir(e1)
    os.mkdir(e2)
    print(norm(capture(filecmp.dircmp(e1, e2).report)), end="")
    print("--- report_partial_closure ---")
    print(norm(capture(dc.report_partial_closure)), end="")
    print("--- report_full_closure ---")
    print(norm(capture(dc.report_full_closure)), end="")
    print("--- report_full_closure (fresh) ---")
    print(norm(capture(filecmp.dircmp(d1, d2).report_full_closure)), end="")
    returned = []
    for meth in (dc.report, dc.report_partial_closure, dc.report_full_closure):
        capture(lambda m=meth: returned.append(m()))
    print("report returns:", returned)
    print("report sorted in place:", dc.left_only == sorted(dc.left_only))
    # `report` honours a replaced sys.stdout and sorts the lists in place.
    dc_unsorted = filecmp.dircmp(d1, d2)
    dc_unsorted.left_only = ["z", "b", "a"]
    dc_unsorted.right_only = []
    dc_unsorted.same_files = []
    dc_unsorted.diff_files = ["y", "x"]
    dc_unsorted.funny_files = []
    dc_unsorted.common_dirs = []
    dc_unsorted.common_funny = ["q", "p"]
    print(norm(capture(dc_unsorted.report)), end="")
    print("sorted in place:", dc_unsorted.left_only, dc_unsorted.diff_files,
          dc_unsorted.common_funny)

    # 9. phase methods ----------------------------------------------------------
    dc = filecmp.dircmp(d1, d2)
    print("phase0 ->", dc.phase0(), sorted(vars(dc)))
    print("phase1 ->", dc.phase1(), sorted(vars(dc)))
    print("phase2 ->", dc.phase2(), sorted(vars(dc)))
    print("phase3 ->", dc.phase3(), sorted(vars(dc)))
    print("phase4 ->", dc.phase4(), sorted(vars(dc)))
    print("subdirs not recursive:", "subdirs" in vars(dc.subdirs["sub"]))
    print("phase4_closure ->", dc.phase4_closure(), "subdirs" in vars(dc.subdirs["sub"]),
          "subdirs" in vars(dc.subdirs["sub"].subdirs["deep"]))
    print("phase on class:", filecmp.dircmp.phase1(filecmp.dircmp(d1, d2)))
    print("phase arity:", err(dc.phase0, 1), err(filecmp.dircmp.phase0))
    print("report arity:", err(dc.report, 1))

    # phase2 / phase3 tolerate OSErrors from os.stat and report them as "funny".
    os.stat = flaky_stat
    try:
        dcf = filecmp.dircmp(d1, d2)
        print("funny via stat error:", dcf.common_funny, "locked" in dcf.common_files)
        print("funny_files (stat):", dcf.funny_files)
    finally:
        os.stat = real_stat

    def value_error_stat(path, *args, **kwargs):
        if str(path).endswith("locked"):
            raise ValueError("embedded null byte")
        return real_stat(path, *args, **kwargs)

    os.stat = value_error_stat
    try:
        # Python 3.12's phase2 only catches OSError.
        print("phase2 ValueError:", err(getattr, filecmp.dircmp(d1, d2), "common_dirs"))
    finally:
        os.stat = real_stat

    # 10. Subclassing ------------------------------------------------------------
    class MyDirCmp(filecmp.dircmp):
        pass

    mdc = MyDirCmp(d1, d2)
    print("subclass subdirs:", type(mdc.subdirs["sub"]) is MyDirCmp,
          type(mdc.subdirs["sub"].subdirs["deep"]) is MyDirCmp)
    print("subclass __class_getitem__:", MyDirCmp[int], MyDirCmp.methodmap is filecmp.dircmp.methodmap)
    print("subclass report:", norm(capture(mdc.report)).splitlines()[0])

    log = []

    class Logging(filecmp.dircmp):
        def __init__(self, a, b, ignore=None, hide=None, tag="t"):
            super().__init__(a, b, ignore, hide)
            self.tag = tag
            log.append(("init", os.path.basename(a)))

        def phase1(self):
            log.append(("phase1", os.path.basename(self.left)))
            super().phase1()

        def phase4(self):
            log.append(("phase4", os.path.basename(self.left)))
            super().phase4()

    lg = Logging(d1, d2)
    # The lazy path goes through `methodmap`, i.e. the base class's phases ...
    lg.common
    print("lazy bypasses override:", log)
    # ... while explicit calls and `phase4_closure` use the overrides.
    lg.phase1()
    lg.phase4_closure()
    print("explicit/closure:", log)
    print("tag default:", lg.tag, "tag" in vars(lg.subdirs["sub"]))
    del log[:]

    class Mapped(filecmp.dircmp):
        methodmap = dict(filecmp.dircmp.methodmap, common=lambda self: setattr(self, "common", ["custom"]))
        extra = 1

    mp = Mapped(d1, d2)
    print("custom methodmap:", mp.common, err(getattr, mp, "extra_missing"),
          mp.left_list == filecmp.dircmp(d1, d2).left_list)

    class Extra(filecmp.dircmp):
        methodmap = dict(filecmp.dircmp.methodmap)
        methodmap["both"] = lambda self: setattr(self, "both", (self.left_only, self.right_only))

    ex = Extra(d1, d2)
    print("extra methodmap:", ex.both)

    class NoInit(filecmp.dircmp):
        def __init__(self):
            pass

    print("no init:", err(getattr, NoInit(), "left_list").split(":")[0])

    class Instances(filecmp.dircmp):
        __slots__ = ()

    print("slots subclass:", Instances(d1, d2).left_only)

    # Instance-level methodmap overrides the class-level one.
    inst = filecmp.dircmp(d1, d2)
    inst.methodmap = {"fake": lambda self: setattr(self, "fake", 42)}
    print("instance methodmap:", inst.fake, err(getattr, inst, "common"))

    # 11. demo() ------------------------------------------------------------------
    old_argv = sys.argv
    try:
        sys.argv = ["filecmp", e1, e2]
        print("demo:", norm(capture(filecmp.demo)), end="")
        sys.argv = ["filecmp", "-r", e1, e2]
        print("demo -r:", norm(capture(filecmp.demo)), end="")
        sys.argv = ["filecmp", d1, d2]
        print("demo tree:", norm(capture(filecmp.demo)).splitlines()[:2])
        sys.argv = ["filecmp", "-r", d1, d2]
        print("demo tree -r:", len(norm(capture(filecmp.demo)).splitlines()))
        sys.argv = ["filecmp", e1]
        print("demo one arg:", err(filecmp.demo))
        sys.argv = ["filecmp"]
        print("demo no args:", err(filecmp.demo))
        sys.argv = ["filecmp", "-x", e1, e2]
        print("demo bad option:", err(filecmp.demo))
        print("demo(1):", err(filecmp.demo, 1))
    finally:
        sys.argv = old_argv
