# pathlib.Path property access — issue #898.
#
# `name`, `parent`, `stem`, `suffix`, and `parts` are read-only properties in
# CPython's pathlib.  Accessing them without `()` must return the value directly
# (a str / Path / tuple), not a bound method or callable.
#
# Windows note: CPython uses `WindowsPath` on Windows.  Skip on Windows to
# avoid platform divergence in the repr / type-name checks.

import sys

if sys.platform == 'win32':
    print('pathlib_properties ok (skipped on Windows)')
    raise SystemExit

from pathlib import Path

# ── basic property access ──────────────────────────────────────────────────────

p = Path('/tmp/foo/bar.txt')

# .name returns a str, not a callable.
assert p.name == 'bar.txt', repr(p.name)
assert isinstance(p.name, str), type(p.name)

# .stem returns a str.
assert p.stem == 'bar', repr(p.stem)
assert isinstance(p.stem, str), type(p.stem)

# .suffix returns a str including the leading dot.
assert p.suffix == '.txt', repr(p.suffix)
assert isinstance(p.suffix, str), type(p.suffix)

# .parent returns a Path (PosixPath on POSIX).
parent = p.parent
assert str(parent) == '/tmp/foo', repr(str(parent))
assert type(parent).__name__ in ('Path', 'PosixPath'), type(parent).__name__

# .parts returns a tuple of strings.
assert p.parts == ('/', 'tmp', 'foo', 'bar.txt'), repr(p.parts)
assert isinstance(p.parts, tuple), type(p.parts)

# ── property, not callable ─────────────────────────────────────────────────────

# Confirm the result is NOT callable (it's a str, not a bound method).
assert not callable(p.name), 'name should not be callable'
assert not callable(p.stem), 'stem should not be callable'
assert not callable(p.suffix), 'suffix should not be callable'
assert not callable(p.parts), 'parts should not be callable'

# parent returns a Path object, which is not callable in this context.
assert not callable(p.parent), 'parent should not be callable'

# Calling a property value raises TypeError (str/tuple/Path is not callable).
try:
    p.name()
    assert False, 'p.name() should raise TypeError'
except TypeError:
    pass

try:
    p.stem()
    assert False, 'p.stem() should raise TypeError'
except TypeError:
    pass

try:
    p.suffix()
    assert False, 'p.suffix() should raise TypeError'
except TypeError:
    pass

try:
    p.parts()
    assert False, 'p.parts() should raise TypeError'
except TypeError:
    pass

# ── chained property access ────────────────────────────────────────────────────

# Chaining: path.parent.name should work without any ()
assert Path('/tmp/foo/bar.txt').parent.name == 'foo', repr(Path('/tmp/foo/bar.txt').parent.name)

# ── relative paths ─────────────────────────────────────────────────────────────

rel = Path('foo/bar.baz')
assert rel.name == 'bar.baz', repr(rel.name)
assert rel.stem == 'bar', repr(rel.stem)
assert rel.suffix == '.baz', repr(rel.suffix)
assert str(rel.parent) == 'foo', repr(str(rel.parent))
assert rel.parts == ('foo', 'bar.baz'), repr(rel.parts)

# ── edge cases ────────────────────────────────────────────────────────────────

# Root path.
root = Path('/')
assert root.name == '', repr(root.name)
assert root.stem == '', repr(root.stem)
assert root.suffix == '', repr(root.suffix)
assert root.parts == ('/',), repr(root.parts)

# Current dir.
dot = Path('.')
assert dot.name == '', repr(dot.name)
assert dot.stem == '', repr(dot.stem)
assert dot.suffix == '', repr(dot.suffix)

# Parent dir token.
dotdot = Path('..')
assert dotdot.name == '..', repr(dotdot.name)
assert dotdot.stem == '..', repr(dotdot.stem)
assert dotdot.suffix == '', repr(dotdot.suffix)

# Hidden file (leading dot is NOT a suffix separator).
hidden = Path('/home/user/.bashrc')
assert hidden.name == '.bashrc', repr(hidden.name)
assert hidden.stem == '.bashrc', repr(hidden.stem)
assert hidden.suffix == '', repr(hidden.suffix)

# Hidden file with extension.
hidden_ext = Path('/home/user/.bashrc.bak')
assert hidden_ext.name == '.bashrc.bak', repr(hidden_ext.name)
assert hidden_ext.stem == '.bashrc', repr(hidden_ext.stem)
assert hidden_ext.suffix == '.bak', repr(hidden_ext.suffix)

# ── parts for absolute and relative paths ─────────────────────────────────────

assert Path('/a/b/c').parts == ('/', 'a', 'b', 'c'), repr(Path('/a/b/c').parts)
assert Path('a/b/c').parts == ('a', 'b', 'c'), repr(Path('a/b/c').parts)
assert Path('/').parts == ('/',), repr(Path('/').parts)

print('pathlib_properties ok')
