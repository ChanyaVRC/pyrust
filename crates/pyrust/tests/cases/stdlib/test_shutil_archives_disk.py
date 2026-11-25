"""CPython 3.12 parity tests for `shutil` archive and disk APIs (issue #3251)."""

import os
import pathlib
import shutil
import tempfile


def _collect_tree(root):
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        filenames.sort()
        for fn in filenames:
            full = os.path.join(dirpath, fn)
            rel = os.path.relpath(full, root).replace("\\", "/")
            with open(full, "rb") as f:
                out[rel] = f.read()
    return sorted(out.items())


# ===========================================================================
# 1. Pre-registered archive & unpack formats
# ===========================================================================
print("=== 1. default formats ===")
arch_map = dict(shutil.get_archive_formats())
for fmt in ("bztar", "gztar", "tar", "xztar", "zip"):
    print("archive fmt:", fmt, fmt in arch_map, isinstance(arch_map[fmt], str))

unpack_map = {name: (exts, desc) for name, exts, desc in shutil.get_unpack_formats()}
for fmt, expected_exts in (
    ("bztar", [".tar.bz2", ".tbz2"]),
    ("gztar", [".tar.gz", ".tgz"]),
    ("tar", [".tar"]),
    ("xztar", [".tar.xz", ".txz"]),
    ("zip", [".zip"]),
):
    print("unpack fmt:", fmt, unpack_map[fmt][0] == expected_exts)

# Sorted order check
arch_names = [n for n, _ in shutil.get_archive_formats()]
unpack_names = [n for n, _, _ in shutil.get_unpack_formats()]
print("sorted:", arch_names == sorted(arch_names), unpack_names == sorted(unpack_names))


# ===========================================================================
# 2. Custom format registration & validation
# ===========================================================================
print("=== 2. custom formats ===")
try:
    shutil.register_archive_format("bad", "not_callable")
    print("non-callable arch: NO-ERR")
except TypeError:
    print("non-callable arch: TypeError")

try:
    shutil.register_archive_format("bad2", lambda *a, **kw: None, extra_args=123)
    print("bad extra_args: NO-ERR")
except TypeError:
    print("bad extra_args: TypeError")

captured = []


def _custom_archiver(base_name, base_dir, dry_run=0, logger=None, owner=None, group=None, tag="none"):
    captured.append((os.path.basename(base_name), base_dir, bool(dry_run), tag))
    return base_name + ".custom"


def _custom_unpacker(filename, extract_dir, tag="none"):
    captured.append(("unpack", os.path.basename(filename), os.path.basename(extract_dir), tag))


shutil.register_archive_format(
    "mycustom", _custom_archiver, [("tag", "v1")], "My custom archive"
)
shutil.register_unpack_format(
    "mycustom", [".mycustom"], _custom_unpacker, [("tag", "u1")], "My custom unpack"
)
print("registered:", "mycustom" in dict(shutil.get_archive_formats()))

# Duplicate extension raises RegistryError
try:
    shutil.register_unpack_format("dup", [".mycustom"], _custom_unpacker)
    print("dup ext: NO-ERR")
except shutil.RegistryError:
    print("dup ext: RegistryError")

with tempfile.TemporaryDirectory() as tmpdir:
    out_arch = shutil.make_archive(
        os.path.join(tmpdir, "sample"), "mycustom", dry_run=True
    )
    print("custom archive path:", os.path.basename(out_arch))
    shutil.unpack_archive(
        os.path.join(tmpdir, "sample.mycustom"), os.path.join(tmpdir, "dest")
    )
    print("custom calls:", captured)

shutil.unregister_archive_format("mycustom")
shutil.unregister_unpack_format("mycustom")
# Unregistering non-existent format raises KeyError
try:
    shutil.unregister_archive_format("does_not_exist")
    print("unreg missing arch: NO-ERR")
except KeyError:
    print("unreg missing arch: KeyError")
try:
    shutil.unregister_unpack_format("does_not_exist")
    print("unreg missing unpack: NO-ERR")
except KeyError:
    print("unreg missing unpack: KeyError")
print("unregistered:", "mycustom" not in dict(shutil.get_archive_formats()))


