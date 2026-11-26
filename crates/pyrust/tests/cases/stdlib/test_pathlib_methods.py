# pathlib.Path methods added for issue #3391 (CPython 3.12.3 parity):
# absolute, expanduser, chmod, lchmod, symlink_to, hardlink_to, walk,
# is_mount, is_junction, owner, group, is_fifo, is_socket, is_block_device,
# is_char_device -- plus Path.home(), which now shares expanduser's rules.
#
# Deterministic and offline: everything happens inside a fresh temporary
# directory, and only relative names, booleans, exception types, and
# CPython-exact messages are printed, so CPython and PyRust agree on POSIX
# and Windows. Creating a symlink needs a privilege on some Windows hosts;
# both interpreters then take the same "unavailable" branch.

import os
import shutil
import stat
import sys
import tempfile
import types
from pathlib import Path, PurePath

WINDOWS = sys.platform == "win32"
P = type(Path())


def check(label, fn):
    """Print the result, or only the exception type."""
    try:
        result = fn()
    except Exception as exc:
        print(label, "->", type(exc).__name__)
    else:
        print(label, "->", result)


def check_msg(label, fn):
    """Print the result, or the exception type and its CPython-exact text."""
    try:
        result = fn()
    except Exception as exc:
        print(label, "->", type(exc).__name__, exc)
    else:
        print(label, "->", result)


def set_env(name, value):
    if value is None:
        if name in os.environ:
            del os.environ[name]
    else:
        os.environ[name] = value


METHODS = [
    "absolute", "expanduser", "chmod", "lchmod", "symlink_to", "hardlink_to",
    "walk", "is_mount", "is_junction", "owner", "group", "is_fifo",
    "is_socket", "is_block_device", "is_char_device",
]

# ── presence: concrete Path only, never PurePath ──────────────────────────────

print("missing on Path:", [n for n in METHODS if not callable(getattr(Path("."), n, None))])
print("present on PurePath:", [n for n in METHODS if hasattr(PurePath("."), n)])

