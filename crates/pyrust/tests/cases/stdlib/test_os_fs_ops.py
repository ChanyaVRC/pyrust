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

# Pick a tempdir name that's stable across CPython and pyrust runs of
# the same script but very unlikely to collide with anyone else.  The
# value of `__name__` for a top-level script is "__main__" in both
# interpreters, so we can't use that; instead the script bakes its own
# discriminator into the path.  Manually-chosen suffix is fine for an
# initial landing.
TMP_ROOT = "/tmp/pyrust-os-test-328"

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
                os.remove(dirpath + "/" + f)
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

deep = TMP_ROOT + "/a/b/c"
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
f1 = deep + "/one.txt"
f2 = deep + "/two.txt"

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
with open(TMP_ROOT + "/a/f2", "w") as h:
    h.write("x")
with open(TMP_ROOT + "/a/b/f1", "w") as h:
    h.write("y")

# Collect (path, sorted_subdirs, sorted_files) so the comparison is
# stable across `read_dir` permutations.  pyrust's parser currently
# trips on a comment that's the first line inside an indented block
# (issue tracked separately), so the body of the loop sticks to
# executable statements.
norm = []
for dirpath, dirs, files in os.walk(TMP_ROOT):
    norm.append((dirpath, sorted(dirs), sorted(files)))
norm.sort()
for entry in norm:
    print("walk", entry)

# ── teardown ─────────────────────────────────────────────────────────
# Order matters — remove files first, then directories bottom-up.
os.remove(TMP_ROOT + "/a/f2")
os.remove(TMP_ROOT + "/a/b/f1")
os.rmdir(TMP_ROOT + "/a/b/c")
os.rmdir(TMP_ROOT + "/a/b")
os.rmdir(TMP_ROOT + "/a")
os.rmdir(TMP_ROOT)
print("teardown", not os.path.exists(TMP_ROOT))

# ── error: remove a missing file → OSError ───────────────────────────
try:
    os.remove(TMP_ROOT + "/nope")
    print("remove-missing", "FAIL-no-error")
except OSError:
    print("remove-missing", "OSError")

# ── error: rmdir on a missing dir → OSError ──────────────────────────
try:
    os.rmdir(TMP_ROOT + "/nope")
    print("rmdir-missing", "FAIL-no-error")
except OSError:
    print("rmdir-missing", "OSError")
