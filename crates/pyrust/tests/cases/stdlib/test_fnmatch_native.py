# CPython 3.12 parity tests for the native `fnmatch` module (issue #3452).
# `translate()` must return CPython's exact regex text; `fnmatchcase`,
# `fnmatch` and `filter` must agree with `re.compile(translate(pat)).match`
# for str and bytes, report CPython's TypeErrors, and stay linear on
# adversarial multi-`*` patterns.

import fnmatch
import os
import posixpath
import re
import sys


def run(fn, *args, **kwargs):
    try:
        r = fn(*args, **kwargs)
        return (type(r).__name__, ascii(r))
    except BaseException as e:
        return (type(e).__name__, str(e)[:100])


class S(str):
    pass


def show(x):
    return "<memoryview>" if isinstance(x, memoryview) else ascii(x)


class B(bytes):
    pass


# 1. Module metadata
print("meta:", fnmatch.__name__, fnmatch.__all__, ascii(fnmatch.__doc__))
print("sys.modules:", sys.modules["fnmatch"] is fnmatch)
print("os attrs:", fnmatch.os is os, fnmatch.posixpath is posixpath)
print("public:", sorted(n for n in ("filter", "fnmatch", "fnmatchcase", "translate", "_compile_pattern") if hasattr(fnmatch, n)))

# 2. translate(): exact regex text
translate_pats = [
    "",
    "*",
    "**",
    "***",
    "a",
    "?",
    "??",
    "*.py",
    "a*",
    "*a",
    "a*b",
    "a*b*c",
    "a*b*c*d",
    "*a*b*c*",
    "**a**b**",
    "a*?b",
    "*?",
    "?*",
    "[abc]",
    "[!abc]",
    "[^a]",
    "[^]",
    "[]a]",
    "[]]",
    "[!]]",
    "[!]",
    "[]",
    "[",
    "[abc",
    "[!abc",
    "a[b",
    "[[a]",
    "[a-z]",
    "[!a-z]",
    "[a-z-]",
    "[-a-z]",
    "[a-]",
    "[-]",
    "[--]",
    "[a--z]",
    "[a---z]",
    "[a-c-e]",
    "[a-cd-f]",
    "[z-a]",
    "[!z-a]",
    "[c-a-z]",
    "[b-ab-a]",
    "[!-a]",
    "[a-a]",
    "[a&&b]",
    "[a~~b]",
    "[a||b]",
    "[a&b~c|d]",
    "[&&]",
    "[a\\b]",
    "[a\\-b]",
    "[\\]",
    "a\\b",
    "a\\*",
    ".^$+(){}|\\",
    "foo.bar+baz(1)",
    "a b\tc\nd\re",
    "#~&-",
    "café*",
    "あ*い?",
    "\U0001f600?",
    "a\nb*",
    "[a\nb]",
    "*\n*",
    "[あ-ん]",
    "[!あ-ん]x",
]
for pat in translate_pats:
    print("translate", ascii(pat), ascii(fnmatch.translate(pat)))
print("translate-str-subclass", ascii(fnmatch.translate(S("a*b"))), type(fnmatch.translate(S("a*"))).__name__)
print("translate-bytes", run(fnmatch.translate, b""), run(fnmatch.translate, b"a"), run(fnmatch.translate, B(b"*")), run(fnmatch.translate, bytearray(b"")))
print("translate-bad", run(fnmatch.translate, 1), run(fnmatch.translate, None))
print("translate-args", run(fnmatch.translate), run(fnmatch.translate, "a", "b"), run(fnmatch.translate, pat="a?"))

# 3. re round trip of every translated pattern
subjects = ["foo.py", "", "a", "ab", "a.b", "a\nb", "ab\n", "x]", "あいx", "a-b", "-", "&", "|", "~", "\\", "^", "["]
for pat in translate_pats:
    rx = re.compile(fnmatch.translate(pat))
    print("rt", ascii(pat), "".join("1" if rx.match(s) else "0" for s in subjects), "".join("1" if fnmatch.fnmatchcase(s, pat) else "0" for s in subjects))

