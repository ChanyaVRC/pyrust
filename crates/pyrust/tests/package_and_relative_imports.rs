//! Multi-file integration tests for package imports (`__init__.py`),
//! `__package__`/`__path__`/`__file__` metadata, dotted imports, and relative
//! imports (issue #3141).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn pyrust_bin() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_BIN_EXE_pyrust")
            .expect("CARGO_BIN_EXE_pyrust is not set; run with cargo test"),
    )
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let mut path = env::temp_dir();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!("pyrust_pkg_{tag}_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&path).expect("create temporary test directory");
        Self(path)
    }

    fn write(&self, rel: &str, content: &str) -> PathBuf {
        let full = self.0.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent directories");
        }
        std::fs::write(&full, content).expect("write fixture file");
        full
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn python_string(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
}

fn run_script(script: &Path) -> String {
    let output = Command::new(pyrust_bin())
        .arg(script)
        .output()
        .expect("run pyrust");
    assert!(
        output.status.success(),
        "script {} failed (status {:?}):\nstdout:\n{}\nstderr:\n{}",
        script.display(),
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

#[test]
fn package_discovery_metadata_and_precedence() {
    let dir = TempDir::new("metadata");
    dir.write("plain.py", "VAL = 'plain'\n");
    dir.write("pkg/__init__.py", "PKG_VAL = 'pkg'\n");
    dir.write("pkg/mod.py", "MOD_VAL = 'mod'\n");
    dir.write("pkg/subpkg/__init__.py", "SUBPKG_VAL = 'subpkg'\n");
    dir.write("pkg/subpkg/leaf.py", "LEAF_VAL = 'leaf'\n");

    // Same directory: package (`both/__init__.py`) takes precedence over `both.py`.
    dir.write("both/__init__.py", "KIND = 'package'\n");
    dir.write("both.py", "KIND = 'module'\n");

    // Cross directory: earlier `sys.path` directory wins over later directory.
    dir.write("dir1/cross.py", "KIND = 'dir1_module'\n");
    dir.write("dir2/cross/__init__.py", "KIND = 'dir2_package'\n");

    let script = dir.write(
        "main.py",
        &format!(
            r#"import sys
import plain
import pkg
import pkg.mod
import pkg.subpkg.leaf
import both
print('plain', plain.__name__, repr(plain.__package__), hasattr(plain, '__path__'), plain.__file__.endswith('plain.py'))
print('pkg', pkg.__name__, repr(pkg.__package__), type(pkg.__path__).__name__, len(pkg.__path__), pkg.__path__[0].endswith('pkg'), pkg.__file__.endswith('__init__.py'))
print('pkg_dunders', [k for k in vars(pkg) if k.startswith('__')])
print('mod', pkg.mod.__name__, repr(pkg.mod.__package__), hasattr(pkg.mod, '__path__'), pkg.mod.__file__.endswith('mod.py'))
print('mod_dunders', [k for k in vars(pkg.mod) if k.startswith('__')])
print('subpkg', pkg.subpkg.__name__, repr(pkg.subpkg.__package__), len(pkg.subpkg.__path__), pkg.subpkg.__file__.endswith('__init__.py'))
print('leaf', pkg.subpkg.leaf.__name__, repr(pkg.subpkg.leaf.__package__), hasattr(pkg.subpkg.leaf, '__path__'), pkg.subpkg.leaf.LEAF_VAL)
print('both', both.KIND, hasattr(both, '__path__'))
sys.path.insert(0, '{}')
sys.path.insert(0, '{}')
import cross
print('cross', cross.KIND)
"#,
            python_string(&dir.path().join("dir2")),
            python_string(&dir.path().join("dir1")),
        ),
    );

    let stdout = run_script(&script);
    assert_eq!(
        stdout,
        "\
plain plain '' False True
pkg pkg 'pkg' list 1 True True
pkg_dunders ['__name__', '__doc__', '__package__', '__loader__', '__spec__', '__path__', '__file__', '__cached__', '__builtins__']
mod pkg.mod 'pkg' False True
mod_dunders ['__name__', '__doc__', '__package__', '__loader__', '__spec__', '__file__', '__cached__', '__builtins__']
subpkg pkg.subpkg 'pkg.subpkg' 1 True
leaf pkg.subpkg.leaf 'pkg.subpkg' False leaf
both package True
cross dir1_module
"
    );
}

#[test]
fn dotted_import_forms_binding_order_and_path_isolation() {
    let dir = TempDir::new("dotted");
    dir.write(
        "pkg/__init__.py",
        r#"ORDER = ['pkg']
DEFINED_ATTR = 'from_init'
shadowed = 'attr_wins'
"#,
    );
    dir.write(
        "pkg/sub.py",
        r#"import pkg
pkg.ORDER.append('pkg.sub')
SYM = 123
"#,
    );
    dir.write("pkg/shadowed.py", "SHOULD_NOT_WIN = True\n");
    dir.write("pkg/lazy_sub.py", "LAZY = 'loaded_via_from'\n");
    dir.write("pkg/nested/__init__.py", "NESTED = 'yes'\n");
    dir.write("pkg/nested/deep.py", "DEEP_SYM = 'deep_ok'\n");

    // Top-level module on sys.path MUST NOT be found as `pkg.top_only`.
    dir.write("top_only.py", "TOP = 1\n");
    // Extra directory appended to `pkg.__path__` at runtime.
    dir.write("extra_pkg_dir/dynamic_sub.py", "DYN = 'from_pkg_path'\n");

    let script = dir.write(
        "main.py",
        &format!(
            r#"import sys
import pkg.sub as s
print('alias_only', 'pkg' in globals(), 's' in globals(), s is sys.modules['pkg.sub'])
print('parent_cached', 'pkg' in sys.modules, sys.modules['pkg'].sub is s)
print('order', sys.modules['pkg'].ORDER)
import pkg.nested.deep
print('top_bound', 'pkg' in globals(), pkg is sys.modules['pkg'])
print('chain', pkg.nested is sys.modules['pkg.nested'], pkg.nested.deep is sys.modules['pkg.nested.deep'])
from pkg import lazy_sub, DEFINED_ATTR, shadowed
print('from_pkg', lazy_sub.LAZY, pkg.lazy_sub is lazy_sub, DEFINED_ATTR, shadowed)
from pkg.nested.deep import DEEP_SYM as ds
print('from_deep', ds)
try:
    import pkg.top_only
    print('FAIL_TOP_ONLY')
except ModuleNotFoundError as e:
    print('top_only_blocked', str(e))
pkg.__path__.append('{}')
import pkg.dynamic_sub
print('dynamic_path', pkg.dynamic_sub.DYN, pkg.dynamic_sub.__package__)
"#,
            python_string(&dir.path().join("extra_pkg_dir")),
        ),
    );

    let stdout = run_script(&script);
    assert_eq!(
        stdout,
        "\
alias_only False True True
parent_cached True True
order ['pkg', 'pkg.sub']
top_bound True True
chain True True
from_pkg loaded_via_from True from_init attr_wins
from_deep deep_ok
top_only_blocked No module named 'pkg.top_only'
dynamic_path from_pkg_path pkg
"
    );
}

#[test]
fn relative_imports_across_levels_lazy_functions_star_and_cycles() {
    let dir = TempDir::new("relative");

    // Multi-level package with relative imports in __init__.py, siblings, parents,
    // aunt/uncle modules, star-import with __all__, and function-scoped lazy import.
    dir.write(
        "app/__init__.py",
        r#"APP_CONST = 'root'
def parent_fn():
    return 'from_app'
from . import init_helper
INIT_SAW = init_helper.TAG
"#,
    );
    dir.write("app/init_helper.py", "TAG = 'helper_tag'\n");
    dir.write(
        "app/other.py",
        r#"OTHER_VAL = 'other_value'
def other_fn():
    return 'from_other'
"#,
    );
    dir.write("app/sub/__init__.py", "SUB_CONST = 'sub_root'\n");
    dir.write(
        "app/sub/sibling.py",
        r#"SIB_VAL = 40
def sib_fn(x):
    return x + 2
"#,
    );
    dir.write("app/sub/late.py", "LATE_VAL = 'loaded_lazily'\n");
    dir.write(
        "app/sub/worker.py",
        r#"from . import sibling
from .sibling import sib_fn
from .. import APP_CONST, parent_fn
from ..other import OTHER_VAL, other_fn
RESULT = (sibling.SIB_VAL, sib_fn(sibling.SIB_VAL), APP_CONST, parent_fn(), OTHER_VAL, other_fn())
def load_late():
    from . import late
    return late.LATE_VAL
"#,
    );

    // Circular relative imports between siblings.
    dir.write("circ/__init__.py", "");
    dir.write("circ/a.py", "A1 = 1\nfrom . import b\nA2 = 2\n");
    dir.write(
        "circ/b.py",
        "from . import a\nB_SAW_A1 = a.A1\nB_HAD_A2 = hasattr(a, 'A2')\n",
    );

    // `from pkg import *` with `__all__` referencing an unloaded submodule.
    dir.write(
        "starpkg/__init__.py",
        "__all__ = ['VAL', 'submod']\nVAL = 7\n_HIDDEN = 99\n",
    );
    dir.write("starpkg/submod.py", "X = 'star_sub'\n");

    let script = dir.write(
        "main.py",
        r#"import app
from app.sub import worker
print('init_rel', app.INIT_SAW)
print('worker', worker.RESULT)
print('lazy_fn', worker.load_late())
import circ.a
print('circ', circ.a.A1, circ.a.A2, circ.b.B_SAW_A1, circ.b.B_HAD_A2, circ.b.a is circ.a)
from starpkg import *
print('star', VAL, submod.X, '_HIDDEN' in globals())
"#,
    );

    let stdout = run_script(&script);
    assert_eq!(
        stdout,
        "\
init_rel helper_tag
worker (40, 42, 'root', 'from_app', 'other_value', 'from_other')
lazy_fn loaded_lazily
circ 1 2 1 False True
star 7 star_sub False
"
    );
}

#[test]
fn relative_and_package_import_error_cases() {
    let dir = TempDir::new("errors");
    dir.write("plain.py", "X = 1\n");
    dir.write("top_rel.py", "from . import plain\n");
    dir.write("pkg/__init__.py", "OK = True\n");
    dir.write("pkg/too_deep.py", "from .. import something\n");
    dir.write("pkg/sub/__init__.py", "");
    dir.write("pkg/sub/too_deep3.py", "from ... import something\n");
    dir.write("pkg/bad_dep.py", "import nonexistent_inner_dep_3141\n");
    dir.write("pkg/broken.py", "PARTIAL = 1\nraise RuntimeError('boom')\n");
    dir.write("pkg/halted.py", "NEVER = 1\n");

    let script = dir.write(
        "main.py",
        r#"import sys
# 1. Relative import in top-level __main__
try:
    from . import plain
except ImportError as e:
    print('main_rel', type(e).__name__, str(e))
# 2. Relative import in top-level non-package module
try:
    import top_rel
except ImportError as e:
    print('top_rel', type(e).__name__, str(e))
# 3. Relative import beyond top-level package (1 level deep, '..')
try:
    import pkg.too_deep
except ImportError as e:
    print('too_deep', type(e).__name__, str(e))
# 4. Relative import beyond top-level package (2 levels deep, '...')
try:
    import pkg.sub.too_deep3
except ImportError as e:
    print('too_deep3', type(e).__name__, str(e))
# 5. Dotted import through non-package module
try:
    import plain.child
except ModuleNotFoundError as e:
    print('not_pkg', type(e).__name__, str(e))
# 6. Missing submodule via import
try:
    import pkg.missing_sub
except ModuleNotFoundError as e:
    print('missing_sub', type(e).__name__, str(e))
# 7. Missing attribute / submodule via from-import
try:
    from pkg import missing_attr
except ImportError as e:
    msg = str(e)
    print('missing_attr', type(e).__name__, msg.startswith("cannot import name 'missing_attr' from 'pkg' ("), '__init__.py)' in msg)
# 8. Existing submodule whose body raises ModuleNotFoundError for another dep
try:
    from pkg import bad_dep
except ModuleNotFoundError as e:
    print('bad_dep', type(e).__name__, str(e))
# 9. Failed submodule execution rolls back sys.modules and parent attr
try:
    import pkg.broken
except RuntimeError as e:
    print('broken', str(e), 'pkg' in sys.modules, 'pkg.broken' in sys.modules, hasattr(sys.modules['pkg'], 'broken'))
# 10. Halted submodule via sys.modules[name] = None
sys.modules['pkg.halted'] = None
try:
    from pkg import halted
except ModuleNotFoundError as e:
    print('halted', type(e).__name__, str(e))
"#,
    );

    let stdout = run_script(&script);
    assert_eq!(
        stdout,
        "\
main_rel ImportError attempted relative import with no known parent package
top_rel ImportError attempted relative import with no known parent package
too_deep ImportError attempted relative import beyond top-level package
too_deep3 ImportError attempted relative import beyond top-level package
not_pkg ModuleNotFoundError No module named 'plain.child'; 'plain' is not a package
missing_sub ModuleNotFoundError No module named 'pkg.missing_sub'
missing_attr ImportError True True
bad_dep ModuleNotFoundError No module named 'nonexistent_inner_dep_3141'
broken boom True False False
halted ModuleNotFoundError import of pkg.halted halted; None in sys.modules
"
    );
}

#[test]
fn builtin_dotted_modules_and_dunder_import_regression() {
    let dir = TempDir::new("builtins");
    dir.write("mypkg/__init__.py", "ROOT = 1\n");
    dir.write("mypkg/child.py", "CHILD = 2\n");

    let script = dir.write(
        "main.py",
        r#"import sys
import os.path
import os.path as op
from os.path import join
import collections.abc
from collections.abc import Iterable
print('os_path', os.path is op, sys.modules['os.path'] is os.path, join('a', 'b') == os.path.join('a', 'b'))
print('collections_abc', sys.modules['collections.abc'] is collections.abc, issubclass(list, Iterable))
try:
    import os.definitely_not_a_submodule
except ModuleNotFoundError as e:
    print('os_missing', str(e))
# __import__ with fromlist and level
m_top = __import__('mypkg.child')
m_leaf = __import__('mypkg.child', fromlist=('CHILD',))
m_rel = __import__('child', {'__package__': 'mypkg'}, None, (), 1)
print('dunder_import', m_top.__name__, m_leaf.__name__, m_leaf.CHILD, m_rel is m_leaf)
"#,
    );

    let stdout = run_script(&script);
    assert_eq!(
        stdout,
        "\
os_path True True True
collections_abc True True
os_missing No module named 'os.definitely_not_a_submodule'; 'os' is not a package
dunder_import mypkg mypkg.child 2 True
"
    );
}
