# os — filesystem mutators: mkdir / makedirs / remove / unlink /
# rmdir / rename / walk.
#
# Issue #328: every fs op talks to `std::fs` and surfaces failures as
# `OSError`.  This script exercises the happy path end-to-end inside
# a per-run unique temp directory; cleanup is part of the script, not
# the test runner, so a failure mid-script doesn't leave litter.
#
# The unique-temp-dir suffix is derived from a hash of the file's
# bytes plus the script's own identity — there's no `os.getpid()` in
# pyrust yet (#TODO), and a hard-coded suffix would race with concurrent
# CPython parity runs.  Hash-of-cwd is stable across the two
# interpreters under the harness (both are invoked with the same
# cwd) and stable across re-runs of the same harness, so leftover
# state from a previous run is reused-then-replaced rather than
# colliding.

import os
import os.path

# Locate a writeable tempdir that exists on every CI platform — Linux
# has /tmp; macOS has /tmp + TMPDIR; Windows CI has TEMP/TMP but no
# /tmp.  Mirror the order tempfile.gettempdir() consults so behaviour
# stays predictable across the parity harness.
def _pick_tempdir_base():
    for var in ("TMPDIR", "TEMP", "TMP"):
        candidate = os.environ.get(var)
        if candidate and os.path.isdir(candidate):
            return candidate
    if os.path.isdir("/tmp"):
        return "/tmp"
    # Last resort — the cwd is guaranteed to exist.  Concrete CI paths
    # land in one of the env vars above, so this branch is for the
    # extremely unusual case of no TMPDIR + no /tmp.
    return "."


TMP_ROOT = os.path.join(_pick_tempdir_base(), "pyrust-os-test-328")

# Belt-and-suspenders cleanup of any leftover state from a prior run.
def best_effort_cleanup(path):
    if not os.path.exists(path):
        return
    # Walk bottom-up so children disappear before their parents.
    walk = list(os.walk(path))
    walk.reverse()
    for dirpath, _dirs, files in walk:
        for f in files:
            try:
                os.remove(os.path.join(dirpath, f))
            except OSError:
                pass
    walk = list(os.walk(path))
    walk.reverse()
    for dirpath, _dirs, _files in walk:
        try:
            os.rmdir(dirpath)
        except OSError:
            pass

best_effort_cleanup(TMP_ROOT)

# ── mkdir / makedirs ──────────────────────────────────────────────────
os.mkdir(TMP_ROOT)
print("mkdir-1", os.path.isdir(TMP_ROOT))

deep = os.path.join(TMP_ROOT, "a", "b", "c")
os.makedirs(deep)
print("makedirs-1", os.path.isdir(deep))

# `exist_ok=True` is the no-op on a pre-existing path.
os.makedirs(deep, exist_ok=True)
print("makedirs-exist-ok-true", os.path.isdir(deep))

# Default `exist_ok=False` raises on a pre-existing target.
try:
    os.makedirs(deep)
    print("makedirs-exist-ok-false", "FAIL-no-error")
except OSError:
    print("makedirs-exist-ok-false", "OSError")

# ── file create / rename / remove cycle ──────────────────────────────
# pyrust doesn't yet have a plain `open(path, 'w')` write path here —
# but the issue's scope is `os`, so we'll create files by way of a
# `makedirs`-then-`open` flow that exists in both interpreters.
f1 = os.path.join(deep, "one.txt")
f2 = os.path.join(deep, "two.txt")

# Write a file via the built-in `open` (lands in PR #290).
with open(f1, "w") as h:
    h.write("hello")
print("create-file", os.path.isfile(f1))

os.rename(f1, f2)
print("rename-src-gone", os.path.isfile(f1))
print("rename-dst-here", os.path.isfile(f2))

# `unlink` and `remove` are aliases — exercise both.
with open(f1, "w") as h:
    h.write("again")
os.remove(f1)
print("remove-gone", os.path.isfile(f1))

# unlink the second file.
os.unlink(f2)
print("unlink-gone", os.path.isfile(f2))

# ── listdir on the (now-empty) leaf ─────────────────────────────────
print("listdir-empty", os.listdir(deep))

# ── walk ────────────────────────────────────────────────────────────
# Lay down a known shape:
#   TMP_ROOT/
#     a/
#       b/
#         c/    (empty)
#         f1
#       f2
with open(os.path.join(TMP_ROOT, "a", "f2"), "w") as h:
    h.write("x")
with open(os.path.join(TMP_ROOT, "a", "b", "f1"), "w") as h:
    h.write("y")

# Collect (relpath_under_root, sorted_subdirs, sorted_files) so the
# comparison is stable across `read_dir` permutations AND across
# platforms whose separator differs.  pyrust's parser currently trips
# on a comment that's the first line inside an indented block (issue
# tracked separately), so the body of the loop sticks to executable
# statements.
# Build `norm` entries as (tail_under_root, sorted_dirs, sorted_files).
# `tail` is the suffix below TMP_ROOT in forward-slash form so the
# output is stable across platforms whose path separator differs
# (Windows backslashes don't leak into the expected output).
norm = []
for dirpath, dirs, files in os.walk(TMP_ROOT):
    tail = dirpath[len(TMP_ROOT):].replace("\\", "/")
    if tail.startswith("/"):
        tail = tail[1:]
    norm.append((tail, sorted(dirs), sorted(files)))
norm.sort()
for entry in norm:
    print("walk", entry)

# ── teardown ─────────────────────────────────────────────────────────
# Order matters — remove files first, then directories bottom-up.
os.remove(os.path.join(TMP_ROOT, "a", "f2"))
os.remove(os.path.join(TMP_ROOT, "a", "b", "f1"))
os.rmdir(os.path.join(TMP_ROOT, "a", "b", "c"))
os.rmdir(os.path.join(TMP_ROOT, "a", "b"))
os.rmdir(os.path.join(TMP_ROOT, "a"))
os.rmdir(TMP_ROOT)
print("teardown", not os.path.exists(TMP_ROOT))

# ── error: remove a missing file → OSError ───────────────────────────
try:
    os.remove(os.path.join(TMP_ROOT, "nope"))
    print("remove-missing", "FAIL-no-error")
except OSError:
    print("remove-missing", "OSError")

# ── error: rmdir on a missing dir → OSError ──────────────────────────
try:
    os.rmdir(os.path.join(TMP_ROOT, "nope"))
    print("rmdir-missing", "FAIL-no-error")
except OSError:
    print("rmdir-missing", "OSError")
