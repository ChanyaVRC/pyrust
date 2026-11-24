# Comprehensive CPython 3.12 parity tests for `tempfile`.

import os
import pathlib
import tempfile

print("all", tempfile.__all__)
print("prefix", tempfile.gettempprefix(), tempfile.gettempprefixb())
print("tmp-max", isinstance(tempfile.TMP_MAX, int) and tempfile.TMP_MAX > 0)

# --- gettempdir / gettempdirb / tempdir override ---
td = tempfile.gettempdir()
tdb = tempfile.gettempdirb()
print("gettempdir-types", isinstance(td, str), isinstance(tdb, bytes))
print("gettempdir-isdir", os.path.isdir(td))
print("gettempdir-bytes-eq", tdb == os.fsencode(td) if hasattr(os, "fsencode") else tdb == td.encode("utf-8"))
print("tempdir-cached", tempfile.tempdir == td)

# --- Isolated base directory for all remaining tempfile tests ---
with tempfile.TemporaryDirectory(prefix="pyrust_tf_", suffix="_base") as base:
    print("td-exists", os.path.isdir(base))
    print("td-pre-suf", os.path.basename(base).startswith("pyrust_tf_"), os.path.basename(base).endswith("_base"))

    # tempdir override & restore
    saved_tempdir = tempfile.tempdir
    try:
        tempfile.tempdir = base
        print("override-gettempdir", tempfile.gettempdir() == base)
    finally:
        tempfile.tempdir = saved_tempdir
    print("restored-gettempdir", tempfile.gettempdir() == td)

    # --- mkdtemp (str, bytes, PathLike, mixed-type error) ---
    d1 = tempfile.mkdtemp(suffix="_s", prefix="p_", dir=base)
    try:
        print("mkdtemp-str", os.path.isdir(d1), os.path.dirname(d1) == os.path.abspath(base))
        bn = os.path.basename(d1)
        print("mkdtemp-affix", bn.startswith("p_"), bn.endswith("_s"))
    finally:
        os.rmdir(d1)

    d2 = tempfile.mkdtemp(suffix=b"_bs", prefix=b"bp_", dir=base.encode("utf-8"))
    try:
        print("mkdtemp-bytes", isinstance(d2, bytes), os.path.isdir(d2.decode("utf-8")))
        bn2 = os.path.basename(d2.decode("utf-8"))
        print("mkdtemp-bytes-affix", bn2.startswith("bp_"), bn2.endswith("_bs"))
    finally:
        os.rmdir(d2.decode("utf-8"))

    d3 = tempfile.mkdtemp(dir=pathlib.Path(base))
    try:
        print("mkdtemp-pathlike", isinstance(d3, str), os.path.isdir(d3))
    finally:
        os.rmdir(d3)

    try:
        tempfile.mkdtemp(prefix=b"a", suffix="b", dir=base)
        print("mkdtemp-mix", "NO-ERROR")
    except TypeError:
        print("mkdtemp-mix", "TypeError")

    # --- mkstemp (atomic exclusive creation, os.write/os.close/os.unlink) ---
    fd, path = tempfile.mkstemp(suffix=".dat", prefix="mk_", dir=base)
    try:
        print("mkstemp-types", isinstance(fd, int) and fd >= 0, isinstance(path, str))
        print("mkstemp-exists", os.path.isfile(path))
        bn = os.path.basename(path)
        print("mkstemp-affix", bn.startswith("mk_"), bn.endswith(".dat"))
        os.write(fd, b"hello mkstemp")
    finally:
        os.close(fd)
    with open(path, "rb") as rf:
        print("mkstemp-readback", rf.read())
    os.unlink(path)
    print("mkstemp-unlinked", not os.path.exists(path))

    # Bytes mkstemp
    fdb, pathb = tempfile.mkstemp(suffix=b".bin", dir=base.encode("utf-8"))
    try:
        print("mkstemp-bytes", isinstance(pathb, bytes), pathb.endswith(b".bin"))
    finally:
        os.close(fdb)
        os.unlink(pathb.decode("utf-8"))

    # --- TemporaryFile (binary & text, seek/read/write/truncate) ---
    with tempfile.TemporaryFile(dir=base) as tf:
        tf.write(b"abc123")
        print("tf-tell", tf.tell())
        tf.seek(0)
        print("tf-read", tf.read())
        tf.seek(0)
        print("tf-read-part", tf.read(3), tf.read())
        tf.truncate(3)
        tf.seek(0)
        print("tf-trunc", tf.read())
        print("tf-flags", tf.readable(), tf.writable(), tf.seekable(), tf.isatty(), tf.closed)
    print("tf-closed-after", tf.closed)

    with tempfile.TemporaryFile(mode="w+", encoding="utf-8", dir=base) as tft:
        tft.write("line1\nline2\nline3\n")
        tft.seek(0)
        print("tft-lines", list(tft))
        tft.seek(0)
        print("tft-readline", tft.readline(), tft.readline())

    # --- NamedTemporaryFile: delete=True, delete_on_close=True (default) ---
    with tempfile.NamedTemporaryFile(
        mode="w+b", prefix="ntf_", suffix=".tmp", dir=base
    ) as ntf:
        ntf_name = ntf.name
        print("ntf-exists-open", os.path.isfile(ntf_name))
        bn = os.path.basename(ntf_name)
        print("ntf-affix", bn.startswith("ntf_"), bn.endswith(".tmp"))
        ntf.write(b"named payload")
        ntf.flush()
        ntf.seek(0)
        print("ntf-read", ntf.read())
    print("ntf-deleted-after", not os.path.exists(ntf_name))

    # NamedTemporaryFile: Python 3.12 delete=True, delete_on_close=False
    with tempfile.NamedTemporaryFile(
        mode="w+", encoding="utf-8", dir=base, delete=True, delete_on_close=False
    ) as ntf2:
        ntf2_name = ntf2.name
        ntf2.write("reopen me")
        ntf2.close()
        # File MUST still exist after close() and be readable by another open()
        print("ntf2-closed", ntf2.closed)
        print("ntf2-still-exists", os.path.isfile(ntf2_name))
        with open(ntf2_name, "r", encoding="utf-8") as rf:
            print("ntf2-reopened", rf.read())
    # Must be deleted on context manager exit
    print("ntf2-deleted-on-exit", not os.path.exists(ntf2_name))

    # NamedTemporaryFile: delete=False persists after exit
    with tempfile.NamedTemporaryFile(
        mode="w+b", dir=base, delete=False
    ) as ntf3:
        ntf3_name = ntf3.name
        ntf3.write(b"keep me")
    print("ntf3-persists", os.path.isfile(ntf3_name))
    with open(ntf3_name, "rb") as rf:
        print("ntf3-content", rf.read())
    os.unlink(ntf3_name)
    print("ntf3-manual-del", not os.path.exists(ntf3_name))

    # --- SpooledTemporaryFile (binary & text rollover) ---
    with tempfile.SpooledTemporaryFile(max_size=10, mode="w+b", dir=base) as sp:
        sp.write(b"12345")
        print("sp-before-roll", sp._rolled, sp.tell())
        sp.seek(0)
        print("sp-read-mem", sp.read())
        sp.write(b"678901")  # total 11 > 10 -> rolls over!
        print("sp-after-roll", sp._rolled, sp.tell())
        sp.seek(0)
        print("sp-read-disk", sp.read())
        print("sp-fileno-int", isinstance(sp.fileno(), int))
    print("sp-closed", sp.closed)

    # Manual rollover and text mode
    with tempfile.SpooledTemporaryFile(
        max_size=100, mode="w+", encoding="utf-8", dir=base
    ) as spt:
        spt.write("hello\nworld\n")
        print("spt-mem", spt._rolled)
        spt.rollover()
        print("spt-rolled", spt._rolled)
        spt.seek(0)
        print("spt-read", spt.read())
        spt.seek(0)
        print("spt-readlines", spt.readlines())

    # Truncate triggering rollover
    with tempfile.SpooledTemporaryFile(max_size=5, mode="w+b", dir=base) as spt2:
        spt2.truncate(5)
        print("spt2-trunc5", spt2._rolled)
        spt2.truncate(6)
        print("spt2-trunc6", spt2._rolled)

    # --- TemporaryDirectory: delete=False and manual cleanup() ---
    td_keep = tempfile.TemporaryDirectory(dir=base, delete=False)
    with td_keep as kept_path:
        with open(os.path.join(kept_path, "nested.txt"), "w") as f:
            f.write("nested")
    print("td-delete-false-kept", os.path.isdir(kept_path))
    td_keep.cleanup()
    print("td-manual-cleanup", not os.path.exists(kept_path))
    # Calling cleanup() a second time is a safe no-op
    td_keep.cleanup()
    print("td-double-cleanup", True)

    # TemporaryDirectory cleans nested files & subdirs on __exit__
    with tempfile.TemporaryDirectory(dir=base) as nested_td:
        sub = os.path.join(nested_td, "a", "b")
        os.makedirs(sub)
        with open(os.path.join(sub, "f.txt"), "w") as f:
            f.write("nested")
        print("nested-td-file", os.path.isfile(os.path.join(sub, "f.txt")))
    print("nested-td-cleaned", not os.path.exists(nested_td))

print("base-cleaned", not os.path.exists(base))
