# pathlib module — Path class smoke-tests.
#
# pyrust implements name/parent/stem/suffix/parts as callable methods rather
# than descriptors (properties).  CPython exposes them as read-only properties.
# To keep this fixture version-stable the helper `_get` calls the attribute
# if it is callable (pyrust bound-method) and accesses it directly otherwise
# (CPython property value).

from pathlib import Path


def _get(obj, attr):
    """Get a Path attribute, calling it if pyrust exposes it as a method."""
    v = getattr(obj, attr)
    return v() if callable(v) else v


# ── constructor and __str__ ───────────────────────────────────────────────────

p = Path('/tmp/foo/bar.txt')
assert str(p) == '/tmp/foo/bar.txt', repr(str(p))

# ── component accessors ───────────────────────────────────────────────────────

assert _get(p, 'name') == 'bar.txt', repr(_get(p, 'name'))
assert str(_get(p, 'parent')) == '/tmp/foo', repr(str(_get(p, 'parent')))
assert _get(p, 'stem') == 'bar', repr(_get(p, 'stem'))
assert _get(p, 'suffix') == '.txt', repr(_get(p, 'suffix'))
assert _get(p, 'parts') == ('/', 'tmp', 'foo', 'bar.txt'), repr(_get(p, 'parts'))

# ── / operator for joining ────────────────────────────────────────────────────

q = Path('/tmp') / 'foo' / 'bar'
assert str(q) == '/tmp/foo/bar', repr(str(q))

# Absolute rhs replaces lhs (CPython semantics)
r = Path('/tmp/foo') / '/other'
assert str(r) == '/other', repr(str(r))

# ── filesystem predicates (paths that exist on Linux) ─────────────────────────

assert Path('.').is_dir()
assert Path('/tmp').exists()
assert Path('/tmp').is_dir()
assert not Path('/tmp').is_file()

# ── relative path ─────────────────────────────────────────────────────────────

rel = Path('a/b/c.py')
assert _get(rel, 'name') == 'c.py', repr(_get(rel, 'name'))
assert _get(rel, 'stem') == 'c', repr(_get(rel, 'stem'))
assert _get(rel, 'suffix') == '.py', repr(_get(rel, 'suffix'))
assert str(_get(rel, 'parent')) == 'a/b', repr(str(_get(rel, 'parent')))
assert _get(rel, 'parts') == ('a', 'b', 'c.py'), repr(_get(rel, 'parts'))

# ── no-suffix file ────────────────────────────────────────────────────────────

nosuf = Path('/tmp/Makefile')
assert _get(nosuf, 'suffix') == '', repr(_get(nosuf, 'suffix'))
assert _get(nosuf, 'stem') == 'Makefile', repr(_get(nosuf, 'stem'))

# ── hidden file (leading dot — suffix should be empty, stem = full name) ──────

hidden = Path('/home/user/.bashrc')
assert _get(hidden, 'suffix') == '', repr(_get(hidden, 'suffix'))
assert _get(hidden, 'stem') == '.bashrc', repr(_get(hidden, 'stem'))

# ── equality ─────────────────────────────────────────────────────────────────

assert Path('/tmp') == Path('/tmp')
assert Path('/tmp') != Path('/var')

print('pathlib ok')
