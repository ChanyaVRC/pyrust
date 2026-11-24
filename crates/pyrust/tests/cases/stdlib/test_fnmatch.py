# Comprehensive CPython 3.12 parity tests for `fnmatch`.

import fnmatch
import os
import re

print("all", fnmatch.__all__)

# --- fnmatchcase: always case-sensitive ---
cases = [
    ("abc", "abc"),
    ("abc", "ABC"),
    ("foo.py", "*.py"),
    ("FOO.PY", "*.py"),
    ("foo.py", "foo.?y"),
    ("foo.py", "foo.??"),
    ("foo.py", "foo.?"),
    ("a.txt", "[abc].txt"),
    ("d.txt", "[abc].txt"),
    ("d.txt", "[!abc].txt"),
    ("a.txt", "[!abc].txt"),
    ("b", "[a-c]"),
    ("d", "[a-c]"),
    ("b", "[!a-c]"),
    ("d", "[!a-c]"),
    ("]", "[]]"),
    ("a", "[]a]"),
    ("]", "[]a]"),
    ("b", "[]a]"),
    ("]", "[!]]"),
    ("a", "[!]]"),
    ("a[b", "a[b"),
    ("ab", "a[b"),
    ("foo\nbar", "foo*bar"),
    ("foo\nbar", "foo?bar"),
    ("a/b/c.py", "*.py"),
    (".hidden", "*"),
    ("", "*"),
    ("", "?"),
    ("", ""),
    ("a-b", "[a-]b"),
    ("a--b", "a*--*b"),
    ("abcde", "a*c*e"),
    ("abce", "a*c*e"),
    ("axxbxxcxx", "a*b*c*"),
    ("a", "[z-a]"),
    ("a", "[!z-a]"),
    ("c", "[a--c]"),
    ("a", "[a--c]"),
    ("&", "[a&&b]"),
    ("~", "[a~~b]"),
    ("|", "[a||b]"),
    ("^", "[^a]"),
    ("a", "[^a]"),
    ("[", "[[a]"),
    ("a\\b", "a\\b"),
]
for name, pat in cases:
    print("case", repr(name), repr(pat), fnmatch.fnmatchcase(name, pat))

# --- bytes patterns ---
bytes_cases = [
    (b"hello.py", b"*.py"),
    (b"HELLO.PY", b"*.py"),
    (b"abc", b"[a-c][!x]?"),
    (b"a\nb", b"a?b"),
    (b"foo\xffbar", b"foo*bar"),
]
for name, pat in bytes_cases:
    print("bytes-case", name, pat, fnmatch.fnmatchcase(name, pat))
    print("bytes-fnmatch", name, pat, fnmatch.fnmatch(name, pat))

# Mixed str/bytes raises TypeError
for a, b in [("abc", b"abc"), (b"abc", "abc")]:
    try:
        fnmatch.fnmatchcase(a, b)
        print("mixed-err", "NO-ERROR")
    except TypeError:
        print("mixed-err", "TypeError")

# --- fnmatch: platform normcase semantics ---
# On Windows (nt), case-insensitive and `/` matches `\`.
# On POSIX, case-sensitive and `/` != `\`.
print("fnmatch-same-case", fnmatch.fnmatch("Foo.py", "Foo.*"))
print("fnmatch-diff-case", fnmatch.fnmatch("FOO.PY", "*.py"))
print("fnmatch-sep", fnmatch.fnmatch("a/b/c.py", r"a\b\*.py"))

# --- filter ---
names = ["a.py", "B.PY", "c.txt", "d.py", "e.PY", ".hidden.py"]
print("filter-py", fnmatch.filter(names, "*.py"))
print("filter-gen", fnmatch.filter((x for x in names), "*.txt"))
print("filter-bytes", fnmatch.filter([b"a.py", b"B.PY", b"c.txt"], b"*.py"))

# --- translate exact parity with CPython 3.12 + compilable with re ---
translate_pats = [
    "*",
    "?",
    "*.py",
    "***",
    "a*b*c",
    "a*b*c*d",
    "[a-z]",
    "[!a-z]",
    "[]a]",
    "[!]]",
    "a[b",
    "a\\b",
    "[a--c]",
    "[a&&b]",
    "[a~~b]",
    "[a||b]",
    "[a-z-]",
    "[z-a]",
    "[!z-a]",
    "[^a]",
    "[[a]",
    "foo.bar+baz(1)",
]
for pat in translate_pats:
    rx = fnmatch.translate(pat)
    print("translate", repr(pat), repr(rx))
    compiled = re.compile(rx)
    print("compiled-match", repr(pat), compiled.match("foo.py") is not None)
