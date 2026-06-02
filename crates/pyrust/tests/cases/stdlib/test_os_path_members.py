# os.path functions added for issue #2021:
# split, isabs, normpath, splitdrive, relpath, commonprefix (all pure
# string logic), plus expanduser/realpath (env-specific -> type-only).
#
# pyrust implements posixpath semantics for os.path on ALL platforms (see
# follow-up issue: os.path should use ntpath on Windows). CPython, by
# contrast, switches os.path to ntpath on Windows, where split/normpath/
# splitdrive/isabs/relpath all produce backslash/drive output. To keep
# this fixture byte-identical between pyrust and CPython on BOTH platforms,
# the path-logic assertions are guarded under `if os.name == 'posix'`:
# under that guard CPython uses posixpath, matching pyrust; on Windows the
# whole block is skipped by both interpreters so their output stays equal.
#
# commonprefix is a pure character-wise prefix in both posixpath and
# ntpath, so it is asserted unconditionally. The isinstance type checks
# are platform-stable and likewise unconditional.

import os
import os.path as p

if os.name == "posix":
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

# --- commonprefix (character-wise, identical in posixpath and ntpath) ---
print("commonprefix-1", p.commonprefix(["/a/b", "/a/c"]))
print("commonprefix-2", p.commonprefix(["abc", "abd"]))
print("commonprefix-empty", p.commonprefix([]))
print("commonprefix-3", p.commonprefix(["/usr/lib", "/usr/local"]))

# --- env-specific: type-only ---
print("expanduser-str", isinstance(p.expanduser("~/x"), str))
print("realpath-str", isinstance(p.realpath("."), str))
