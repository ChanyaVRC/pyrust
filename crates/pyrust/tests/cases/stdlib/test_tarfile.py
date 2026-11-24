# Comprehensive CPython 3.12 parity tests for `tarfile`.

import io
import os
import pathlib
import sys
import tarfile
import tempfile


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, isinstance(e, tarfile.TarError))


# 1. Module metadata, constants, exception hierarchy
print("in sys.modules:", "tarfile" in sys.modules)
print(
    "exceptions:",
    issubclass(tarfile.TarError, Exception),
    issubclass(tarfile.ReadError, tarfile.TarError),
    issubclass(tarfile.CompressionError, tarfile.TarError),
    issubclass(tarfile.StreamError, tarfile.TarError),
    issubclass(tarfile.ExtractError, tarfile.TarError),
    issubclass(tarfile.HeaderError, tarfile.TarError),
    tarfile.TarError.__module__,
)
print(
    "types:",
    tarfile.REGTYPE,
    tarfile.AREGTYPE,
    tarfile.LNKTYPE,
    tarfile.SYMTYPE,
    tarfile.DIRTYPE,
)
print(
    "formats:",
    tarfile.USTAR_FORMAT,
    tarfile.GNU_FORMAT,
    tarfile.PAX_FORMAT,
    tarfile.DEFAULT_FORMAT,
)

# 2. TarInfo defaults and predicates
ti = tarfile.TarInfo("sample.txt")
print(
    "tarinfo default:",
    ti.name,
    oct(ti.mode),
    ti.size,
    ti.mtime,
    ti.uid,
    ti.gid,
    ti.uname,
    ti.gname,
    ti.linkname,
    ti.isfile(),
    ti.isreg(),
    ti.isdir(),
    ti.issym(),
    ti.islnk(),
)
dti = tarfile.TarInfo("mydir")
dti.type = tarfile.DIRTYPE
sti = tarfile.TarInfo("mylink")
sti.type = tarfile.SYMTYPE
sti.linkname = "sample.txt"
lti = tarfile.TarInfo("myhard")
lti.type = tarfile.LNKTYPE
lti.linkname = "sample.txt"
print("predicates:", dti.isdir(), dti.isfile(), sti.issym(), lti.islnk())

# 3. Read real CPython-generated PAX .tar.gz archive (with dir + long filename > 100 chars)
TARGZ_CPY = bytes.fromhex(
    "1f8b08003702066b02ffedd5416a84301805e0ac7b8aff04fa2746b32a7439cb5e219dc9a8e02443cc80ede96b6d"
    "29585aba8ab4f8be4d4417828fe71b6f4fa73e9622279e99ba5eced9d773b996ba56da18d6cb7da38c11548b0ddc"
    "c664e3fc4ab14fe37bfe6d742ef5be2dd294b2e4df68fd63fe9af53a7f291bad0431f2cfae73c310e81cc3858ed7"
    "e7d4054ff3f728da97bbfd3c113b569445f9f068a783b32717f3fdffbfe9fd67ff1557ab2d60c94ab2a009fdcf4e"
    "5692ae3675f71f436037f53637bbaedf5fd9ffac29ffb6ffac9af5fe576c8cc6fe6f6108be256f2f8ecefde0e818"
    "7c723ea11700000000000000000000000000ffd12bb3825ee600280000"
)
print("is_tarfile cpy gz:", tarfile.is_tarfile(io.BytesIO(TARGZ_CPY)))
print("is_tarfile bad:", tarfile.is_tarfile(io.BytesIO(b"not a tar archive")))

for mode in ("r", "r:*", "r:gz"):
    with tarfile.open(fileobj=io.BytesIO(TARGZ_CPY), mode=mode) as tf:
        names = tf.getnames()
        long_expected = "subdir/" + "a" * 110 + ".txt"
        print("cpy mode", mode, "names ok:", names == ["subdir", "subdir/greeting.txt", long_expected])
        m0 = tf.getmember("subdir")
        m1 = tf.getmember("subdir/greeting.txt")
        m2 = tf.getmember(long_expected)
        print("cpy types:", m0.isdir(), tf.extractfile(m0) is None, m1.isfile(), m2.isfile())
        print("cpy greet ok:", tf.extractfile(m1).read() == b"hello from cpython tar.gz\n" * 10)
        print("cpy long ok:", tf.extractfile(m2).read() == b"long name file content")

# 4. In-memory create and read across uncompressed (`w:`) and gzip (`w:gz`) + formats
for fmt_name, fmt in [("ustar", tarfile.USTAR_FORMAT), ("gnu", tarfile.GNU_FORMAT), ("pax", tarfile.PAX_FORMAT)]:
    for wmode, rmode in [("w:", "r:"), ("w:gz", "r:gz"), ("w:gz", "r:*")]:
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode=wmode, format=fmt) as tf:
            d = tarfile.TarInfo("pkg")
            d.type = tarfile.DIRTYPE
            d.mode = 0o755
            tf.addfile(d)

            payload = b"payload for " + fmt_name.encode("ascii") + b"\n" * 20
            f = tarfile.TarInfo("pkg/data.txt")
            f.size = len(payload)
            f.mode = 0o644
            f.mtime = 1700000100
            tf.addfile(f, io.BytesIO(payload))

        buf.seek(0)
        with tarfile.open(fileobj=buf, mode=rmode) as tf:
            members = list(tf)
            assert [m.name for m in members] == ["pkg", "pkg/data.txt"]
            assert tf.extractfile("pkg/data.txt").read() == payload
