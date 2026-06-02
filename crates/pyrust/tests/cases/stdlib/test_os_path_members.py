# os.path functions added for issue #2021:
# split, isabs, normpath, splitdrive, relpath, commonprefix (all pure
# string logic, DETERMINISTIC -> parity-compared against CPython 3.12
# posixpath), plus expanduser/realpath (env-specific -> type-only).

import os.path as p

# --- split ---
print("split-1", p.split("/a/b/c"))
print("split-root", p.split("/"))
print("split-dslash", p.split("//a"))
print("split-trailing", p.split("a/"))
print("split-empty", p.split(""))
print("split-trailing2", p.split("a/b/"))
print("split-nodir", p.split("c"))

# --- isabs ---
print("isabs-abs", p.isabs("/x"))
print("isabs-rel", p.isabs("x"))
print("isabs-empty", p.isabs(""))
print("isabs-dslash", p.isabs("//x"))

# --- normpath ---
print("normpath-1", p.normpath("a/./b/../c"))
print("normpath-empty", p.normpath(""))
print("normpath-root", p.normpath("/"))
print("normpath-dslash", p.normpath("//"))
print("normpath-tslash", p.normpath("///"))
print("normpath-up", p.normpath("a/.."))
print("normpath-rootup", p.normpath("/foo/../.."))
print("normpath-dotdot", p.normpath(".."))
print("normpath-dupsep", p.normpath("a//b"))
print("normpath-relup", p.normpath("../a"))
print("normpath-trailing", p.normpath("/a/b/"))

# --- splitdrive (always ('', path) on POSIX) ---
print("splitdrive-1", p.splitdrive("/a/b"))
print("splitdrive-2", p.splitdrive("c:/x"))

# --- relpath (absolute pairs are deterministic; no cwd needed) ---
print("relpath-1", p.relpath("/a/b/c", "/a"))
print("relpath-2", p.relpath("/a/b/c", "/a/b"))
print("relpath-same", p.relpath("/a", "/a"))
print("relpath-diverge", p.relpath("/a/b", "/c/d"))

# --- commonprefix (character-wise, not path-aware) ---
print("commonprefix-1", p.commonprefix(["/a/b", "/a/c"]))
print("commonprefix-2", p.commonprefix(["abc", "abd"]))
print("commonprefix-empty", p.commonprefix([]))
print("commonprefix-3", p.commonprefix(["/usr/lib", "/usr/local"]))

# --- env-specific: type-only ---
print("expanduser-str", isinstance(p.expanduser("~/x"), str))
print("realpath-str", isinstance(p.realpath("."), str))
