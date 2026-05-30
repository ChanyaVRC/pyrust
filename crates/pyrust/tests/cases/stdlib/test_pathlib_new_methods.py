# pathlib.Path new methods — issue #333.
#
# Tests for methods added to the pathlib.Path implementation:
# cwd(), home(), is_absolute(), resolve(), read_bytes(), write_bytes(),
# open(), unlink(), iterdir(), glob(), with_name(), with_stem(), with_suffix().
#
# These tests avoid using `tempfile` (not yet implemented in pyrust) and
# instead use `/tmp` directly with deterministic file names.  Cleanup is
# manual via unlink / os.rmdir.
#
# Windows note: CPython uses WindowsPath on Windows.  Skip on Windows to
# avoid platform divergence in the repr / type-name checks.

import sys
import os

if sys.platform == 'win32':
    print('pathlib_new_methods ok (skipped on Windows)')
    raise SystemExit

from pathlib import Path

# ── cwd() ─────────────────────────────────────────────────────────────────────

cwd = Path.cwd()
assert type(cwd).__name__ == 'PosixPath', type(cwd).__name__
assert cwd.is_absolute(), f'cwd() result is not absolute: {cwd}'

# cwd() also works when called on an instance.
cwd2 = Path('/tmp').cwd()
assert type(cwd2).__name__ == 'PosixPath', type(cwd2).__name__
assert cwd2.is_absolute()

# ── home() ────────────────────────────────────────────────────────────────────

home = Path.home()
assert type(home).__name__ == 'PosixPath', type(home).__name__
assert home.is_absolute(), f'home() result is not absolute: {home}'

# home() also works when called on an instance.
home2 = Path('/tmp').home()
assert type(home2).__name__ == 'PosixPath', type(home2).__name__

# ── is_absolute() ─────────────────────────────────────────────────────────────

assert Path('/tmp').is_absolute()
assert Path('/').is_absolute()
assert not Path('foo').is_absolute()
assert not Path('.').is_absolute()
assert not Path('foo/bar').is_absolute()

# ── resolve() ─────────────────────────────────────────────────────────────────

r = Path('.').resolve()
assert type(r).__name__ == 'PosixPath', type(r).__name__
assert r.is_absolute(), f'resolve() is not absolute: {r}'

# Resolving an already-absolute path that exists.
r2 = Path('/tmp').resolve()
assert r2.is_absolute()
assert str(r2) != ''

# ── read_bytes() / write_bytes() ──────────────────────────────────────────────

_test_file_bytes = Path('/tmp/pyrust_pathlib_test_bytes.bin')
n = _test_file_bytes.write_bytes(b'hello world')
assert n == 11, f'write_bytes returned {n!r}'
assert _test_file_bytes.read_bytes() == b'hello world'
_test_file_bytes.unlink()

# Empty bytes.
_test_file_bytes2 = Path('/tmp/pyrust_pathlib_test_bytes2.bin')
n2 = _test_file_bytes2.write_bytes(b'')
assert n2 == 0
assert _test_file_bytes2.read_bytes() == b''
_test_file_bytes2.unlink()

# TypeError for non-bytes.
try:
    Path('/tmp/pyrust_pathlib_test_bytes_type.bin').write_bytes('not bytes')
    assert False, 'should raise TypeError'
except TypeError:
    pass

# ── open() ───────────────────────────────────────────────────────────────────

_test_file_open = Path('/tmp/pyrust_pathlib_test_open.txt')
_test_file_open.write_text('hello open')
with _test_file_open.open('r') as f:
    assert f.read() == 'hello open'
_test_file_open.unlink()

# Binary open.
_test_file_open_b = Path('/tmp/pyrust_pathlib_test_open_b.bin')
_test_file_open_b.write_bytes(b'\x01\x02\x03')
with _test_file_open_b.open('rb') as f:
    assert f.read() == b'\x01\x02\x03'
_test_file_open_b.unlink()

# ── unlink() ─────────────────────────────────────────────────────────────────

_test_file_unlink = Path('/tmp/pyrust_pathlib_test_unlink.txt')
_test_file_unlink.write_text('x')
assert _test_file_unlink.exists()
_test_file_unlink.unlink()
assert not _test_file_unlink.exists()

# Unlinking non-existent raises FileNotFoundError.
try:
    _test_file_unlink.unlink()
    assert False, 'should raise FileNotFoundError'
