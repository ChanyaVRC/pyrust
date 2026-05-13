# os.path — join, split, exists, basename/dirname, splitext.
#
# `import os.path` works because pyrust now ships an `os` parent package
# (see crates/pyrust/src/builtin_modules/bodies/os.rs) whose `path`
# attribute points at the `os.path` module.  Both the bare-import and
# alias forms are exercised here.

import os.path

# --- join: simple concatenation ---
print("join-1", os.path.join('a', 'b'))
print("join-2", os.path.join('a', 'b', 'c'))

# --- join: absolute-component reset (CPython quirk) ---
# When an absolute component appears in the middle, it replaces the
# running path.  Behaviour is platform-specific in CPython: on POSIX
# `/abs` is absolute; on Windows it isn't (no drive letter).  Both
# pyrust and CPython agree per-platform — the parity harness checks
# output identity, so we deliberately don't normalise here.
print("join-abs-mid", os.path.join('rel', '/abs', 'tail'))

# --- dirname / basename ---
print("dirname-1", os.path.dirname('/a/b/c.txt'))
print("dirname-2", os.path.dirname('relative.txt'))
print("basename-1", os.path.basename('/a/b/c.txt'))
print("basename-2", os.path.basename('only_a_name'))
# basename of a trailing-separator path is empty (CPython rule).
print("basename-trailing", os.path.basename('/a/b/'))

# --- splitext: leading-dot quirk ---
# `.bashrc` -> ('.bashrc', '')   — leading dots aren't extension separators.
# `foo.tar.gz` -> ('foo.tar', '.gz')  — only the last dot counts.
print("splitext-double", os.path.splitext('foo.tar.gz'))
print("splitext-dotfile", os.path.splitext('.bashrc'))
print("splitext-no-ext", os.path.splitext('no_ext'))

# --- exists ---
# Probing real paths is platform-fragile (no /tmp on Windows), and the
# parity harness needs byte-identical output across both interpreters.
# Stick to a clearly-non-existent path; that returns False everywhere.
print("exists-bogus", os.path.exists('/this/does/not/exist'))

# --- the `from` form imports submodule attributes directly ---
from os.path import join, splitext
print("from-join", join('p', 'q'))
print("from-splitext", splitext('a.txt'))

# --- the alias form ---
import os.path as op
print("alias-join", op.join('x', 'y'))
