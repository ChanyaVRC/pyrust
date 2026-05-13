# `os.path` — the three import forms.
#
# `import os.path` binds the *topmost* component (CPython package
# semantics), so the running name is `os` and the submodule is reached
# via `os.path.X`.  pyrust ships an `os` parent package
# (crates/pyrust/src/builtin_modules/bodies/os.rs) so that the chain
# resolves; without the parent, only the `as`/`from` forms would work.
#
# This script pins each form's surface so a regression in the dotted-
# import codegen surfaces immediately.  Output is byte-identical to
# CPython under the parity harness.


# ── Form 1: `import os.path` — bind the topmost component ────────────
import os.path

# The name bound is `os`, not `os.path`.
print("form1-call", os.path.join('a', 'b'))
print("form1-splitext", os.path.splitext('foo.txt'))
print("form1-dirname", os.path.dirname('/x/y/z'))
print("form1-basename", os.path.basename('/x/y/z'))

# `os.sep` lives on the parent module (the `os` package), not on the
# submodule.  After `import os.path` the parent is reachable too —
# attribute access on the bound name goes through `os`.
print("form1-sep-is-str", isinstance(os.sep, str))
print("form1-sep-len", len(os.sep))


# ── Form 2: `import os.path as op` — alias binds the leaf directly ──
import os.path as op

# Now `op` IS the os.path submodule (not the parent), so `op.path`
# would be wrong.  Only submodule members are reachable.
print("form2-call", op.join('p', 'q'))
print("form2-splitext", op.splitext('a.tar.gz'))


# ── Form 3: `from os.path import …` — pull individual attrs ─────────
from os.path import join, splitext, dirname

# `join`, `splitext`, `dirname` are now bound directly; no module
# qualifier needed.
print("form3-call", join('m', 'n'))
print("form3-splitext", splitext('with.ext'))
print("form3-dirname", dirname('/a/b/c'))


# ── Multiple imports across forms in the same program ──────────────
# The module is cached after first load, so subsequent imports reuse it.
import os.path as same1
import os.path as same2
print("cache-same-module-1", same1.join('u', 'v'))
print("cache-same-module-2", same2.join('u', 'v'))

# `from` import after `import as` still works.
from os.path import join as join_alias
print("from-after-as", join_alias('first', 'second'))