except FileNotFoundError:
    pass

# missing_ok=True silently passes.
_test_file_unlink.unlink(missing_ok=True)

# ── iterdir() ─────────────────────────────────────────────────────────────────

_test_dir_iter = Path('/tmp/pyrust_pathlib_test_iterdir')
_test_dir_iter.mkdir(exist_ok=True)
(_test_dir_iter / 'a.py').write_text('a')
(_test_dir_iter / 'b.py').write_text('b')
(_test_dir_iter / 'c.txt').write_text('c')

names = sorted(p.name for p in _test_dir_iter.iterdir())
assert names == ['a.py', 'b.py', 'c.txt'], names

# iterdir() yields PosixPath instances.
for p in _test_dir_iter.iterdir():
    assert type(p).__name__ == 'PosixPath', type(p).__name__
    assert p.is_absolute() or not p.parts[0].startswith('/'), str(p)
    break

for f in _test_dir_iter.iterdir():
    f.unlink()
os.rmdir(str(_test_dir_iter))

# ── glob() ───────────────────────────────────────────────────────────────────

_test_dir_glob = Path('/tmp/pyrust_pathlib_test_glob')
_test_dir_glob.mkdir(exist_ok=True)
(_test_dir_glob / 'a.py').write_text('a')
(_test_dir_glob / 'b.py').write_text('b')
(_test_dir_glob / 'c.txt').write_text('c')

py_files = sorted(p.name for p in _test_dir_glob.glob('*.py'))
assert py_files == ['a.py', 'b.py'], py_files

txt_files = sorted(p.name for p in _test_dir_glob.glob('*.txt'))
assert txt_files == ['c.txt'], txt_files

all_files = sorted(p.name for p in _test_dir_glob.glob('*'))
assert all_files == ['a.py', 'b.py', 'c.txt'], all_files

# glob() yields PosixPath instances.
for p in _test_dir_glob.glob('*.py'):
    assert type(p).__name__ == 'PosixPath', type(p).__name__
    break

for f in _test_dir_glob.iterdir():
    f.unlink()
os.rmdir(str(_test_dir_glob))

# ── with_name() ──────────────────────────────────────────────────────────────

p = Path('/tmp/foo.txt')
assert repr(p.with_name('bar.py')) == "PosixPath('/tmp/bar.py')", repr(p.with_name('bar.py'))
assert repr(p.with_name('bar')) == "PosixPath('/tmp/bar')", repr(p.with_name('bar'))

# Relative path.
rel = Path('foo/bar.txt')
assert repr(rel.with_name('baz.rs')) == "PosixPath('foo/baz.rs')", repr(rel.with_name('baz.rs'))

# Empty name raises ValueError.
try:
    p.with_name('')
    assert False, 'should raise ValueError'
except ValueError:
    pass

# Root path raises ValueError.
try:
    Path('/').with_name('foo')
    assert False, 'should raise ValueError'
except ValueError:
    pass

# ── with_stem() ───────────────────────────────────────────────────────────────

p = Path('/tmp/foo.txt')
assert repr(p.with_stem('bar')) == "PosixPath('/tmp/bar.txt')", repr(p.with_stem('bar'))

# Compound extension — only the final suffix is kept.
p_compound = Path('/tmp/foo.tar.gz')
assert repr(p_compound.with_stem('bar')) == "PosixPath('/tmp/bar.gz')", repr(p_compound.with_stem('bar'))

# ── with_suffix() ─────────────────────────────────────────────────────────────

p = Path('/tmp/foo.txt')
assert repr(p.with_suffix('.py')) == "PosixPath('/tmp/foo.py')", repr(p.with_suffix('.py'))
assert repr(p.with_suffix('')) == "PosixPath('/tmp/foo')", repr(p.with_suffix(''))

# Multi-dot suffix is valid.
assert repr(p.with_suffix('.tar.gz')) == "PosixPath('/tmp/foo.tar.gz')", repr(p.with_suffix('.tar.gz'))

# No leading dot raises ValueError.
try:
    p.with_suffix('txt')
    assert False, 'should raise ValueError'
except ValueError:
    pass

# Bare '.' raises ValueError.
try:
    p.with_suffix('.')
    assert False, 'should raise ValueError'
except ValueError:
    pass

print('pathlib_new_methods ok')