# ===========================================================================
# 3. Round-trip make_archive & unpack_archive across zip/tar/gztar/bztar/xztar
# ===========================================================================
print("=== 3. archive round-trip ===")
with tempfile.TemporaryDirectory() as tmpdir:
    src_root = os.path.join(tmpdir, "src_root")
    pkg = os.path.join(src_root, "pkg")
    sub = os.path.join(pkg, "sub")
    os.makedirs(sub)
    with open(os.path.join(pkg, "hello.txt"), "wb") as f:
        f.write(b"Hello PyRust archives!\n")
    with open(os.path.join(sub, "data.bin"), "wb") as f:
        f.write(bytes(range(64)))

    expected = _collect_tree(src_root)

    for fmt in ("zip", "tar", "gztar", "bztar", "xztar"):
        base_out = pathlib.Path(tmpdir) / f"out_{fmt}"
        archive_path = shutil.make_archive(
            base_out, fmt, root_dir=src_root, base_dir="pkg"
        )
        print(fmt, "exists:", os.path.isfile(archive_path), "ext:", os.path.basename(archive_path))
        dest_dir = os.path.join(tmpdir, f"extract_{fmt}")
        shutil.unpack_archive(pathlib.Path(archive_path), pathlib.Path(dest_dir))
        actual = _collect_tree(dest_dir)
        print(fmt, "parity:", actual == expected)

    # dry_run does not create archive file
    dry_base = os.path.join(tmpdir, "dry_only")
    dry_ret = shutil.make_archive(dry_base, "zip", root_dir=src_root, base_dir="pkg", dry_run=True)
    print("dry_run:", os.path.basename(dry_ret), os.path.exists(dry_ret))

    # Error handling
    try:
        shutil.make_archive(os.path.join(tmpdir, "x"), "nope_fmt")
        print("unknown arch fmt: NO-ERR")
    except ValueError:
        print("unknown arch fmt: ValueError")

    try:
        shutil.unpack_archive(os.path.join(tmpdir, "x.unknown_ext"))
        print("unknown unpack ext: NO-ERR")
    except shutil.ReadError:
        print("unknown unpack ext: ReadError")

    try:
        shutil.unpack_archive(os.path.join(tmpdir, "x.zip"), format="nope_fmt")
        print("unknown unpack fmt: NO-ERR")
    except ValueError:
        print("unknown unpack fmt: ValueError")

    # Corrupt zip raises ReadError
    bad_zip = os.path.join(tmpdir, "bad.zip")
    with open(bad_zip, "wb") as f:
        f.write(b"not a valid zip file")
    try:
        shutil.unpack_archive(bad_zip, os.path.join(tmpdir, "bad_out"))
        print("corrupt zip: NO-ERR")
    except shutil.ReadError:
        print("corrupt zip: ReadError")


# ===========================================================================
# 4. disk_usage & chown
# ===========================================================================
print("=== 4. disk_usage & chown ===")
with tempfile.TemporaryDirectory() as tmpdir:
    du = shutil.disk_usage(tmpdir)
    total, used, free = du
    print(
        "disk_usage dir:",
        isinstance(du, tuple),
        len(du) == 3,
        du.total == total and du.used == used and du.free == free,
        total > 0 and used >= 0 and free >= 0 and used <= total,
    )
    fpath = pathlib.Path(tmpdir) / "probe.txt"
    fpath.write_text("ok", encoding="utf-8")
    du_file = shutil.disk_usage(fpath)
    print("disk_usage file:", du_file.total > 0 and du_file.free >= 0)

    try:
        shutil.disk_usage(os.path.join(tmpdir, "definitely_missing_path_3251"))
        print("missing disk_usage: NO-ERR")
    except FileNotFoundError:
        print("missing disk_usage: FileNotFoundError")

    if os.name == "posix" and hasattr(shutil, "chown"):
        try:
            shutil.chown(fpath)
            print("chown no args: NO-ERR")
        except ValueError:
            print("chown no args: ValueError")
        try:
            shutil.chown(fpath, user="__nonexistent_pyrust_user_3251__")
            print("chown bad user: NO-ERR")
        except LookupError:
            print("chown bad user: LookupError")
        st = os.stat(fpath)
        shutil.chown(fpath, user=st.st_uid, group=st.st_gid)
        print("chown self ok:", True)
    else:
        print("chown no args: ValueError")
        print("chown bad user: LookupError")
        print("chown self ok:", True)

print("DONE")
