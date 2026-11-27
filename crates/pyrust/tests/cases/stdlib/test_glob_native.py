# CPython 3.12 parity tests for the native `glob` module (issue #3463).
# `glob` / `iglob` must walk the filesystem exactly like `Lib/glob.py`:
# lazy iteration, str/bytes patterns and root_dir, `**` recursion, hidden
# entries, trailing separators, `escape()` / `has_magic()`, and the
# TypeErrors CPython raises for mixed or invalid operands.  Directory order is
# filesystem dependent, so every listing is sorted.

import glob
import os
import pathlib
import sys
import tempfile


def norm(x):
    if isinstance(x, bytes):
        return x.replace(b"\\", b"/")
    return x.replace("\\", "/")


def show(items):
    return [norm(x) for x in sorted(items)]


def run(fn, *args, **kwargs):
    try:
        r = fn(*args, **kwargs)
        if isinstance(r, list):
            return ("list", show(r))
        return (type(r).__name__, ascii(r))
    except BaseException as e:
        return (type(e).__name__, str(e)[:100])


# 1. Module metadata
print("meta:", glob.__name__, glob.__all__, ascii(glob.__doc__))
print("sys.modules:", sys.modules["glob"] is glob)
print(
    "public:",
    sorted(n for n in ("glob", "iglob", "escape", "has_magic", "glob0", "glob1") if hasattr(glob, n)),
)
print("callables:", all(callable(getattr(glob, n)) for n in ("glob", "iglob", "escape", "has_magic")))

# 2. has_magic
for s in ["foo.py", "*.py", "a?b", "[a-z]", "a]b", "", "plain/dir", "a/**/b", b"*.py", b"plain", b"x?y", b"[0-9]", b""]:
    print("has_magic", ascii(s), glob.has_magic(s))
print("has_magic int:", run(glob.has_magic, 5))
print("has_magic None:", run(glob.has_magic, None))
print("has_magic bytearray:", run(glob.has_magic, bytearray(b"*")))
print("has_magic missing:", run(glob.has_magic))


class S(str):
    pass


class B(bytes):
    pass


print("has_magic subclass:", glob.has_magic(S("a*")), glob.has_magic(B(b"abc")))

# 3. escape
for s in ["plain.txt", "a*b?c[d].txt", "[foo]*?", "[a]*?", "", "**", "dir/*/x?", b"a*b?c[d]", b"[a]*?", b""]:
    print("escape", ascii(s), ascii(glob.escape(s)))
print("escape type:", type(glob.escape("x")).__name__, type(glob.escape(b"x")).__name__)
print("escape subclass:", ascii(glob.escape(S("a*"))), ascii(glob.escape(B(b"a*"))))
print("escape pathlike:", run(glob.escape, pathlib.PurePosixPath("a/b*")))
print("escape int:", run(glob.escape, 5))
print("escape None:", run(glob.escape, None))
print("escape missing:", run(glob.escape))
if os.name == "nt":
    for s in ["C:\\dir*\\file?[1].txt", "C:foo*", "\\\\?\\C:\\foo*\\bar?.txt", "\\\\server\\share\\a*", b"C:\\dir*\\f?"]:
        print("escape-nt", ascii(s), ascii(glob.escape(s)))
else:
    for s in ["C:\\dir*\\file?[1].txt", "/abs/a*", "//net/a?"]:
        print("escape-posix", ascii(s), ascii(glob.escape(s)))

