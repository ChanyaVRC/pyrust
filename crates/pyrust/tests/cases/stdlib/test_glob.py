# Comprehensive CPython 3.12 parity tests for `glob`.

import glob
import os
import pathlib
import tempfile

print("all", glob.__all__)

# --- has_magic ---
for s in [
    "foo.py",
    "*.py",
    "a?b",
    "[a-z]",
    "a]b",
    b"*.py",
    b"plain",
    b"x?y",
    b"[0-9]",
]:
    print("has_magic", repr(s), glob.has_magic(s))

# --- escape ---
for s in [
    "plain.txt",
    "a*b?c[d].txt",
    "[foo]*?",
    b"a*b?c[d]",
]:
    print("escape", repr(s), repr(glob.escape(s)))

if os.name == "nt":
    print("escape-nt-drive", repr(glob.escape(r"C:\dir*\file?[1].txt")))
    print("escape-nt-unc", repr(glob.escape(r"\\?\C:\foo*\bar?.txt")))


def _norm(items):
    out = []
    for x in items:
        if isinstance(x, bytes):
            out.append(x.replace(b"\\", b"/"))
        else:
            out.append(x.replace("\\", "/"))
    out.sort()
    return out


# --- Filesystem glob / iglob tests in an isolated temp directory ---
with tempfile.TemporaryDirectory() as root:
    # Build a deterministic directory tree:
    #   root/
    #     a.txt
    #     b.py
    #     c.py
    #     .dotfile
    #     .dotpy.py
    #     [weird].txt
    #     sub1/
    #       x.py
    #       y.txt
    #       .hidden_in_sub.py
    #       deep/
    #         z.py
    #     sub2/
    #       w.py
    #     .hiddendir/
    #       secret.py
    os.makedirs(os.path.join(root, "sub1", "deep"))
    os.makedirs(os.path.join(root, "sub2"))
    os.makedirs(os.path.join(root, ".hiddendir"))

    files = [
        "a.txt",
        "b.py",
        "c.py",
        ".dotfile",
        ".dotpy.py",
        "[weird].txt",
        os.path.join("sub1", "x.py"),
        os.path.join("sub1", "y.txt"),
        os.path.join("sub1", ".hidden_in_sub.py"),
        os.path.join("sub1", "deep", "z.py"),
        os.path.join("sub2", "w.py"),
        os.path.join(".hiddendir", "secret.py"),
    ]
    for rel in files:
        with open(os.path.join(root, rel), "w") as f:
            f.write("data")

    # Basic wildcard with root_dir (str and Path)
    print("root-star-py", _norm(glob.glob("*.py", root_dir=root)))
    print("root-path-star-py", _norm(glob.glob("*.py", root_dir=pathlib.Path(root))))
    print("root-q-py", _norm(glob.glob("?.py", root_dir=root)))
    print("root-bracket", _norm(glob.glob("[bc].py", root_dir=root)))
    print("root-star-all", _norm(glob.glob("*", root_dir=root)))

    # Hidden files: excluded by default, matched by explicit `.` or include_hidden=True
    print("hidden-explicit", _norm(glob.glob(".*", root_dir=root)))
    print("hidden-explicit-py", _norm(glob.glob(".*.py", root_dir=root)))
    print(
        "hidden-included",
        _norm(glob.glob("*", root_dir=root, include_hidden=True)),
    )
    print(
        "hidden-included-py",
        _norm(glob.glob("*.py", root_dir=root, include_hidden=True)),
    )

    # Trailing separator matches directories only
    print("dirs-only", _norm(glob.glob("*/", root_dir=root)))
    print(
        "dirs-only-hidden",
        _norm(glob.glob("*/", root_dir=root, include_hidden=True)),
    )
    print("literal-dir-slash", _norm(glob.glob("sub1/", root_dir=root)))
    print("literal-file-slash", _norm(glob.glob("a.txt/", root_dir=root)))

    # Recursive ** with recursive=True vs recursive=False
    print(
        "rec-false-starstar",
        _norm(glob.glob("**/*.py", root_dir=root, recursive=False)),
    )
    print(
        "rec-true-py",
        _norm(glob.glob("**/*.py", root_dir=root, recursive=True)),
    )
    print(
        "rec-true-py-hidden",
        _norm(
            glob.glob(
                "**/*.py",
                root_dir=root,
                recursive=True,
                include_hidden=True,
            )
        ),
    )
    print(
        "rec-true-all",
        _norm(glob.glob("**", root_dir=root, recursive=True)),
    )
    print(
        "rec-true-dirs",
        _norm(glob.glob("**/", root_dir=root, recursive=True)),
    )
    print(
        "rec-sub1",
        _norm(glob.glob("sub1/**/*.py", root_dir=root, recursive=True)),
    )

    # Escape literal brackets in filename
    escaped = glob.escape("[weird].txt")
    print("escaped-match", _norm(glob.glob(escaped, root_dir=root)))

    # Non-existent literal and non-matching pattern
    print("missing-literal", glob.glob("does_not_exist.py", root_dir=root))
    print("missing-wildcard", glob.glob("*.rs", root_dir=root))
    print("empty-pattern", glob.glob("", root_dir=root))

    # iglob returns a lazy iterator
    it = glob.iglob("**/*.py", root_dir=root, recursive=True)
    print("iglob-is-iter", iter(it) is it)
    print("iglob-list", _norm(list(it)))

    # Absolute path globbing without root_dir
    abs_matches = glob.glob(os.path.join(root, "*.py"))
    rel_from_abs = [os.path.relpath(p, root) for p in abs_matches]
    print("abs-glob", _norm(rel_from_abs))

    # Bytes pathname + bytes root_dir
    root_b = root.encode("utf-8")
    print("bytes-glob", _norm(glob.glob(b"*.py", root_dir=root_b)))
    print(
        "bytes-rec",
        _norm(glob.glob(b"**/*.py", root_dir=root_b, recursive=True)),
    )

    # Mixing bytes pathname and str root_dir raises TypeError
    try:
        glob.glob(b"*.py", root_dir=root)
        print("mix-err", "NO-ERROR")
    except TypeError:
        print("mix-err", "TypeError")

print("cleaned-up", not os.path.exists(root))
