# pathlib.PosixPath subclass parity — issue #922.
#
# On Linux/macOS, `pathlib.Path(...)` returns a `PosixPath` instance, not a
# bare `Path` instance.  `type(Path('/tmp')).__name__` must be `'PosixPath'`.
#
# `name`/`parent`/`stem`/`suffix`/`parts` are read-only properties in CPython.
# pyrust now exposes them as property descriptors so direct access works.
#
# Windows note: CPython uses `WindowsPath` on Windows.  These tests are
# POSIX-only (Linux/macOS) and are skipped on Windows to avoid platform
# divergence.

import sys

if sys.platform == 'win32':
    print('pathlib_posixpath ok (skipped on Windows)')
    raise SystemExit

from pathlib import Path, PosixPath


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

# ── properties: direct access without () ──────────────────────────────────────

# name, stem, suffix, parent, parts are read-only properties in CPython.
# Access them without calling; calling them would raise TypeError.
assert Path('/tmp/foo/bar.txt').name == 'bar.txt'
assert Path('/tmp/foo/bar.txt').stem == 'bar'
assert Path('/tmp/foo/bar.txt').suffix == '.txt'
assert str(Path('/tmp/foo/bar.txt').parent) == '/tmp/foo'
assert Path('/tmp/foo/bar.txt').parts == ('/', 'tmp', 'foo', 'bar.txt')

assert PosixPath('/tmp/foo/bar.txt').name == 'bar.txt'
assert PosixPath('/tmp/foo/bar.txt').stem == 'bar'
assert PosixPath('/tmp/foo/bar.txt').suffix == '.txt'

# ── / operator produces PosixPath ─────────────────────────────────────────────

joined = Path('/tmp') / 'foo'
assert type(joined).__name__ == 'PosixPath', repr(type(joined).__name__)
assert str(joined) == '/tmp/foo', repr(str(joined))

# ── parent produces PosixPath ─────────────────────────────────────────────────

parent = Path('/tmp/foo/bar').parent
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
assert Path('.').name == '', repr(Path('.').name)
assert Path('.').stem == '', repr(Path('.').stem)
assert Path('.').suffix == '', repr(Path('.').suffix)

# CPython: Path('/').name == ''.
assert Path('/').name == '', repr(Path('/').name)
assert Path('/').stem == '', repr(Path('/').stem)
assert Path('/').suffix == '', repr(Path('/').suffix)

# CPython: Path('..').name == '..', stem == '..', suffix == ''.
assert Path('..').name == '..', repr(Path('..').name)
assert Path('..').stem == '..', repr(Path('..').stem)
assert Path('..').suffix == '', repr(Path('..').suffix)

# Hidden files: leading dot is NOT a suffix separator.
assert Path('/tmp/.hidden').name == '.hidden', repr(Path('/tmp/.hidden').name)
assert Path('/tmp/.hidden').stem == '.hidden', repr(Path('/tmp/.hidden').stem)
assert Path('/tmp/.hidden').suffix == '', repr(Path('/tmp/.hidden').suffix)

# Hidden files with extension.
assert Path('/tmp/.hidden.txt').stem == '.hidden', repr(Path('/tmp/.hidden.txt').stem)
assert Path('/tmp/.hidden.txt').suffix == '.txt', repr(Path('/tmp/.hidden.txt').suffix)

print('pathlib_posixpath ok')