# 4. Filesystem tree
with tempfile.TemporaryDirectory() as root:
    root = os.path.realpath(root)

    def touch(*parts, data=""):
        path = os.path.join(root, *parts)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(data)

    touch("a.txt")
    touch("b.py")
    touch("c.py")
    touch(".dotfile")
    touch(".dotpy.py")
    touch("[weird].txt")
    touch("sub1", "x.py")
    touch("sub1", "y.txt")
    touch("sub1", ".hid.py")
    touch("sub1", "deep", "z.py")
    touch("sub1", "deep", "deeper", "w.py")
    touch("sub2", "x.py")
    touch("sub2", "inner", "q.txt")
    touch(".hidden", "h.py")
    touch(".hidden", "inner", "i.py")
    touch("empty", ".keep")
    os.makedirs(os.path.join(root, "emptydir"), exist_ok=True)
    touch("a", "b", "b.txt")
    touch("a", "b", "c", "b.txt")
    touch("a", "x", "b.txt")
    touch("ab.txt")
    touch("a?b.txt") if os.name != "nt" else None

    try:
        os.symlink(os.path.join(root, "sub1"), os.path.join(root, "linkdir"), target_is_directory=True)
        os.symlink(os.path.join(root, "a.txt"), os.path.join(root, "linkfile"))
        os.symlink(os.path.join(root, "nonexistent"), os.path.join(root, "broken"))
        have_links = True
    except (OSError, NotImplementedError):
        have_links = False
    print("symlinks:", have_links)

    def g(pat, **kw):
        return run(glob.glob, pat, root_dir=root, **kw)

    def gb(pat, **kw):
        return run(glob.glob, pat, root_dir=os.fsencode(root), **kw)

    def full(pat, **kw):
        # absolute pattern: results start with the escaped root
        pattern = glob.escape(os.path.join(root, pat))
        res = glob.glob(pattern, **kw)
        prefix = norm(root).rstrip("/") + "/"
        return ("list", [norm(x)[len(prefix):] if norm(x).startswith(prefix) else norm(x) for x in sorted(res)])

    # 4a. basic patterns
    for pat in ["*", "*.py", "?.py", "[ab].*", "[!a]*", "*.txt", "a*", "[[]weird].txt", "*[*]*", "sub*", "sub[12]/*", "sub?/x.py",
                "*/x.py", "*/*/*.py", "*/*/*/*.py", "nonexistent*", "nonexistent", "a.txt", "sub1", "sub1/x.py", "sub1/nope.py",
                "a/b/b.txt", "a/*/b.txt", "a/*/*/b.txt", "*/b/b.txt", "a/b*"]:
        print("glob", ascii(pat), g(pat))

    # 4b. hidden handling
    for pat in [".*", ".*.py", ".h*", ".hidden/*", ".hidden/*.py", "*/.hid*", "sub1/.*", ".dot[fp]*", "?dotfile"]:
        print("hidden", ascii(pat), g(pat))
    for pat in ["*", "*.py", "?dotfile", ".*", "*/*", "sub1/*", "*/*.py"]:
        print("include_hidden", ascii(pat), g(pat, include_hidden=True))
    print("include_hidden=False:", g("*", include_hidden=False))
    print("include_hidden truthy:", g("*.py", include_hidden=1))

    # 4c. trailing separator: directories only
    for pat in ["*/", "sub*/", "a/*/", "a/b/", "a.txt/", "sub1/", "sub1//", "nonexistent/", "*/*/", "sub1/*/", "[a]/", "sub1/x.py/", "emptydir/", "*dir/"]:
        print("trailing", ascii(pat), g(pat))
    print("trailing bytes:", gb(b"*/"))
    print("trailing bytes literal:", gb(b"sub1/"))

    # 4d. recursive
    for pat in ["**", "**/", "**/*.py", "**/x.py", "sub1/**", "sub1/**/", "sub1/**/*.py", "**/deep/*", "a/**/b.txt", "a/**/b/b.txt",
                "**/b.txt", "**/*", "*/**", "**/**", "**/**/*.py", "sub1/**/z.py", "sub*/**/*.py", "a/**", "a/**/", "**x", "x**", "**/inner/**"]:
        print("recursive", ascii(pat), g(pat, recursive=True))
        print("non-recursive", ascii(pat), g(pat, recursive=False))
    for pat in ["**", "**/*.py", "**/", "sub1/**", "a/**/b.txt"]:
        print("recursive+hidden", ascii(pat), g(pat, recursive=True, include_hidden=True))
    print("recursive default:", g("**"))
    print("recursive truthy:", g("**/x.py", recursive=1))

    # 4e. str / bytes / PathLike root_dir
    for pat in [b"*", b"*.py", b"sub1/*", b"**", b"**/*.py", b"sub1/", b".*", b"nonexistent*", b"a/*/b.txt", b"[ab].*"]:
        print("bytes", ascii(pat), gb(pat, recursive=True))
    print("root_dir Path:", run(glob.glob, "*.py", root_dir=pathlib.Path(root)))
    print("root_dir PurePath:", run(glob.glob, "sub1/*", root_dir=pathlib.PurePath(root)))
    print("root_dir bytes:", run(glob.glob, b"*.py", root_dir=os.fsencode(root)))
    print("root_dir None:", run(glob.glob, os.path.join(glob.escape(root), "*.py")) == ("list", show(
        [os.path.join(root, "b.py"), os.path.join(root, "c.py")]
    )))
    print("root_dir empty:", run(glob.glob, "nonexistent*", root_dir=""))
    print("root_dir missing dir:", run(glob.glob, "*", root_dir=os.path.join(root, "no_such_dir")))
    print("root_dir file:", run(glob.glob, "*", root_dir=os.path.join(root, "a.txt")))
    print("root_dir int:", run(glob.glob, "*", root_dir=5))
    print("root_dir list:", run(glob.glob, "*", root_dir=[]))

    # 4f. mixed str / bytes
    print("mix literal str/bytes root:", run(glob.glob, "a.txt", root_dir=os.fsencode(root)))
    print("mix literal bytes/str root:", run(glob.glob, b"a.txt", root_dir=root))
    print("mix magic str/bytes root:", run(glob.glob, "*.py", root_dir=os.fsencode(root)))
    print("mix magic bytes/str root:", run(glob.glob, b"*.py", root_dir=root))
    print("mix dir str/bytes root:", run(glob.glob, "sub1/*.py", root_dir=os.fsencode(root)))
    print("mix dir bytes/str root:", run(glob.glob, b"sub1/*.py", root_dir=root))
    print("mix trailing:", run(glob.glob, "sub1/", root_dir=os.fsencode(root)))
    print("mix recursive:", run(glob.glob, "**", root_dir=os.fsencode(root), recursive=True))
    print("mix recursive 2:", run(glob.glob, b"**", root_dir=root, recursive=True))
    print("mix empty root str/bytes:", run(glob.glob, b"*.py", root_dir=""))

    # 4g. absolute patterns and result types
    for pat in ["*.py", "sub1/*.py", "*/x.py", "a/*/b.txt", "**"]:
        print("absolute", ascii(pat), full(pat))
    res = glob.glob("*.py", root_dir=root)
    print("types:", {type(x).__name__ for x in res}, type(res).__name__)
    res = glob.glob(b"*.py", root_dir=os.fsencode(root))
    print("types bytes:", {type(x).__name__ for x in res}, type(res).__name__)
    print("literal absolute:", norm(glob.glob(os.path.join(root, "a.txt"))[0]) == norm(os.path.join(root, "a.txt")))
    print("literal absolute missing:", glob.glob(os.path.join(root, "no.txt")))
    print("literal root_dir:", glob.glob("a.txt", root_dir=root), glob.glob("sub1/x.py", root_dir=root), glob.glob("sub1/", root_dir=root))
    print("literal dotdot:", show(glob.glob("sub1/../a.txt", root_dir=root)), show(glob.glob("sub1/../*.py", root_dir=root)))
    print("literal dot:", show(glob.glob("./a.txt", root_dir=root)), show(glob.glob("./*.py", root_dir=root)))
    print("literal repeated sep:", show(glob.glob("sub1//x.py", root_dir=root)), show(glob.glob("sub1//*.py", root_dir=root)))
    print("literal weird:", glob.glob(glob.escape("[weird].txt"), root_dir=root), glob.glob("[weird].txt", root_dir=root))
    print("escape roundtrip:", [glob.glob(glob.escape(n), root_dir=root) for n in ["[weird].txt", "a.txt"]])
    print("escape no magic:", glob.escape("[weird].txt"), glob.glob("[[]weird[]].txt", root_dir=root))

    # 4h. symlinks
    if have_links:
        for pat in ["link*", "linkdir/*", "linkdir/", "*/x.py", "linkfile/", "broken", "broken*", "linkdir/**/*.py", "**/x.py", "**/link*"]:
            print("symlink", ascii(pat), g(pat, recursive=True))
        print("symlink nonrec:", g("**/*.py"))

    # 4i. current-directory relative patterns
    cwd = os.getcwd()
    os.chdir(root)
    try:
        for pat in ["*.py", "sub1/*.py", "**/*.py", "*/", "./*.py", ".", "..", "./", "", "a.txt", "nonexistent", "sub1/../*.py", "**", "./**"]:
            print("cwd", ascii(pat), run(glob.glob, pat, recursive=True))
        print("cwd bytes:", run(glob.glob, b"*.py"), run(glob.glob, b"**", recursive=True))
        print("cwd hidden:", run(glob.glob, "*", include_hidden=True))
        print("cwd root_dir None vs '.':", run(glob.glob, "*.py", root_dir=None) == run(glob.glob, "*.py", root_dir="."))
    finally:
        os.chdir(cwd)

    # 4j. empty / odd patterns
    for pat in ["", "/", "//nonexistent*", b"", "*" * 3, "[", "[]", "[!]", "[a", "a]", "?", "??", "?" * 40, "*/", "**/**/**", ".", ".."]:
        print("odd", ascii(pat), g(pat, recursive=True) if not isinstance(pat, bytes) else gb(pat, recursive=True))
    print("odd nul:", g("a\x00*"), g("a\x00b"))
    print("odd nul bytes:", gb(b"a\x00*"), gb(b"a\x00b"))

    # 5. glob0 / glob1 (not public, but used by third-party code)
    print("glob0:", run(glob.glob0, root, "a.txt"), run(glob.glob0, root, "nope"), run(glob.glob0, root, ""), run(glob.glob0, root + os.sep, ""))
    print("glob1:", run(glob.glob1, root, "*.py"), run(glob.glob1, root, ".*"), run(glob.glob1, root, "nope*"))
    print("glob1 bytes:", run(glob.glob1, os.fsencode(root), b"*.py"))
    print("glob1 mixed:", run(glob.glob1, os.fsencode(root), "*.py"), run(glob.glob1, root, b"*.py"))

    # 6. iglob laziness
    it = glob.iglob("*.py", root_dir=root)
    print("iglob type:", type(it).__name__, iter(it) is it, hasattr(it, "__next__"))
    print("iglob is not list:", not isinstance(it, (list, tuple)))
    first = next(it)
    rest = list(it)
    print("iglob consume:", sorted(norm(x) for x in [first] + rest))
    print("iglob exhausted:", list(it), run(next, it))
    print("iglob exhausted again:", run(next, it))

    # laziness: later directories are not listed until needed.  Deleting a
    # not-yet-visited directory between two next() calls changes the result.
    ordered = os.path.join(root, "lazy")
    os.makedirs(os.path.join(ordered, "d1"))
    os.makedirs(os.path.join(ordered, "d2"))
    for d in ("d1", "d2"):
        with open(os.path.join(ordered, d, "f.txt"), "w") as f:
            f.write("x")
    it = glob.iglob("*/f.txt", root_dir=ordered)
    seen = [next(it)]
    other = "d2" if norm(seen[0]).startswith("d1") else "d1"
    os.remove(os.path.join(ordered, other, "f.txt"))
    seen.extend(it)
    print("iglob lazy dirs:", [norm(x) for x in seen])

    # the listing of a directory is taken when the iterator reaches it
    it = glob.iglob("**/*.dat", root_dir=ordered, recursive=True)
    with open(os.path.join(ordered, "first.dat"), "w") as f:
        f.write("1")
    got = list(it)
    print("iglob late scan:", sorted(norm(x) for x in got))

    # recursive iteration yields names before descending into them
    rec = os.path.join(root, "rec")
    os.makedirs(os.path.join(rec, "p", "q"))
    with open(os.path.join(rec, "p", "q", "leaf.txt"), "w") as f:
        f.write("1")
    it = glob.iglob("**", root_dir=rec, recursive=True)
    print("iglob rec first:", norm(next(it)))
    print("iglob rec order:", [norm(x) for x in it])
    it = glob.iglob("**", root_dir=rec, recursive=True)
    print("iglob rec sorted:", sorted(norm(x) for x in it))

    # iglob is eager about argument errors that glob() would also raise first
    print("iglob bad pathname int:", run(glob.iglob, 5))
    print("iglob bad pathname None:", run(glob.iglob, None))
    print("iglob bad pathname list:", run(lambda: list(glob.iglob(["a"]))))
    print("iglob bad pathname empty list:", run(glob.iglob, []))
    print("iglob bad pathname Path:", run(glob.iglob, pathlib.PurePosixPath("a")))
    print("iglob bad root_dir:", run(glob.iglob, "*", root_dir=5))
    print("iglob mixed lazy:", type(glob.iglob("*.py", root_dir=os.fsencode(root))).__name__)
    print("iglob mixed raises on next:", run(lambda: next(glob.iglob("*.py", root_dir=os.fsencode(root)))))
    print("iglob pattern type:", type(glob.iglob("")).__name__, type(glob.iglob("**", recursive=True, root_dir=root)).__name__ in ("generator", "chain"))
    print("iglob empty:", list(glob.iglob("")), list(glob.iglob(b"")))
    print("iglob recursive first skipped:", [norm(x) for x in glob.iglob("**", root_dir=rec, recursive=True)][:1] != [""])

    # glob() / iglob() agree
    for pat in ["*", "**/*.py", "sub1/*", "a/**/b.txt", "*/", ".*"]:
        a = sorted(glob.glob(pat, root_dir=root, recursive=True, include_hidden=True))
        b = sorted(glob.iglob(pat, root_dir=root, recursive=True, include_hidden=True))
        print("agree", ascii(pat), a == b)

    # 7. call signatures
    print("glob positional kw-only:", run(glob.glob, "*", root))
    print("iglob positional kw-only:", run(glob.iglob, "*", root))
    print("glob unexpected kw:", run(glob.glob, "*", foo=1))
    print("glob missing:", run(glob.glob))
    print("iglob missing:", run(glob.iglob))
    print("glob dup:", run(glob.glob, "*", pathname="x"))
    print("glob pathname kw:", run(glob.glob, pathname="a.txt", root_dir=root))
    print("glob dir_fd None:", run(glob.glob, "a.txt", root_dir=root, dir_fd=None))
    print("glob kwargs order:", run(glob.glob, "*.py", include_hidden=True, recursive=True, dir_fd=None, root_dir=root))

    # 8. audit events
    events = []

    def hook(event, args):
        if event.startswith("glob."):
            events.append((event, len(args)))

    sys.addaudithook(hook)
    glob.glob("*.py", root_dir=root)
    list(glob.iglob("*.py", root_dir=root, recursive=True))
    print("audit:", events)

    # 9. unreadable / vanishing entries are skipped silently
    gone = os.path.join(root, "gone")
    os.makedirs(gone)
    with open(os.path.join(gone, "keep.txt"), "w") as f:
        f.write("1")
    print("vanishing dir:", run(glob.glob, "gone/*.txt", root_dir=root), run(glob.glob, "nope/*.txt", root_dir=root),
          run(glob.glob, "a.txt/*", root_dir=root), run(glob.glob, "a.txt/**", root_dir=root, recursive=True))
    print("file as dir recursive:", run(glob.glob, "a.txt/**/x", root_dir=root, recursive=True))

    # 10. functions are plain functions (bound state is not shared)
    print("callable names:", all(callable(getattr(glob, n)) for n in ("glob", "iglob", "escape", "has_magic")))
    print("glob.glob.__name__:", glob.glob.__name__, glob.iglob.__name__, glob.escape.__name__, glob.has_magic.__name__)

# 11. patterns relative to a missing / non-directory cwd-independent root
print("missing root:", run(glob.glob, "*", root_dir="/definitely/not/here"))
print("missing root recursive:", run(glob.glob, "**", root_dir="/definitely/not/here", recursive=True))
print("missing root literal:", run(glob.glob, "x", root_dir="/definitely/not/here"))
print("missing root trailing:", run(glob.glob, "x/", root_dir="/definitely/not/here"))