print("in-memory formats & modes ok")

# PAX unicode + long filename roundtrip
pax_buf = io.BytesIO()
long_uni = "dir_" + "x" * 95 + "/\u30c6\u30b9\u30c8_file.txt"
with tarfile.open(fileobj=pax_buf, mode="w:gz", format=tarfile.PAX_FORMAT) as tf:
    u_data = "unicode tar body \u65e5\u672c\u8a9e".encode("utf-8")
    uti = tarfile.TarInfo(long_uni)
    uti.size = len(u_data)
    tf.addfile(uti, io.BytesIO(u_data))
pax_buf.seek(0)
with tarfile.open(fileobj=pax_buf, mode="r:gz") as tf:
    print("pax unicode long:", tf.getnames() == [long_uni], tf.extractfile(long_uni).read().decode("utf-8"))

# 5. Filesystem add, append ('a:'), extract, extractall, filter, is_tarfile
with tempfile.TemporaryDirectory(prefix="pyrust_tar_") as tmpdir:
    src = os.path.join(tmpdir, "src")
    os.makedirs(os.path.join(src, "nested"))
    f_a = os.path.join(src, "alpha.txt")
    f_b = os.path.join(src, "nested", "beta.txt")
    f_skip = os.path.join(src, "skip.tmp")
    with open(f_a, "wb") as fh:
        fh.write(b"alpha content\n")
    with open(f_b, "wb") as fh:
        fh.write(b"beta content " * 30)
    with open(f_skip, "wb") as fh:
        fh.write(b"should be skipped")

    tar_path = os.path.join(tmpdir, "plain.tar")
    def add_filter(info):
        if info.name.endswith(".tmp"):
            return None
        return info

    with tarfile.open(pathlib.Path(tar_path), "w:") as tf:
        tf.add(src, arcname="tree", filter=add_filter)

    print("fs is_tarfile plain:", tarfile.is_tarfile(tar_path))

    # Append a file to the plain tar archive on disk
    with tarfile.open(tar_path, "a:") as tf:
        extra_bytes = b"appended member"
        eti = tarfile.TarInfo("tree/appended.txt")
        eti.size = len(extra_bytes)
        tf.addfile(eti, io.BytesIO(extra_bytes))

    with tarfile.open(tar_path, "r:") as tf:
        print("fs plain names:", tf.getnames())
        print("fs plain appended:", tf.extractfile("tree/appended.txt").read())

    # Extract with data filter
    out_plain = os.path.join(tmpdir, "out_plain")
    os.makedirs(out_plain)
    with tarfile.open(tar_path, "r") as tf:
        tf.extractall(out_plain, filter="data")

    with open(os.path.join(out_plain, "tree", "alpha.txt"), "rb") as fh:
        print("extracted alpha:", fh.read())
    with open(os.path.join(out_plain, "tree", "nested", "beta.txt"), "rb") as fh:
        print("extracted beta ok:", fh.read() == b"beta content " * 30)
    with open(os.path.join(out_plain, "tree", "appended.txt"), "rb") as fh:
        print("extracted appended:", fh.read())
    print("skipped absent:", not os.path.exists(os.path.join(out_plain, "tree", "skip.tmp")))

    # .tar.gz on filesystem
    tgz_path = os.path.join(tmpdir, "archive.tar.gz")
    with tarfile.open(tgz_path, "w:gz") as tf:
        tf.add(f_a, arcname="alpha.txt")
        tf.add(f_b, arcname="beta.txt")

    print("fs is_tarfile tgz:", tarfile.is_tarfile(tgz_path))
    print("fs is_tarfile non-tar:", tarfile.is_tarfile(f_a))
    print("fs is_tarfile missing:", expect_exc(tarfile.is_tarfile, os.path.join(tmpdir, "nope.tar")))

    out_tgz = os.path.join(tmpdir, "out_tgz")
    os.makedirs(out_tgz)
    with tarfile.open(tgz_path, "r") as tf:
        print("fs tgz names:", tf.getnames())
        tf.extract("alpha.txt", out_tgz, filter="data")
    with open(os.path.join(out_tgz, "alpha.txt"), "rb") as fh:
        print("tgz single extract:", fh.read())

# 6. Error handling parity
print("err bad tar:", expect_exc(tarfile.open, fileobj=io.BytesIO(b"not a tar" * 100), mode="r:"))
print("err bad gz:", expect_exc(tarfile.open, fileobj=io.BytesIO(b"not gzip"), mode="r:gz"))
print("err missing member:", expect_exc(tarfile.open(fileobj=io.BytesIO(TARGZ_CPY), mode="r:gz").getmember, "nope"))
print("err bad filter:", expect_exc(tarfile.open(fileobj=io.BytesIO(TARGZ_CPY), mode="r:gz").extract, "subdir/greeting.txt", ".", filter="bogus"))

# Path traversal blocked by data_filter
evil_buf = io.BytesIO()
with tarfile.open(fileobj=evil_buf, mode="w:") as tf:
    eti = tarfile.TarInfo("../escape.txt")
    eti.size = 4
    tf.addfile(eti, io.BytesIO(b"evil"))
evil_buf.seek(0)
with tempfile.TemporaryDirectory(prefix="pyrust_tar_sec_") as tmpdir:
    with tarfile.open(fileobj=evil_buf, mode="r:") as tf:
        print("err traversal:", expect_exc(tf.extractall, tmpdir, filter="data"))