# 4. fnmatchcase / fnmatch with str, case sensitivity, newlines
cases = [
    ("abc", "abc"),
    ("abc", "ABC"),
    ("foo.py", "*.py"),
    ("FOO.PY", "*.py"),
    ("foo.pyc", "*.py"),
    ("a\nb", "a*b"),
    ("a\nb", "a?b"),
    ("a\nb", "*"),
    ("\n", "?"),
    ("\n", "[!a]"),
    ("a\n", "a*"),
    ("a\n", "a"),
    ("\na", "*a"),
    (".hidden", "*"),
    ("a/b/c.py", "*.py"),
    ("a/b/c.py", "a/*/c.py"),
    ("abcde", "a*c*e"),
    ("abce", "a*c*e"),
    ("ace", "a*c*e"),
    ("ae", "a*c*e"),
    ("acce", "a*c*e"),
    ("aXbYbZc", "a*b*c"),
    ("abab", "*ab*ab*"),
    ("aab", "a*ab"),
    ("ab", "a*ab"),
    ("aa", "a*a"),
    ("a", "a*a"),
    ("xabx", "*ab"),
    ("xabxab", "*ab"),
    ("]", "[]]"),
    ("a", "[]a]"),
    ("b", "[]a]"),
    ("]", "[!]]"),
    ("a", "[!]]"),
    ("a[b", "a[b"),
    ("ab", "a[b"),
    ("a-b", "[a-]b"),
    ("-b", "[a-]b"),
    ("c", "[a--c]"),
    ("-", "[a--c]"),
    ("a", "[a--c]"),
    ("&", "[a&&b]"),
    ("~", "[a~~b]"),
    ("|", "[a||b]"),
    ("^", "[^a]"),
    ("a", "[^a]"),
    ("^", "[!a]"),
    ("[", "[[a]"),
    ("a", "[z-a]"),
    ("q", "[!z-a]"),
    ("\\", "[a\\b]"),
    ("b", "[a\\b]"),
    ("a\\b", "a\\b"),
    ("a", "a\\b"),
    ("x", "[c-a-z]"),
    ("b", "[c-a-z]"),
    ("-", "[c-a-z]"),
    ("あ", "[ぁ-ん]"),
    ("あ", "[!ぁ-ん]"),
    ("café", "caf?"),
    ("café", "café*"),
    ("\U0001f600", "?"),
    ("\U0001f600x", "?x"),
    ("", "*"),
    ("", "?"),
    ("", ""),
    ("a", ""),
    ("", "[!a]"),
    (S("abc"), "a*"),
    ("abc", S("a*")),
]
for name, pat in cases:
    print("fnmatchcase", ascii(name), ascii(pat), fnmatch.fnmatchcase(name, pat))

# 5. fnmatch(): normcase applied to both arguments (platform dependent, same
# platform as the CPython reference run)
for name, pat in [("Foo.py", "Foo.*"), ("FOO.PY", "*.py"), ("foo.py", "*.PY"), ("a/b", "a/b"), ("a/b", "a\\b"), ("a\\b", "a/b"), ("A", "[a]")]:
    print("fnmatch", ascii(name), ascii(pat), fnmatch.fnmatch(name, pat))
print("fnmatch-bytes", fnmatch.fnmatch(b"foo.py", b"*.py"), fnmatch.fnmatch(b"abc", b"a?c"), fnmatch.fnmatch(b"a\nb", b"a*b"))

# 6. bytes patterns (latin-1 decoded) and TypeErrors
bytes_cases = [
    (b"hello.py", b"*.py"),
    (b"HELLO.PY", b"*.py"),
    (b"abc", b"[a-c][!x]?"),
    (b"a\nb", b"a?b"),
    (b"a\nb", b"a*b"),
    (b"foo\xffbar", b"foo*bar"),
    (b"foo\xffbar", b"foo?bar"),
    (b"\xe9", b"[\xe0-\xef]"),
    (b"\xe9", b"[!\xe0-\xef]"),
    (b"\xe9", b"\xe9"),
    (b"\xe9", b"?"),
    (b"\xe9\xe8", b"?"),
    (b"a.b", b"a.b"),
    (b"axb", b"a.b"),
    (b"", b"*"),
    (b"", b""),
    (b"a-b", b"[a-]b"),
    (b"\x00", b"\x00"),
    (b"x", b"[\\x]"),
    (bytearray(b"abc"), b"a*"),
    (memoryview(b"abc"), b"a*"),
    (B(b"abc"), b"a*"),
    (b"abc", B(b"a*")),
]
for name, pat in bytes_cases:
    print("bytes-case", show(name), show(pat), run(fnmatch.fnmatchcase, name, pat))

mismatch = [
    ("abc", b"abc"),
    (b"abc", "abc"),
    ("abc", b"*"),
    (b"abc", "*"),
    (bytearray(b"abc"), "abc"),
    (memoryview(b"abc"), "abc"),
    ("abc", bytearray(b"abc")),
    (1, "a"),
    (None, "a"),
    ("a", 1),
    ("a", None),
    ("a", ["a"]),
    (["a"], "a"),
    ((), "a"),
    ("a", 1.5),
    (b"a", 1),
]
for a, b in mismatch:
    print("mismatch", show(a), show(b), run(fnmatch.fnmatchcase, a, b), run(fnmatch.fnmatch, a, b))
print("arity", run(fnmatch.fnmatchcase), run(fnmatch.fnmatchcase, "a"), run(fnmatch.fnmatch, "a", "b", "c"), run(fnmatch.filter), run(fnmatch.filter, [], pat="a", names=[]))
print("keywords", fnmatch.fnmatchcase(name="abc", pat="a*"), fnmatch.fnmatch(pat="a*", name="abc"), fnmatch.filter(pat="a*", names=["ab", "b"]))

