# CPython 3.12 parity tests for the `stat` module (issues #3209, #3355).

import os
import stat
import tempfile

# ── Stat-result index constants ──────────────────────────────────────────────
print(
    stat.ST_MODE,
    stat.ST_INO,
    stat.ST_DEV,
    stat.ST_NLINK,
    stat.ST_UID,
    stat.ST_GID,
    stat.ST_SIZE,
    stat.ST_ATIME,
    stat.ST_MTIME,
    stat.ST_CTIME,
)

# ── File type constants ──────────────────────────────────────────────────────
print(
    oct(stat.S_IFDIR),
    oct(stat.S_IFCHR),
    oct(stat.S_IFBLK),
    oct(stat.S_IFREG),
    oct(stat.S_IFIFO),
    oct(stat.S_IFLNK),
    oct(stat.S_IFSOCK),
    stat.S_IFDOOR,
    stat.S_IFPORT,
    stat.S_IFWHT,
)

# ── Permission bit constants ─────────────────────────────────────────────────
print(
    oct(stat.S_ISUID),
    oct(stat.S_ISGID),
    oct(stat.S_ENFMT),
    oct(stat.S_ISVTX),
    oct(stat.S_IREAD),
    oct(stat.S_IWRITE),
    oct(stat.S_IEXEC),
    oct(stat.S_IRWXU),
    oct(stat.S_IRUSR),
    oct(stat.S_IWUSR),
    oct(stat.S_IXUSR),
    oct(stat.S_IRWXG),
    oct(stat.S_IRGRP),
    oct(stat.S_IWGRP),
    oct(stat.S_IXGRP),
    oct(stat.S_IRWXO),
    oct(stat.S_IROTH),
    oct(stat.S_IWOTH),
    oct(stat.S_IXOTH),
)

# ── File flag & Windows attribute constants ──────────────────────────────────
print(
    stat.UF_NODUMP,
    stat.UF_IMMUTABLE,
    stat.UF_APPEND,
    stat.UF_OPAQUE,
    stat.UF_NOUNLINK,
    stat.UF_COMPRESSED,
    stat.UF_HIDDEN,
    stat.SF_ARCHIVED,
    stat.SF_IMMUTABLE,
    stat.SF_APPEND,
    stat.SF_NOUNLINK,
    stat.SF_SNAPSHOT,
)
print(
    stat.FILE_ATTRIBUTE_READONLY,
    stat.FILE_ATTRIBUTE_HIDDEN,
    stat.FILE_ATTRIBUTE_SYSTEM,
    stat.FILE_ATTRIBUTE_DIRECTORY,
    stat.FILE_ATTRIBUTE_ARCHIVE,
    stat.FILE_ATTRIBUTE_DEVICE,
    stat.FILE_ATTRIBUTE_NORMAL,
    stat.FILE_ATTRIBUTE_TEMPORARY,
    stat.FILE_ATTRIBUTE_SPARSE_FILE,
    stat.FILE_ATTRIBUTE_REPARSE_POINT,
    stat.FILE_ATTRIBUTE_COMPRESSED,
    stat.FILE_ATTRIBUTE_OFFLINE,
    stat.FILE_ATTRIBUTE_NOT_CONTENT_INDEXED,
    stat.FILE_ATTRIBUTE_ENCRYPTED,
    stat.FILE_ATTRIBUTE_INTEGRITY_STREAM,
    stat.FILE_ATTRIBUTE_VIRTUAL,
    stat.FILE_ATTRIBUTE_NO_SCRUB_DATA,
)

# ── S_IFMT / S_IMODE / S_IS* predicates ──────────────────────────────────────
print(oct(stat.S_IFMT(0o100644)), oct(stat.S_IMODE(0o104755)))
print(oct(stat.S_IFMT(0o041777)), oct(stat.S_IMODE(0o041777)))

type_modes = [
    stat.S_IFDIR | 0o755,
    stat.S_IFCHR | 0o666,
    stat.S_IFBLK | 0o660,
    stat.S_IFREG | 0o644,
    stat.S_IFIFO | 0o600,
    stat.S_IFLNK | 0o777,
    stat.S_IFSOCK | 0o755,
    0,
]
predicates = [
    stat.S_ISDIR,
    stat.S_ISCHR,
    stat.S_ISBLK,
    stat.S_ISREG,
    stat.S_ISFIFO,
    stat.S_ISLNK,
    stat.S_ISSOCK,
    stat.S_ISDOOR,
    stat.S_ISPORT,
    stat.S_ISWHT,
]
for mode in type_modes:
    print(oct(mode), [fn(mode) for fn in predicates])

# ── int subclass / bool and TypeError validation ─────────────────────────────
class ModeInt(int):
    pass


print(stat.S_ISREG(ModeInt(0o100644)), stat.filemode(ModeInt(0o100644)))
print(stat.S_ISDIR(False), stat.S_IMODE(True))

