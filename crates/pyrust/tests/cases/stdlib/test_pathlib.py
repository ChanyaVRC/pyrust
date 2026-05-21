# pathlib module — Path class smoke-tests.
#
# pyrust implements name/parent/stem/suffix/parts as callable methods rather
# than descriptors (properties).  CPython exposes them as read-only properties.
# To keep this fixture version-stable the helper `_get` calls the attribute
# if it is callable (pyrust bound-method) and accesses it directly otherwise
# (CPython property value).
#
# POSIX-path tests (absolute paths with forward-slash separators and root '/')
# are skipped on Windows because CPython uses WindowsPath there, which
# normalises separators to '\' and produces different string representations.
# pyrust's pathlib is POSIX-only so these tests are Linux/macOS only.

import sys

POSIX = sys.platform != 'win32'

from pathlib import Path


def _get(obj, attr):
    """Get a Path attribute, calling it if pyrust exposes it as a method."""
    v = getattr(obj, attr)
    return v() if callable(v) else v


# ── filesystem predicates (cross-platform) ────────────────────────────────────
# '.' always exists as a directory on every platform.

assert Path('.').exists()
assert Path('.').is_dir()
assert not Path('.').is_file()

# A path that does not exist returns False for exists().
assert not Path('xyzzy_nonexistent_pyrust_test_9z7q').exists()

# ── relative path — component accessors ───────────────────────────────────────
# name/stem/suffix don't depend on platform path separators.

_rel_name = _get(Path('a/b/c.py'), 'name')
assert _rel_name == 'c.py', repr(_rel_name)
_rel_stem = _get(Path('a/b/c.py'), 'stem')
assert _rel_stem == 'c', repr(_rel_stem)
_rel_suffix = _get(Path('a/b/c.py'), 'suffix')
assert _rel_suffix == '.py', repr(_rel_suffix)

# ── no-suffix and hidden files ────────────────────────────────────────────────

assert _get(Path('Makefile'), 'suffix') == ''
assert _get(Path('Makefile'), 'stem') == 'Makefile'
assert _get(Path('.bashrc'), 'suffix') == ''
assert _get(Path('.bashrc'), 'stem') == '.bashrc'

# ── write_text returns character count (CPython 3.10+) ───────────────────────
# Use a relative path so this works on every platform.  Clean up afterwards
# so the harness doesn't leave test artifacts in the repo/worktree.

_wt_path = Path('pyrust_test_pathlib_wt_tmp.txt')
try:
    _wt_result = _wt_path.write_text('hello')
    assert _wt_result == 5, repr(_wt_result)
finally:
    import os as _os
    try:
        _os.remove('pyrust_test_pathlib_wt_tmp.txt')
    except OSError:
        pass

# ── POSIX-only tests (absolute paths with '/' root) ───────────────────────────

if POSIX:
    # ── constructor and __str__ ───────────────────────────────────────────────

    p = Path('/tmp/foo/bar.txt')
    assert str(p) == '/tmp/foo/bar.txt', repr(str(p))

    # ── component accessors ───────────────────────────────────────────────────

    assert _get(p, 'name') == 'bar.txt', repr(_get(p, 'name'))
    assert str(_get(p, 'parent')) == '/tmp/foo', repr(str(_get(p, 'parent')))
    assert _get(p, 'stem') == 'bar', repr(_get(p, 'stem'))
    assert _get(p, 'suffix') == '.txt', repr(_get(p, 'suffix'))
    assert _get(p, 'parts') == ('/', 'tmp', 'foo', 'bar.txt'), repr(_get(p, 'parts'))

    # ── / operator for joining ────────────────────────────────────────────────

    q = Path('/tmp') / 'foo' / 'bar'
    assert str(q) == '/tmp/foo/bar', repr(str(q))

    # Absolute rhs replaces lhs (CPython semantics)
    r = Path('/tmp/foo') / '/other'
    assert str(r) == '/other', repr(str(r))

    # ── POSIX filesystem predicates ───────────────────────────────────────────

    assert Path('/tmp').exists()
    assert Path('/tmp').is_dir()
    assert not Path('/tmp').is_file()

    # ── relative path — parent and parts (separator-dependent) ───────────────

    rel = Path('a/b/c.py')
    assert str(_get(rel, 'parent')) == 'a/b', repr(str(_get(rel, 'parent')))
    assert _get(rel, 'parts') == ('a', 'b', 'c.py'), repr(_get(rel, 'parts'))

    # ── no-suffix and hidden files with directory context ────────────────────

    nosuf = Path('/tmp/Makefile')
    assert _get(nosuf, 'suffix') == '', repr(_get(nosuf, 'suffix'))
    assert _get(nosuf, 'stem') == 'Makefile', repr(_get(nosuf, 'stem'))

    hidden = Path('/home/user/.bashrc')
    assert _get(hidden, 'suffix') == '', repr(_get(hidden, 'suffix'))
    assert _get(hidden, 'stem') == '.bashrc', repr(_get(hidden, 'stem'))

    # ── equality ──────────────────────────────────────────────────────────────

    assert Path('/tmp') == Path('/tmp')
    assert Path('/tmp') != Path('/var')

    # ── trailing-slash normalisation ──────────────────────────────────────────
    # CPython normalises trailing slashes in the constructor.

    trailing = Path('/tmp/foo/')
    assert str(trailing) == '/tmp/foo', repr(str(trailing))
    assert _get(trailing, 'name') == 'foo', repr(_get(trailing, 'name'))
    assert str(_get(trailing, 'parent')) == '/tmp', repr(str(_get(trailing, 'parent')))

    # ── dot component normalisation ───────────────────────────────────────────

    assert str(Path('/tmp') / '.') == '/tmp', repr(str(Path('/tmp') / '.'))
    assert str(Path('/tmp/./foo')) == '/tmp/foo', repr(str(Path('/tmp/./foo')))

    # ── empty rhs in / operator ───────────────────────────────────────────────

    assert str(Path('/tmp') / '') == '/tmp', repr(str(Path('/tmp') / ''))

print('pathlib ok')
