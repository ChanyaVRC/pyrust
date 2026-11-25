import collections.abc
import mmap
import os
import struct
import sys
import tempfile

# 1. Constants and error alias
assert mmap.error is OSError
assert mmap.ACCESS_DEFAULT == 0
assert mmap.ACCESS_READ == 1
assert mmap.ACCESS_WRITE == 2
assert mmap.ACCESS_COPY == 3
assert isinstance(mmap.PAGESIZE, int) and mmap.PAGESIZE > 0
assert isinstance(mmap.ALLOCATIONGRANULARITY, int)
assert mmap.ALLOCATIONGRANULARITY >= mmap.PAGESIZE
assert (mmap.ALLOCATIONGRANULARITY % mmap.PAGESIZE) == 0
print("mmap_constants_ok")

# 2. Anonymous mapping: read/write/seek/tell/find/rfind/move/slicing/resize
mm = mmap.mmap(-1, 32)
assert mm.closed is False
assert len(mm) == 32
if sys.platform == "win32":
    assert mm.size() == 32
else:
    try:
        mm.size()
        assert False, "expected OSError for anonymous mmap.size() on POSIX"
    except OSError:
        pass
assert mm.tell() == 0

assert mm.write(b"hello world\nsecond line\n") == 24
assert mm.tell() == 24

assert mm.seek(0) is None
assert mm.tell() == 0
assert mm.readline() == b"hello world\n"
assert mm.readline() == b"second line\n"
assert mm.read(4) == b"\x00\x00\x00\x00"
assert mm.read() == b"\x00\x00\x00\x00"
assert mm.tell() == 32

mm.seek(0)
assert mm.read_byte() == ord("h")
assert mm.tell() == 1
mm.write_byte(ord("E"))
assert mm[:5] == b"hEllo"
assert mm[0] == ord("h")
assert mm[-1] == 0

# Seek modes (SEEK_SET=0, SEEK_CUR=1, SEEK_END=2)
mm.seek(6, 0)
assert mm.tell() == 6
mm.seek(-2, 1)
assert mm.tell() == 4
mm.seek(-4, 2)
assert mm.tell() == 28
try:
    mm.seek(-1, 0)
    assert False, "expected ValueError"
except ValueError:
    pass
try:
    mm.seek(33, 0)
    assert False, "expected ValueError"
except ValueError:
    pass

# find / rfind (including default start == tell())
mm.seek(0)
assert mm.find(b"world") == 6
assert mm.find(b"l") == 2
assert mm.rfind(b"l") == 19
assert mm.find(b"l", 5, 12) == 9
assert mm.rfind(b"l", 0, 5) == 3
assert mm.find(b"missing") == -1
mm.seek(10)
assert mm.find(b"hEllo") == -1

# Indexing and slice assignment
mm[0] = ord("H")
mm[1:5] = b"ELLO"
assert mm[:5] == b"HELLO"
mm[0:6:2] = b"ABC"
assert mm[:6] == b"AEBLC "
try:
    mm[0:2] = b"123"
    assert False, "expected IndexError for mismatched slice size"
except IndexError:
    pass

# Overlapping move (forward and backward)
mm.seek(0)
mm.write(b"0123456789abcdef")
mm.move(2, 0, 6)
assert mm[:10] == b"0101234589"
mm.move(0, 2, 6)
assert mm[:8] == b"01234545"

# Slice iteration yields byte integers; direct mmap iteration yields 1-byte bytes
assert list(mm[:4]) == [ord("0"), ord("1"), ord("2"), ord("3")]
assert [b for b in mm][:4] == [b"0", b"1", b"2", b"3"]

# flush and resize
assert mm.flush() is None
mm.resize(48)
assert len(mm) == 48
if sys.platform == "win32":
    assert mm.size() == 48
assert mm[:6] == b"012345"
assert mm[32:48] == b"\x00" * 16

print("mmap_anon_ops_ok")