# 7. filter
names = ["a.py", "B.PY", "c.txt", "d.py", "e.PY", ".hidden.py", "", "x\ny.py"]
print("filter-py", fnmatch.filter(names, "*.py"))
print("filter-all", fnmatch.filter(names, "*"))
print("filter-q", fnmatch.filter(names, "?.py"))
print("filter-none", fnmatch.filter(names, "zzz"))
print("filter-empty", fnmatch.filter([], "*"), fnmatch.filter((), "[a"), fnmatch.filter(iter(()), ""))
print("filter-gen", fnmatch.filter((x for x in names), "*.txt"))
print("filter-tuple", fnmatch.filter(tuple(names), "[ac]*"))
print("filter-set-ordered", sorted(fnmatch.filter(set(names), "*.py")))
print("filter-dict", fnmatch.filter({"a.py": 1, "b.c": 2}, "*.py"))
print("filter-str", fnmatch.filter("abcab", "a"))
print("filter-bytes", fnmatch.filter([b"a.py", b"B.PY", b"c.txt", b"d\xff.py"], b"*.py"))
print("filter-bytearray", run(fnmatch.filter, [bytearray(b"a.py"), bytearray(b"b.txt")], b"*.py"))
sub = S("a.py")
print("filter-keeps-original", fnmatch.filter([sub], "*.py")[0] is sub)
print("filter-bad", run(fnmatch.filter, 1, "a"), run(fnmatch.filter, ["a", 1], "a"), run(fnmatch.filter, ["a", b"a"], "a"), run(fnmatch.filter, [b"a", "a"], b"a"), run(fnmatch.filter, ["a"], 1), run(fnmatch.filter, ["a"], None), run(fnmatch.filter, ["a"], ["a"]), run(fnmatch.filter, [1], "a"))
print("filter-bad-lazy", run(fnmatch.filter, [], 1), run(fnmatch.filter, 1, 1))


def failing():
    yield "a"
    raise RuntimeError("boom")


print("filter-gen-error", run(fnmatch.filter, failing(), "a"))

# 8. _compile_pattern keeps CPython's contract: `re.compile(translate(pat)).match`
cp = fnmatch._compile_pattern("a*b")
print("compile-pattern", cp("axxb") is not None, cp("axxc") is None, getattr(cp, "__name__", None))
cpb = fnmatch._compile_pattern(b"a*b")
print("compile-pattern-bytes", cpb(b"axxb") is not None, cpb(b"axxc") is None)
print("compile-pattern-bad", run(fnmatch._compile_pattern, 1), run(fnmatch._compile_pattern, ["a"]))

# 9. Non-exponential multi-star matching (the atomic groups in translate())
for n in (1, 5, 20, 50):
    pat = "*a" * n + "b"
    print("adversarial", n, fnmatch.fnmatchcase("a" * 100, pat), fnmatch.fnmatchcase("a" * 100 + "b", pat), fnmatch.fnmatchcase("a" * (n - 1) + "b", pat))
print("adversarial-long", fnmatch.fnmatchcase("a" * 20000, "*a" * 30 + "b"), fnmatch.fnmatchcase("a" * 20000 + "b", "*a" * 30 + "b"))
print("adversarial-qmarks", fnmatch.fnmatchcase("a" * 300, "*?" * 40 + "b"), fnmatch.fnmatchcase("a" * 300 + "b", "*?" * 40 + "b"))
print("adversarial-bytes", fnmatch.fnmatchcase(b"a" * 5000, b"*a" * 30 + b"b"))
print("long-subject", fnmatch.fnmatchcase("x" * 100000, "*x"), fnmatch.fnmatchcase("x" * 100000, "x*x*x"), fnmatch.fnmatchcase("x" * 100000, "*y*"))
print("many-patterns", sum(fnmatch.fnmatchcase("file%d.txt" % i, "file%d*" % (i % 7)) for i in range(2000)))

# 10. Differential fuzz: native matching vs `re.compile(translate(pat)).match`
seed = 12345


def rnd(n):
    global seed
    seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
    return (seed >> 8) % n


pat_alpha = ["a", "b", "c", "*", "*", "?", "[", "]", "!", "-", "^", "\\", "&", "|", "~", ".", "\n", "a-c", "[a-c]", "[!b]", "z-a"]
sub_alpha = ["a", "b", "c", "-", "!", "^", "\\", "&", "|", "~", "]", "[", ".", "\n", "z"]
checksum = 0
bad = []
for _ in range(120):
    pat = "".join(pat_alpha[rnd(len(pat_alpha))] for _ in range(rnd(9)))
    rx = re.compile(fnmatch.translate(pat))
    for _ in range(10):
        s = "".join(sub_alpha[rnd(len(sub_alpha))] for _ in range(rnd(7)))
        want = rx.match(s) is not None
        got = fnmatch.fnmatchcase(s, pat)
        checksum = (checksum * 31 + (7 if got else 3) + len(s)) % 1000003
        if want != got:
            bad.append((pat, s, want, got))
print("fuzz", checksum, bad)
