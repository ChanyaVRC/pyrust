# pathlib.PosixPath subclass parity — issue #922.
#
# On Linux/macOS, `pathlib.Path(...)` returns a `PosixPath` instance, not a
# bare `Path` instance.  `type(Path('/tmp')).__name__` must be `'PosixPath'`.
#
# pyrust implements `name`/`parent`/`stem`/`suffix`/`parts` as callable
# methods rather than descriptors (properties).  CPython exposes them as
# read-only properties.  The `_get` helper bridges the gap without branching
# on the interpreter.
#
# Windows note: CPython uses `WindowsPath` on Windows.  These tests are
# POSIX-only (Linux/macOS) and are skipped on Windows to avoid platform
# divergence.

import sys

if sys.platform == 'win32':
    print('pathlib_posixpath ok (skipped on Windows)')
    raise SystemExit

from pathlib import Path, PosixPath


def _get(obj, attr):
    """Get a Path attribute, calling it if pyrust exposes it as a method."""
    v = getattr(obj, attr)
    return v() if callable(v) else v


# ── type identity ──────────────────────────────────────────────────────────────

# Path(str) returns a PosixPath instance on POSIX.
p = Path('/tmp')
assert type(p).__name__ == 'PosixPath', repr(type(p).__name__)

# isinstance checks: PosixPath is a subclass of Path.
assert isinstance(p, PosixPath), 'Path() should be a PosixPath instance'
assert isinstance(p, Path), 'Path() should also be a Path instance'

# Constructing PosixPath directly also gives a PosixPath.
pp = PosixPath('/tmp')
assert type(pp).__name__ == 'PosixPath', repr(type(pp).__name__)
assert isinstance(pp, PosixPath)
assert isinstance(pp, Path)

# ── from pathlib import PosixPath works ───────────────────────────────────────

# Already imported above — just confirm the import doesn't raise.
assert PosixPath is not None

# ── subclass relationship ──────────────────────────────────────────────────────

assert issubclass(PosixPath, Path), 'PosixPath should be a subclass of Path'

# ── repr uses class name ───────────────────────────────────────────────────────

assert repr(Path('/tmp/foo')) == "PosixPath('/tmp/foo')", repr(repr(Path('/tmp/foo')))
assert repr(PosixPath('/tmp/foo')) == "PosixPath('/tmp/foo')", repr(repr(PosixPath('/tmp/foo')))

# ── str still returns the path string ─────────────────────────────────────────

assert str(Path('/tmp/foo')) == '/tmp/foo', repr(str(Path('/tmp/foo')))
assert str(PosixPath('/tmp/foo')) == '/tmp/foo', repr(str(PosixPath('/tmp/foo')))

# ── methods inherited from Path work on PosixPath instances ───────────────────

assert _get(Path('/tmp/foo/bar.txt'), 'name') == 'bar.txt'
assert _get(Path('/tmp/foo/bar.txt'), 'stem') == 'bar'
assert _get(Path('/tmp/foo/bar.txt'), 'suffix') == '.txt'
assert str(_get(Path('/tmp/foo/bar.txt'), 'parent')) == '/tmp/foo'

assert _get(PosixPath('/tmp/foo/bar.txt'), 'name') == 'bar.txt'
assert _get(PosixPath('/tmp/foo/bar.txt'), 'stem') == 'bar'
assert _get(PosixPath('/tmp/foo/bar.txt'), 'suffix') == '.txt'

# ── / operator produces PosixPath ─────────────────────────────────────────────

joined = Path('/tmp') / 'foo'
assert type(joined).__name__ == 'PosixPath', repr(type(joined).__name__)
assert str(joined) == '/tmp/foo', repr(str(joined))

# ── parent() produces PosixPath ───────────────────────────────────────────────

parent = _get(Path('/tmp/foo/bar'), 'parent')
assert type(parent).__name__ == 'PosixPath', repr(type(parent).__name__)
assert str(parent) == '/tmp/foo', repr(str(parent))

# ── equality across Path and PosixPath instances ──────────────────────────────

assert Path('/tmp') == Path('/tmp')
assert Path('/tmp') == PosixPath('/tmp')
assert PosixPath('/tmp') == Path('/tmp')
assert not (Path('/tmp') == Path('/var'))

# ── filesystem predicates ─────────────────────────────────────────────────────

assert Path('/tmp').exists()
assert Path('/tmp').is_dir()
assert not Path('/tmp').is_file()

# ── joinpath ──────────────────────────────────────────────────────────────────

# Single argument joinpath.
jp = Path('/tmp').joinpath('foo')
assert str(jp) == '/tmp/foo', repr(str(jp))
assert type(jp).__name__ == 'PosixPath', repr(type(jp).__name__)

# Multiple arguments joinpath.
jp2 = Path('/tmp').joinpath('foo', 'bar.txt')
assert str(jp2) == '/tmp/foo/bar.txt', repr(str(jp2))

# Absolute component in joinpath resets the path (CPython semantics).
jp3 = Path('/tmp').joinpath('/var', 'log')
assert str(jp3) == '/var/log', repr(str(jp3))

# Joinpath with a Path instance argument.
jp4 = Path('/tmp').joinpath(PosixPath('foo'))
assert str(jp4) == '/tmp/foo', repr(str(jp4))

# ── name/stem/suffix edge cases ───────────────────────────────────────────────

# CPython: Path('.').name == '' (dot is a pure-anchor, not a filename).
assert _get(Path('.'), 'name') == '', repr(_get(Path('.'), 'name'))
assert _get(Path('.'), 'stem') == '', repr(_get(Path('.'), 'stem'))
assert _get(Path('.'), 'suffix') == '', repr(_get(Path('.'), 'suffix'))

# CPython: Path('/').name == ''.
assert _get(Path('/'), 'name') == '', repr(_get(Path('/'), 'name'))
assert _get(Path('/'), 'stem') == '', repr(_get(Path('/'), 'stem'))
assert _get(Path('/'), 'suffix') == '', repr(_get(Path('/'), 'suffix'))

# CPython: Path('..').name == '..', stem == '..', suffix == ''.
assert _get(Path('..'), 'name') == '..', repr(_get(Path('..'), 'name'))
assert _get(Path('..'), 'stem') == '..', repr(_get(Path('..'), 'stem'))
assert _get(Path('..'), 'suffix') == '', repr(_get(Path('..'), 'suffix'))

# Hidden files: leading dot is NOT a suffix separator.
assert _get(Path('/tmp/.hidden'), 'name') == '.hidden', repr(_get(Path('/tmp/.hidden'), 'name'))
assert _get(Path('/tmp/.hidden'), 'stem') == '.hidden', repr(_get(Path('/tmp/.hidden'), 'stem'))
assert _get(Path('/tmp/.hidden'), 'suffix') == '', repr(_get(Path('/tmp/.hidden'), 'suffix'))

# Hidden files with extension.
assert _get(Path('/tmp/.hidden.txt'), 'stem') == '.hidden', repr(_get(Path('/tmp/.hidden.txt'), 'stem'))
assert _get(Path('/tmp/.hidden.txt'), 'suffix') == '.txt', repr(_get(Path('/tmp/.hidden.txt'), 'suffix'))

print('pathlib_posixpath ok')
