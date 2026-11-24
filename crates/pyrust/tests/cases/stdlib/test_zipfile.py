# Comprehensive CPython 3.12 parity tests for `zipfile`.

import io
import os
import pathlib
import sys
import tempfile
import zipfile


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, isinstance(e, zipfile.BadZipFile))


# 1. Module metadata, constants, exception hierarchy
print("in sys.modules:", "zipfile" in sys.modules)
print("constants:", zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED)
print(
    "exceptions:",
    issubclass(zipfile.BadZipFile, Exception),
    issubclass(zipfile.LargeZipFile, Exception),
    zipfile.BadZipFile.__module__,
)

# 2. Read real CPython-generated ZIP archive (STORED + DEFLATED + UTF-8 + comment)
ZIP_CPY = bytes.fromhex(
    "504b03041400000000000539a6581d73ee6d0e0000000e0000000900000068656c6c6f2e74787473746f72656420"
    "636f6e74656e74504b03041400000008000639a658ec67c74319000000fe010000100000006469722f6465666c61"
    "7465642e7478744b494dcb492c494d5148cecf2b49cd2b51481915481c310200504b03041400000808000739a658"
    "7431b3a11c0000001900000012000000757466385fe38386e382b9e383882e7478742bcdcb4cce4f49552848accc"
    "c94f4c5178367de9b3396b5eac9a0700504b010214001400000000000539a6581d73ee6d0e0000000e0000000900"
    "0000000000000000000080010000000068656c6c6f2e747874504b010214001400000008000639a658ec67c74319"
    "000000fe0100001000000000000000000000008001350000006469722f6465666c617465642e747874504b010214"
    "001400000808000739a6587431b3a11c0000001900000012000000000000000000000080017c000000757466385f"
    "e38386e382b9e383882e747874504b05060000000003000300b5000000c8000000130063707974686f6e207a6970"
    "20636f6d6d656e74"
)

print("is_zipfile cpy:", zipfile.is_zipfile(io.BytesIO(ZIP_CPY)))
print("is_zipfile bad:", zipfile.is_zipfile(io.BytesIO(b"not a zip")))

with zipfile.ZipFile(io.BytesIO(ZIP_CPY), "r") as zf:
    print("cpy namelist:", zf.namelist())
    print("cpy comment:", zf.comment)
    print("cpy testzip:", zf.testzip())
    for info in zf.infolist():
        print(
            "cpy info:",
            info.filename,
            info.date_time,
            info.compress_type,
            info.file_size,
            info.CRC,
            info.is_dir(),
        )
    print("cpy read hello:", zf.read("hello.txt"))
    print("cpy read deflated:", zf.read("dir/deflated.txt") == b"deflated content " * 30)
    print("cpy read utf8:", zf.read("utf8_\u30c6\u30b9\u30c8.txt").decode("utf-8"))
    with zf.open("dir/deflated.txt", "r") as member:
        print("cpy open chunk:", member.read(17), member.tell())
        member.seek(0)
        print("cpy open seek0:", member.read(17))
    print("cpy missing key:", expect_exc(zf.getinfo, "does-not-exist.txt"))

# 3. In-memory BytesIO create (STORED + DEFLATED + ZipInfo + open('w') + append + comment)
buf = io.BytesIO()
with zipfile.ZipFile(buf, "w", compression=zipfile.ZIP_DEFLATED) as zf:
    zf.comment = b"my archive comment"
    zf.writestr("a.txt", "hello string \u30c6\u30b9\u30c8")
    zf.writestr("b.bin", b"repeat " * 200, compress_type=zipfile.ZIP_DEFLATED)
    zi = zipfile.ZipInfo("c_stored.txt", (2025, 1, 2, 3, 4, 6))
    zi.compress_type = zipfile.ZIP_STORED
    zi.comment = b"entry note"
    zf.writestr(zi, b"stored payload")
    zf.writestr("emptydir/", b"")
    with zf.open("streamed.txt", "w") as wf:
        wf.write(b"line one\n")
        wf.write(b"line two\n")