for fn in (stat.S_IFMT, stat.S_IMODE, stat.S_ISDIR, stat.S_ISREG, stat.filemode):
    try:
        fn("not-an-int")
    except TypeError:
        print(fn.__name__, "TypeError")

# ── filemode formatting ──────────────────────────────────────────────────────
filemode_samples = [
    0o100644,
    0o100755,
    0o100000,
    0o040755,
    0o120777,
    0o060660,
    0o020666,
    0o010644,
    0o140755,
    0o104755,  # setuid + x -> s
    0o104644,  # setuid - x -> S
    0o102755,  # setgid + x -> s
    0o102644,  # setgid - x -> S
    0o041777,  # sticky + x -> t
    0o041776,  # sticky - x -> T
    0o107777,  # all special + rwx
    0o107666,  # all special - x
    0o000000,  # unknown file type -> ?
    0o050755,  # unknown file type -> ?
    0o170777,  # unknown file type -> ?
]
for m in filemode_samples:
    print(oct(m), stat.filemode(m))

# ── Integration with os.stat() ───────────────────────────────────────────────
with tempfile.TemporaryDirectory() as tmpdir:
    st_dir = os.stat(tmpdir)
    print(
        "dir:",
        stat.S_ISDIR(st_dir.st_mode),
        stat.S_ISREG(st_dir.st_mode),
        stat.S_ISDIR(st_dir[stat.ST_MODE]),
        stat.filemode(st_dir.st_mode)[0],
    )
    fpath = os.path.join(tmpdir, "sample.txt")
    with open(fpath, "w") as fh:
        fh.write("hello")
    st_file = os.stat(fpath)
    print(
        "file:",
        stat.S_ISREG(st_file.st_mode),
        stat.S_ISDIR(st_file.st_mode),
        stat.S_ISREG(st_file[stat.ST_MODE]),
        st_file[stat.ST_SIZE],
        stat.filemode(st_file.st_mode)[0],
    )

# ── Module surface (#3355) ───────────────────────────────────────────────────
public = sorted(n for n in dir(stat) if not n.startswith("__"))
print(len(public), public[:5], public[-5:])
print(hasattr(stat, "__all__"), stat.__name__)
print(stat.__doc__.splitlines()[0])
# IO_REPARSE_TAG_* exist only on Windows builds of CPython.
print([getattr(stat, n, None) for n in ("IO_REPARSE_TAG_SYMLINK", "IO_REPARSE_TAG_MOUNT_POINT", "IO_REPARSE_TAG_APPEXECLINK")])
print(stat._filemode_table)
print(type(stat._filemode_table), len(stat._filemode_table))
print(stat.S_ENFMT == stat.S_ISGID, stat.S_IREAD == stat.S_IRUSR, stat.S_IEXEC == stat.S_IXUSR)

# Every callable is a builtin function.
for fn in (
    stat.S_IMODE, stat.S_IFMT, stat.S_ISDIR, stat.S_ISCHR, stat.S_ISBLK,
    stat.S_ISREG, stat.S_ISFIFO, stat.S_ISLNK, stat.S_ISSOCK, stat.S_ISDOOR,
    stat.S_ISPORT, stat.S_ISWHT, stat.filemode,
):
    print(fn.__name__, type(fn).__name__)

# ── Argument validation (matches Modules/_stat.c) ────────────────────────────
class Idx:
    def __index__(self):
        return 5

class Sub(int):
    pass

fns = (
    stat.S_IMODE, stat.S_IFMT, stat.S_ISDIR, stat.S_ISREG, stat.S_ISDOOR,
    stat.S_ISPORT, stat.S_ISWHT, stat.filemode,
)
bad_values = [
    "x", 1.5, None, b"1", [1], Idx(), 1j, -1, -(1 << 70), 1 << 70,
    1 << 64, (1 << 64) - 1, 1 << 40, 1 << 32, (1 << 32) - 1, 1 << 16,
    0xFFFF, True, False, Sub(0o40755),
]
for fn in fns:
    for v in bad_values:
        try:
            r = fn(v)
        except (TypeError, OverflowError) as e:
            print(fn.__name__, type(v).__name__, type(e).__name__, e)
        else:
            print(fn.__name__, type(v).__name__, repr(r))

for call in (
    lambda: stat.S_IFMT(),
    lambda: stat.S_IFMT(1, 2),
    lambda: stat.filemode(),
    lambda: stat.S_ISDIR(),
):
    try:
        call()
    except TypeError:
        print("TypeError")

# Bits beyond the type/permission fields.
for m in (0o7777, 0o177777, 0o170000, 0o160000, 0o030000, 0o070000, 0o110000, 0o130000, 0o150000):
    print(oct(m), oct(stat.S_IFMT(m)), oct(stat.S_IMODE(m)), stat.filemode(m))
