# pathlib.Path fixes — issues #1829 and #1830.
#
# #1829: Path.glob("[!...]") negation support.
# #1830: Path.resolve(strict=True) raises FileNotFoundError.
#
# Uses /tmp with deterministic names to avoid tempfile dependency.

import sys
import os

if sys.platform == 'win32':
    print('pathlib_fixes ok (skipped on Windows)')
    raise SystemExit

from pathlib import Path

# ── Setup: scratch directory ──────────────────────────────────────────────────

_test_dir = Path('/tmp/pyrust_pathlib_fixes_test')
_test_dir.mkdir(exist_ok=True)
(_test_dir / 'apple.txt').write_text('a')
(_test_dir / 'banana.txt').write_text('b')
(_test_dir / 'cherry.py').write_text('c')
(_test_dir / 'date.py').write_text('d')

# ── #1829: glob() negation [!...] ────────────────────────────────────────────

# [!a]* — files not starting with 'a'
names_not_a = sorted(p.name for p in _test_dir.glob('[!a]*'))
assert 'apple.txt' not in names_not_a, f'apple.txt should be excluded: {names_not_a}'
assert 'banana.txt' in names_not_a, names_not_a
assert 'cherry.py' in names_not_a, names_not_a
assert 'date.py' in names_not_a, names_not_a

# [!ab]* — files not starting with 'a' or 'b'
names_not_ab = sorted(p.name for p in _test_dir.glob('[!ab]*'))
assert 'apple.txt' not in names_not_ab, names_not_ab
assert 'banana.txt' not in names_not_ab, names_not_ab
assert 'cherry.py' in names_not_ab, names_not_ab
assert 'date.py' in names_not_ab, names_not_ab

# [!a-c]* — files not starting with letters in a-c range
names_not_ac = sorted(p.name for p in _test_dir.glob('[!a-c]*'))
assert 'apple.txt' not in names_not_ac, names_not_ac
assert 'banana.txt' not in names_not_ac, names_not_ac
assert 'cherry.py' not in names_not_ac, names_not_ac
assert 'date.py' in names_not_ac, names_not_ac

# Regression: [abc]* still works (positive class, no negation)
names_abc = sorted(p.name for p in _test_dir.glob('[abc]*'))
assert 'apple.txt' in names_abc, names_abc
assert 'banana.txt' in names_abc, names_abc
assert 'cherry.py' in names_abc, names_abc
assert 'date.py' not in names_abc, names_abc

# Regression: [a-z]* still works (positive range class)
names_az = sorted(p.name for p in _test_dir.glob('[a-z]*'))
assert len(names_az) == 4, names_az  # all four files start with a lowercase letter

# ── #1830: resolve(strict=True) / resolve(strict=False) ──────────────────────

# Existing directory — strict=True must not raise.
r = Path('.').resolve(strict=True)
assert r.is_absolute(), f'resolve(strict=True) on "." is not absolute: {r}'
assert type(r).__name__ == 'PosixPath', type(r).__name__

# /tmp exists on all POSIX systems.
r2 = Path('/tmp').resolve(strict=True)
assert r2.is_absolute()

# strict=False (default) — non-existent path must not raise.
r3 = Path('/tmp/pyrust_definitely_nonexistent_xyz123').resolve(strict=False)
assert r3.is_absolute(), f'resolve(strict=False) is not absolute: {r3}'

# Bare resolve() — same as strict=False.
r4 = Path('/tmp/pyrust_definitely_nonexistent_xyz123').resolve()
assert r4.is_absolute()

# strict=True on a non-existent path must raise FileNotFoundError.
try:
    Path('/tmp/pyrust_pathlib_fixes_nonexistent_xyz123abc').resolve(strict=True)
    assert False, 'should raise FileNotFoundError'
except FileNotFoundError:
    pass

# ── Cleanup ───────────────────────────────────────────────────────────────────

for f in _test_dir.iterdir():
    f.unlink()
os.rmdir(str(_test_dir))

print('pathlib_fixes ok')
