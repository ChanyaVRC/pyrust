# os — module-level functions and `os.environ` dict-like view.
#
# Issue #328: this is the parity script for the read-only-ish parts of
# the `os` surface that landed alongside the filesystem mutators.  The
# fs-mutating tests live in test_os_fs_ops.py so they can isolate
# tempdirs from this script's environ writes.
#
# Tests avoid:
#   * `os.getcwd()` raw output — the cwd diverges between CPython and
#     pyrust under the parity harness (CPython runs from the workspace
#     root, pyrust from wherever cargo points).  We assert structural
#     facts about it instead.
#   * Reading or printing process env keys we didn't write — they would
#     differ across CI / dev machines.

import os
import os.path

# ── getcwd: structural assertions only ────────────────────────────────
cwd = os.getcwd()
print("getcwd-is-str", isinstance(cwd, str))
print("getcwd-nonempty", len(cwd) > 0)
# `pyrust` appears in the workspace's full path on both interpreters
# (CPython is invoked with cwd=workspace-root, pyrust runs the binary
# from the same workspace).  Case-insensitive to keep it robust.
print("getcwd-has-pyrust", "pyrust" in cwd.lower())

# ── getenv ────────────────────────────────────────────────────────────
# Default is None when key is absent.
print("getenv-missing-no-default", os.getenv("PYRUST_DEFINITELY_NOT_SET_328"))
print(
    "getenv-missing-with-default",
    os.getenv("PYRUST_DEFINITELY_NOT_SET_328", "fallback"),
)

# ── environ: set / get / contains / del ──────────────────────────────
KEY = "PYRUST_TEST_OS_FUNCS_KEY_328"
# Sanity: start clean (a leftover from a prior failed run would skew
# the contains-False assertion below).
if KEY in os.environ:
    del os.environ[KEY]

print("env-initial-contains", KEY in os.environ)
print("env-initial-get-default", os.environ.get(KEY, "absent"))

os.environ[KEY] = "first"
print("env-after-set-contains", KEY in os.environ)
print("env-after-set-getitem", os.environ[KEY])
print("env-after-set-get", os.environ.get(KEY))
print("env-after-set-get-fallback-unused", os.environ.get(KEY, "unused"))

# getenv reads the live env too — sanity-check.
print("env-after-set-getenv", os.getenv(KEY))

# Overwrite.
os.environ[KEY] = "second"
print("env-after-overwrite", os.environ[KEY])

# del removes it.
del os.environ[KEY]
print("env-after-del-contains", KEY in os.environ)
print("env-after-del-get-default", os.environ.get(KEY, "absent-again"))

# KeyError on missing __getitem__.
try:
    _ = os.environ[KEY]
    print("env-missing-getitem", "FAIL-no-error")
except KeyError:
    print("env-missing-getitem", "KeyError")

# KeyError on missing __delitem__.
try:
    del os.environ[KEY]
    print("env-missing-delitem", "FAIL-no-error")
except KeyError:
    print("env-missing-delitem", "KeyError")

# ── listdir on a known directory ─────────────────────────────────────
# Use the system tmp dir — exists on both Linux and macOS, and on
# Windows when the harness is run under WSL.  We assert structural
# facts only (entries is a list of strings, length >= 0) so we don't
# depend on what happens to be in /tmp at test time.
tmp_entries = os.listdir("/tmp")
print("listdir-is-list", isinstance(tmp_entries, list))
print("listdir-nonneg", len(tmp_entries) >= 0)
print(
    "listdir-all-strings",
    all([isinstance(e, str) for e in tmp_entries]),
)