# 3. PEP 688 Buffer Protocol & zero-copy memoryview integration
assert isinstance(mm, collections.abc.Buffer)
mv = memoryview(mm)
assert mv.obj is mm
assert mv.readonly is False
assert mv.format == "B"
assert mv.ndim == 1
assert mv.shape == (48,)
assert mv.nbytes == 48

# Bidirectional zero-copy mutation between memoryview and mmap
mv[0:4] = b"ZCPY"
assert mm[:4] == b"ZCPY"
mm[4:8] = b"LIVE"
assert mv[4:8].tobytes() == b"LIVE"

# bytes(), bytearray(), and struct buffer consumers
assert bytes(mm)[:8] == b"ZCPYLIVE"
assert bytearray(mm)[:8] == bytearray(b"ZCPYLIVE")
struct.pack_into("<II", mm, 8, 0x11223344, 0x55667788)
assert struct.unpack_from("<II", mm, 8) == (0x11223344, 0x55667788)
assert struct.unpack_from("<II", mv, 8) == (0x11223344, 0x55667788)

# Exported buffer guards close() and resize()
try:
    mm.close()
    assert False, "expected BufferError while memoryview is exported"
except BufferError:
    pass

try:
    mm.resize(64)
    assert False, "expected BufferError while memoryview is exported"
except BufferError:
    pass

mv.release()
mm.close()
assert mm.closed is True

# Closed mmap raises ValueError
for action in [
    lambda: mm.read(1),
    lambda: mm.tell(),
    lambda: mm.seek(0),
    lambda: mm.size(),
    lambda: mm.find(b"a"),
    lambda: len(mm),
    lambda: mm[0],
    lambda: memoryview(mm),
]:
    try:
        action()
        assert False, "expected ValueError on closed mmap"
    except ValueError:
        pass
print("mmap_buffer_protocol_ok")

# 4. File-backed mappings: ACCESS_WRITE, ACCESS_READ, ACCESS_COPY, context manager
with tempfile.TemporaryDirectory() as tmpdir:
    fpath = os.path.join(tmpdir, "sample.bin")
    with open(fpath, "wb") as f:
        f.write(b"0123456789ABCDEF")

    # ACCESS_WRITE + memoryview mutation + flush persists to file
    with open(fpath, "r+b") as f:
        with mmap.mmap(f.fileno(), 0) as fmm:
            assert len(fmm) == 16
            assert fmm.size() == 16
            assert fmm[:] == b"0123456789ABCDEF"
            fmm[0:4] = b"WXYZ"
            with memoryview(fmm) as fmv:
                fmv[12:16] = b"9876"
            fmm.flush()
        assert fmm.closed is True

    with open(fpath, "rb") as f:
        persisted = f.read()
    assert persisted == b"WXYZ456789AB9876", f"got {persisted!r}"

    # ACCESS_READ: readable, readonly memoryview, rejects writes
    with open(fpath, "rb") as f:
        with mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as rmm:
            assert rmm[:4] == b"WXYZ"
            with memoryview(rmm) as rmv:
                assert rmv.readonly is True
                assert rmv[:4].tobytes() == b"WXYZ"
            try:
                rmm[0] = 65
                assert False, "expected TypeError on readonly mmap"
            except TypeError:
                pass
            try:
                rmm.write(b"a")
                assert False, "expected TypeError on readonly mmap"
            except TypeError:
                pass

    # ACCESS_COPY: mutations stay in memory and do not alter file on disk
    with open(fpath, "r+b") as f:
        with mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_COPY) as cmm:
            cmm[0:4] = b"COPY"
            assert cmm[:4] == b"COPY"
            cmm.flush()

    with open(fpath, "rb") as f:
        after_copy = f.read()
    assert after_copy == b"WXYZ456789AB9876"

    # Empty file raises ValueError
    empty_path = os.path.join(tmpdir, "empty.bin")
    with open(empty_path, "wb") as f:
        pass
    with open(empty_path, "r+b") as f:
        try:
            mmap.mmap(f.fileno(), 0)
            assert False, "expected ValueError for empty file"
        except ValueError:
            pass

print("mmap_file_backed_ok")