# Append to BytesIO archive
with zipfile.ZipFile(buf, "a", compression=zipfile.ZIP_DEFLATED) as zf:
    zf.writestr("appended.txt", b"appended data")

buf.seek(0)
print("is_zipfile mem:", zipfile.is_zipfile(buf))
with zipfile.ZipFile(buf, "r") as zf:
    print("mem namelist:", zf.namelist())
    print("mem comment:", zf.comment)
    print("mem testzip:", zf.testzip())
    print("mem a:", zf.read("a.txt").decode("utf-8"))
    print("mem b ok:", zf.read("b.bin") == b"repeat " * 200)
    b_info = zf.getinfo("b.bin")
    print("mem b compressed:", b_info.compress_type == zipfile.ZIP_DEFLATED, b_info.compress_size < b_info.file_size)
    c_info = zf.getinfo("c_stored.txt")
    print("mem c:", c_info.date_time, c_info.compress_type, c_info.comment, zf.read(c_info))
    d_info = zf.getinfo("emptydir/")
    print("mem dir:", d_info.is_dir())
    with zf.open("streamed.txt") as sf:
        print("mem streamed lines:", list(sf))
    print("mem appended:", zf.read("appended.txt"))

# 4. Filesystem write / append / extract / extractall / is_zipfile
with tempfile.TemporaryDirectory(prefix="pyrust_zip_") as tmpdir:
    src_dir = os.path.join(tmpdir, "src")
    os.makedirs(os.path.join(src_dir, "sub"))
    f1 = os.path.join(src_dir, "root.txt")
    f2 = os.path.join(src_dir, "sub", "nested.txt")
    with open(f1, "wb") as fh:
        fh.write(b"root file contents\n")
    with open(f2, "wb") as fh:
        fh.write(b"nested payload " * 50)

    zpath = os.path.join(tmpdir, "archive.zip")
    with zipfile.ZipFile(pathlib.Path(zpath), "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.write(f1, "root.txt")
        zf.write(f2, "sub/nested.txt")
        zf.writestr("sub/extra/", b"")

    print("fs is_zipfile:", zipfile.is_zipfile(zpath), zipfile.is_zipfile(f1))
    print("fs missing is_zipfile:", zipfile.is_zipfile(os.path.join(tmpdir, "missing.zip")))

    # Append on disk
    with zipfile.ZipFile(zpath, "a", compression=zipfile.ZIP_STORED) as zf:
        zf.writestr("added_on_disk.txt", b"disk append ok")

    out_dir = os.path.join(tmpdir, "out")
    os.makedirs(out_dir)
    with zipfile.ZipFile(zpath, "r") as zf:
        print("fs namelist:", zf.namelist())
        print("fs testzip:", zf.testzip())
        zf.extractall(out_dir)

    with open(os.path.join(out_dir, "root.txt"), "rb") as fh:
        print("extracted root:", fh.read())
    with open(os.path.join(out_dir, "sub", "nested.txt"), "rb") as fh:
        print("extracted nested ok:", fh.read() == b"nested payload " * 50)
    print("extracted dir:", os.path.isdir(os.path.join(out_dir, "sub", "extra")))
    with open(os.path.join(out_dir, "added_on_disk.txt"), "rb") as fh:
        print("extracted appended:", fh.read())

    # Single extract
    single_dir = os.path.join(tmpdir, "single")
    with zipfile.ZipFile(zpath, "r") as zf:
        extracted_path = zf.extract("root.txt", single_dir)
        print("single extract exists:", os.path.isfile(extracted_path))

# 5. Corruption detection (BadZipFile & testzip)
corrupt = bytearray(ZIP_CPY)
# Flip a byte inside hello.txt stored data (offset 39)
corrupt[40] ^= 0xFF
with zipfile.ZipFile(io.BytesIO(bytes(corrupt)), "r") as zf:
    print("corrupt testzip:", zf.testzip())
    print("corrupt read:", expect_exc(zf.read, "hello.txt"))

print("err not a zip:", expect_exc(zipfile.ZipFile, io.BytesIO(b"hello"), "r"))
print("err pre-1980:", expect_exc(zipfile.ZipInfo, "old.txt", (1979, 12, 31, 0, 0, 0)))