orig_cwd = os.getcwd()
base = tempfile.mkdtemp(prefix="pyrust_3391_")
os.chdir(base)
saved_env = {k: os.environ.get(k) for k in ("HOME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "USERNAME")}
try:
    cwd = os.getcwd()
    Path("file.txt").write_text("data")
    Path("subdir").mkdir()

    # ── absolute(): prepend the cwd, never normalise or resolve ──────────────

    print("== absolute")
    rel = Path("a/b").absolute()
    print("joined:", rel == Path(cwd, "a", "b"), rel.is_absolute(), type(rel).__name__ == P.__name__)
    print("empty is cwd:", [str(p.absolute()) == cwd for p in (Path(), Path("."), Path(""))])
    print("cwd matches:", Path().absolute() == Path.cwd())
    already = Path(cwd, "x")
    print("absolute is self:", already.absolute() is already)
    print("dotdot kept:", Path("x/../y").absolute().parts[-3:])
    print("dot dropped:", Path("./x/./y").absolute() == Path(cwd, "x", "y"))
    print("not resolved:", Path("subdir/..").absolute().name)

    class MyPath(P):
        pass

    print("subclass kept:", type(MyPath("q").absolute()).__name__)
    check_msg("absolute(1)", lambda: Path("x").absolute(1))
    if WINDOWS:
        drive = Path(cwd).drive
        print("rooted:", Path("\\foo").absolute() == Path(drive + "\\foo"))
        print("drive-relative:", Path(drive + "foo").absolute() == Path(cwd, "foo"))
        print("drive-relative dot:", Path(drive).absolute() == Path(cwd))
    else:
        print("rooted:", True)
        print("drive-relative:", True)
        print("drive-relative dot:", True)

    # ── expanduser() / home() ────────────────────────────────────────────────

    print("== expanduser")
    if WINDOWS:
        set_env("USERPROFILE", "C:\\Users\\pyrust3391")
        set_env("USERNAME", "pyrust3391")
    else:
        set_env("HOME", "/home/pyrust3391")
    print("tilde:", Path("~").expanduser().as_posix())
    print("tilde tail:", Path("~/a/b").expanduser().as_posix())
    print("home():", Path.home().as_posix(), Path.home() == Path("~").expanduser())
    print("home() type:", type(Path.home()).__name__ == P.__name__, type(MyPath.home()).__name__)
    for raw in ("a/~", "/~", "x", "y/~/z"):
        p = Path(raw)
        print("unchanged", repr(raw), p.expanduser() is p)
    print("os.path agrees:", Path("~/q").expanduser() == Path(os.path.expanduser("~/q")))
    check_msg("expanduser(1)", lambda: Path("~").expanduser(1))
    if WINDOWS:
        # ~user guesses a sibling of the current profile directory.
        print("~other:", Path("~other/x").expanduser().as_posix())
        print("~self:", Path("~pyrust3391/x").expanduser().as_posix())
        set_env("USERNAME", "somebody-else")
        check_msg("~other mismatched profile", lambda: Path("~other/x").expanduser())
        set_env("USERNAME", "pyrust3391")
        print("drive tilde unchanged:", str(Path("C:~").expanduser()))
        set_env("USERPROFILE", None)
        set_env("HOMEDRIVE", "D:")
        set_env("HOMEPATH", "\\Homes\\pyrust3391")
        print("HOMEDRIVE+HOMEPATH:", Path("~/x").expanduser().as_posix())
        set_env("HOME", "C:\\ignored-on-windows")
        print("HOME ignored:", Path.home().as_posix())
        set_env("HOMEPATH", None)
        check_msg("no home", lambda: Path("~").expanduser())
        check_msg("no home()", lambda: Path.home())
    else:
        set_env("HOME", "/")
        print("root home:", Path("~").expanduser().as_posix(), Path("~/x").expanduser().as_posix())
        set_env("HOME", "/home/pyrust3391/")
        print("trailing slash:", Path("~/x").expanduser().as_posix())
        check_msg("unknown user", lambda: Path("~no_such_user_pyrust_3391/x").expanduser())
        import pwd
        set_env("HOME", None)
        fallback = pwd.getpwuid(os.getuid()).pw_dir
        print("pwd fallback:", Path("~").expanduser() == Path(fallback))
        print("~root:", Path("~root/x").expanduser() == Path(pwd.getpwnam("root").pw_dir, "x"))
    for name, value in saved_env.items():
        set_env(name, value)

    # ── chmod() / lchmod() ───────────────────────────────────────────────────

    print("== chmod")
    f = Path("file.txt")
    f.chmod(0o444)
    print("read-only:", f.stat().st_mode & 0o222 == 0)
    f.chmod(mode=0o666)
    print("writable again:", f.stat().st_mode & 0o200 != 0)
    print("returns None:", f.chmod(0o644) is None)
    f.chmod(0o666)
    check("chmod missing", lambda: Path("missing.txt").chmod(0o644))
    check("chmod follow_symlinks=False", lambda: f.chmod(0o666, follow_symlinks=False))
    check("lchmod file", lambda: f.lchmod(0o666))
    check("lchmod missing", lambda: Path("missing.txt").lchmod(0o644))
    check_msg("os.chmod follow_symlinks=False",
              lambda: os.chmod("file.txt", 0o666, follow_symlinks=False))
    print("os.chmod in supports_follow_symlinks:", os.chmod in os.supports_follow_symlinks)
    check_msg("chmod()", lambda: f.chmod())
    check_msg("chmod('x')", lambda: f.chmod("x"))
    check_msg("chmod(1.5)", lambda: f.chmod(1.5))
    check_msg("chmod(2**40)", lambda: f.chmod(2 ** 40))
    check_msg("chmod(1, True)", lambda: f.chmod(0o666, True))
    check_msg("chmod(mode twice)", lambda: f.chmod(0o666, mode=0o666))
    check_msg("chmod(bad kw)", lambda: f.chmod(0o666, follow=True))
    check_msg("lchmod(kw-only)", lambda: f.lchmod(0o666, follow_symlinks=True))
    check_msg("lchmod()", lambda: f.lchmod())
    f.chmod(0o666)

    # ── hardlink_to() ────────────────────────────────────────────────────────

    print("== hardlink_to")
    target = Path("target.txt")
    target.write_text("one")
    hard = Path("hard.txt")
    print("returns None:", hard.hardlink_to(target) is None)
    target.write_text("two")
    print("shares data:", hard.read_text(), hard.is_file(), hard.is_symlink())
    print("str target:", Path("hard2.txt").hardlink_to("target.txt") is None, Path("hard2.txt").read_text())
    try:
        hard.hardlink_to(target)
    except OSError as exc:
        print("exists:", type(exc).__name__, exc.filename, exc.filename2 == str(hard))
    try:
        Path("hard3.txt").hardlink_to("missing.txt")
    except OSError as exc:
        print("missing:", type(exc).__name__, exc.filename, exc.filename2)
    check_msg("hardlink_to()", lambda: hard.hardlink_to())
    check_msg("hardlink_to(123)", lambda: Path("hard4.txt").hardlink_to(123))
    check_msg("hardlink_to(2 args)", lambda: hard.hardlink_to("a", "b"))

    # ── symlink_to() ─────────────────────────────────────────────────────────

    print("== symlink_to")
    link = Path("link.txt")
    try:
        link.symlink_to(target)
    except OSError:
        SYMLINKS = False
        print("symlinks unavailable")
    else:
        SYMLINKS = True
        print("file link:", link.is_symlink(), link.read_text(), link.readlink() == target)
    if SYMLINKS:
        dlink = Path("dirlink")
        print("returns None:", dlink.symlink_to("subdir", target_is_directory=True) is None)
        print("dir link:", dlink.is_symlink(), dlink.is_dir())
        auto = Path("dirlink_auto")
        auto.symlink_to(Path("subdir"))
        print("auto dir link:", auto.is_symlink(), auto.is_dir())
        dangling = Path("dangling")
        dangling.symlink_to("nowhere.txt", False)
        print("dangling:", dangling.is_symlink(), dangling.exists())
        try:
            link.symlink_to("target.txt")
        except OSError as exc:
            print("exists:", type(exc).__name__, exc.filename, exc.filename2 == str(link))
        Path("subdir", "inner.txt").write_text("inner")
        print("through dir link:", Path("dirlink", "inner.txt").read_text())
        print("absolute keeps link:", Path("dirlink").absolute().name)
        check("chmod via link", lambda: link.chmod(0o666))
        check("lchmod link", lambda: link.lchmod(0o666))
    try:
        Path("no_such_dir", "link").symlink_to("target.txt")
    except OSError as exc:
        print("bad parent:", type(exc).__name__, exc.filename, exc.filename2 == str(Path("no_such_dir", "link")))
    check_msg("symlink_to()", lambda: link.symlink_to())
    check_msg("symlink_to(123)", lambda: Path("l2").symlink_to(123))
    check_msg("symlink_to(4 args)", lambda: link.symlink_to("a", True, 1))
    check_msg("symlink_to(bad kw)", lambda: link.symlink_to("a", directory=True))

    # ── walk() ───────────────────────────────────────────────────────────────

    print("== walk")
    tree = Path("tree")
    for d in ("tree/sub1/deep", "tree/sub2", "tree/sub3/x"):
        Path(d).mkdir(parents=True)
    for fname in ("tree/a.txt", "tree/z.txt", "tree/sub1/b.txt", "tree/sub1/deep/c.txt", "tree/sub3/x/d.txt"):
        Path(fname).write_text(fname)

    gen = tree.walk()
    print("generator:", isinstance(gen, types.GeneratorType), type(gen).__name__)
    first = next(gen)
    print("top is self:", first[0] is tree, type(first[1]).__name__, type(first[2]).__name__)
    first[1].sort()
    rows = [first] + list(gen)
    print("top-down:", [(p.as_posix(), sorted(d), sorted(f)) for p, d, f in rows])

    print("sorted in place:")
    for dirpath, dirnames, filenames in tree.walk():
        dirnames.sort()
        print("  ", dirpath.as_posix(), dirnames, sorted(filenames))

    print("pruned:")
    for dirpath, dirnames, filenames in tree.walk(top_down=True):
        dirnames.sort()
        if "sub1" in dirnames:
            dirnames.remove("sub1")
        if "sub3" in dirnames:
            dirnames[dirnames.index("sub3")] = "sub3/x" if not WINDOWS else "sub3\\x"
        print("  ", dirpath.as_posix(), dirnames, sorted(filenames))

    bottom = list(tree.walk(top_down=False))
    order = [p.as_posix() for p, _, _ in bottom]
    print("bottom-up children first:",
          all(order.index(c) < order.index(p) for c in order for p in order if c.startswith(p + "/")))
    print("bottom-up set:", sorted((p.as_posix(), sorted(d), sorted(f)) for p, d, f in bottom))
    print("bottom-up last is self:", bottom[-1][0] is tree)

    print("positional args:", sorted(p.as_posix() for p, _, _ in tree.walk(False, None, False)))

    os.chdir("tree")
    try:
        print("from '.':", sorted(str(p) for p, _, _ in Path().walk()))
        print("from '': ", sorted(str(p) for p, _, _ in Path("").walk()))
    finally:
        os.chdir(base)

    print("subclass:", sorted({type(p).__name__ for p, _, _ in MyPath("tree").walk()}))
    print("missing, no on_error:", list(Path("missing_dir").walk()))
    errors = []
    print("missing, on_error:", list(Path("missing_dir").walk(on_error=errors.append)),
          [(type(e).__name__, e.filename) for e in errors])
    errors.clear()
    print("file, on_error:", list(Path("file.txt").walk(on_error=errors.append)),
          [(type(e).__name__, e.filename) for e in errors])

    def boom(error):
        raise ValueError("stop: " + type(error).__name__)

    g = Path("missing_dir").walk(on_error=boom)
    check_msg("on_error raises", lambda: next(g))
    print("exhausted after raise:", list(g))

    events = []
    lazy = Path("missing_dir").walk(on_error=lambda e: events.append("called"))
    print("lazy before next:", events)
    list(lazy)
    print("lazy after list:", events)

    print("on_error=None:", list(Path("missing_dir").walk(on_error=None)))
    print("truthy flags:", sorted(p.as_posix() for p, _, _ in tree.walk(top_down=0, follow_symlinks=[])))

    if SYMLINKS:
        Path("tree/link_to_sub1").symlink_to(Path("sub1"), target_is_directory=True)
        nofollow = {p.as_posix(): (sorted(d), sorted(f)) for p, d, f in tree.walk()}
        print("nofollow top:", nofollow["tree"])
        print("nofollow descends link:", "tree/link_to_sub1" in nofollow)
        follow = {p.as_posix(): (sorted(d), sorted(f)) for p, d, f in tree.walk(follow_symlinks=True)}
        print("follow top:", follow["tree"])
        print("follow link:", follow.get("tree/link_to_sub1"))
    check_msg("walk(5 args)", lambda: tree.walk(True, None, False, 1))
    check_msg("walk(bad kw)", lambda: tree.walk(bogus=1))
    check_msg("walk(top_down twice)", lambda: tree.walk(True, top_down=False))

    # ── is_mount() / is_junction() ───────────────────────────────────────────

    print("== is_mount / is_junction")
    anchor = Path(Path(cwd).anchor)
    print("anchor is mount:", anchor.is_mount())
    print("tempdir is mount:", Path(cwd).is_mount())
    print("relative/missing:", Path("tree").is_mount(), Path("missing").is_mount(), Path("file.txt").is_mount())
    print("junction:", Path(".").is_junction(), Path("tree").is_junction(),
          Path("missing").is_junction(), Path("file.txt").is_junction())
    if SYMLINKS:
        print("symlink is not mount/junction:", Path("dirlink").is_mount(), Path("dirlink").is_junction())
    check_msg("is_mount(1)", lambda: anchor.is_mount(1))
    check_msg("is_junction(x=1)", lambda: anchor.is_junction(x=1))

    # ── is_fifo() / is_socket() / is_block_device() / is_char_device() ──────

    print("== file kinds")

    def kinds(p):
        return (p.is_fifo(), p.is_socket(), p.is_block_device(), p.is_char_device())

    for name in ("file.txt", "tree", "missing", "file.txt/child", "missing/child"):
        print("kinds", repr(name), kinds(Path(name)))
    print("embedded NUL:", kinds(Path("bad\0name")))
    if SYMLINKS:
        print("dangling link:", kinds(Path("dangling")))
    if WINDOWS:
        print("char device:", kinds(Path("NUL")))
    else:
        print("char device:", kinds(Path("/dev/null")))
    if hasattr(os, "mkfifo"):
        os.mkfifo("fifo")
        print("fifo:", kinds(Path("fifo")))
    else:
        print("fifo:", (True, False, False, False))
    if not WINDOWS and os.geteuid() != 0:
        Path("locked").mkdir()
        os.chmod("locked", 0)
        try:
            check("permission denied propagates", lambda: Path("locked/x").is_fifo())
        finally:
            os.chmod("locked", 0o755)
    else:
        print("permission denied propagates -> PermissionError")
    check_msg("is_fifo(1)", lambda: Path("x").is_fifo(1))
    check_msg("is_socket(1)", lambda: Path("x").is_socket(1))
    check_msg("is_block_device(1)", lambda: Path("x").is_block_device(1))
    check_msg("is_char_device(kw)", lambda: Path("x").is_char_device(k=1))

    # ── owner() / group() ────────────────────────────────────────────────────

    print("== owner / group")
    if WINDOWS:
        check_msg("owner", lambda: f.owner())
        check_msg("group", lambda: f.group())
        check_msg("owner missing", lambda: Path("missing").owner())
        check_msg("group missing", lambda: Path("missing").group())
    else:
        import grp
        import pwd
        st = f.stat()
        print("owner:", f.owner() == pwd.getpwuid(st.st_uid).pw_name)
        print("group:", f.group() == grp.getgrgid(st.st_gid).gr_name)
        check("owner missing", lambda: Path("missing").owner())
        check("group missing", lambda: Path("missing").group())
        if SYMLINKS:
            print("owner via link:", link.owner() == f.owner(), link.group() == f.group())
    check_msg("owner(1)", lambda: f.owner(1))
    check_msg("group(x=1)", lambda: f.group(x=1))
finally:
    for name, value in saved_env.items():
        set_env(name, value)
    os.chdir(orig_cwd)
    for root, dirs, files in os.walk(base):
        for name in files:
            try:
                os.chmod(os.path.join(root, name), 0o666)
            except OSError:
                pass
    shutil.rmtree(base, ignore_errors=True)

print("done")
