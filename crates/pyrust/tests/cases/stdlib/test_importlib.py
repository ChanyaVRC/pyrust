import importlib
import importlib.machinery
import importlib.util
import os
import pathlib
import sys
import tempfile
import types
import warnings

# ---------------------------------------------------------------------------
# 1. Module structure, submodule binding, and __all__
# ---------------------------------------------------------------------------
print("all:", importlib.__all__)
print(
    "identity:",
    importlib is sys.modules["importlib"],
    importlib.util is sys.modules["importlib.util"],
    importlib.machinery is sys.modules["importlib.machinery"],
)

# ---------------------------------------------------------------------------
# 2. importlib.machinery constants & ModuleSpec
# ---------------------------------------------------------------------------
print(
    "suffixes:",
    ".py" in importlib.machinery.SOURCE_SUFFIXES,
    importlib.machinery.BYTECODE_SUFFIXES,
    isinstance(importlib.machinery.EXTENSION_SUFFIXES, list),
    ".py" in importlib.machinery.all_suffixes(),
    ".pyc" in importlib.machinery.all_suffixes(),
)

s1 = importlib.machinery.ModuleSpec("pkg.sub", None, origin="/a/b.py")
print("s1_repr:", repr(s1))
print(
    "s1_attrs:",
    s1.name,
    s1.loader,
    s1.origin,
    s1.submodule_search_locations,
    s1.loader_state,
    s1.cached,
    s1.parent,
    s1.has_location,
)

s2 = importlib.machinery.ModuleSpec(
    "pkg", None, origin="/a/__init__.py", loader_state={"k": 1}, is_package=True
)
print("s2_repr:", repr(s2))
print(
    "s2_attrs:",
    s2.name,
    s2.submodule_search_locations,
    s2.loader_state,
    s2.parent,
    s2.has_location,
    s2.cached,
)

s3 = importlib.machinery.ModuleSpec("top", None)
print("s3_parent:", repr(s3.parent), repr(s3))

s1_copy = importlib.machinery.ModuleSpec("pkg.sub", None, origin="/a/b.py")
print("spec_eq:", s1 == s1_copy, s1 == s2, s1 == "not_a_spec")

s_loc = importlib.machinery.ModuleSpec("locmod", None, origin="dir/locmod.py")
s_loc.has_location = True
print(
    "spec_loc_cached:",
    s_loc.has_location,
    s_loc.cached == importlib.util.cache_from_source("dir/locmod.py"),
)
s_loc.cached = "custom.pyc"
print("spec_cached_set:", s_loc.cached)

# ---------------------------------------------------------------------------
# 3. importlib.util.MAGIC_NUMBER, cache_from_source, source_from_cache, decode_source
# ---------------------------------------------------------------------------
print(
    "magic:",
    isinstance(importlib.util.MAGIC_NUMBER, bytes),
    len(importlib.util.MAGIC_NUMBER),
    importlib.util.MAGIC_NUMBER,
)

c_default = importlib.util.cache_from_source(os.path.join("foo", "bar.py"))
c_opt1 = importlib.util.cache_from_source(os.path.join("foo", "bar.py"), optimization=1)
c_opt2 = importlib.util.cache_from_source(os.path.join("foo", "bar.py"), optimization=2)
c_empty = importlib.util.cache_from_source(os.path.join("foo", "bar.py"), optimization="")
c_path = importlib.util.cache_from_source(pathlib.Path("foo") / "bar.py")
print(
    "cache_paths:",
    c_default == c_empty == c_path,
    c_default.endswith(os.path.join("__pycache__", "bar.cpython-312.pyc")),
    c_opt1.endswith(os.path.join("__pycache__", "bar.cpython-312.opt-1.pyc")),
    c_opt2.endswith(os.path.join("__pycache__", "bar.cpython-312.opt-2.pyc")),
)
print(
    "source_from_cache:",
    importlib.util.source_from_cache(c_default) == os.path.join("foo", "bar.py"),
    importlib.util.source_from_cache(c_opt1) == os.path.join("foo", "bar.py"),
)

with warnings.catch_warnings():
    warnings.simplefilter("ignore", DeprecationWarning)
    c_dbg_f = importlib.util.cache_from_source(
        os.path.join("foo", "bar.py"), debug_override=False
    )
    c_dbg_t = importlib.util.cache_from_source(
        os.path.join("foo", "bar.py"), debug_override=True
    )
    print("debug_override:", c_dbg_f == c_opt1, c_dbg_t == c_default)
    try:
        importlib.util.cache_from_source("a.py", debug_override=True, optimization=1)
    except TypeError as e:
        print("cache_both_err:", str(e))

try:
    importlib.util.cache_from_source("a.py", optimization="bad-opt")
except ValueError as e:
    print("cache_opt_err:", str(e))

try:
    importlib.util.source_from_cache(os.path.join("foo", "bar.cpython-312.pyc"))
except ValueError as e:
    print("src_from_cache_err:", "__pycache__" in str(e))

print(
    "decode_source:",
    repr(importlib.util.decode_source(b"x = 1\r\ny = 2\r\n")),
    repr(importlib.util.decode_source(b"\xef\xbb\xbfz = 'ok'\n")),
    repr(importlib.util.decode_source(b"# coding: latin-1\ns = 'caf\xe9'\n")),
)

# ---------------------------------------------------------------------------
# 4. importlib.util.resolve_name
# ---------------------------------------------------------------------------
resolve_cases = [
    ("foo", None),
    ("foo.bar", None),
    ("foo", "pkg"),
    (".bar", "pkg"),
    (".bar", "pkg.sub"),
    ("..bar", "pkg.sub"),
    (".", "pkg"),
    ("..", "pkg.sub"),
    (".bar", None),
    (".bar", ""),
    ("..bar", "pkg"),
    ("...bar", "pkg.sub"),
    ("..", "pkg"),
    ("", None),
    ("", "pkg"),
]
for mod_name, pkg_name in resolve_cases:
    try:
        res = importlib.util.resolve_name(mod_name, pkg_name)
        print("resolve:", mod_name, pkg_name, "->", repr(res))
    except Exception as e:
        print("resolve_err:", mod_name, pkg_name, "->", type(e).__name__, str(e))

# ---------------------------------------------------------------------------
# 5. importlib.import_module (built-in, dotted, error cases)
# ---------------------------------------------------------------------------
m_json = importlib.import_module("json")
m_up = importlib.import_module("urllib.parse")
m_cabc = importlib.import_module("collections.abc")
print(
    "import_builtin:",
    m_json.__name__,
    m_json.dumps([1, 2]),
    m_up.__name__,
    "urllib" in sys.modules,
    sys.modules["urllib"].parse is m_up,
    m_cabc.__name__,
    sys.modules["collections"].abc is m_cabc,
)

for mod_name, pkg_name in [
    (".sub", None),
    (".sub", ""),
    (".sub", 123),
    ("..sub", "pkg"),
    ("", None),
    ("nonexistent_mod_3189_xyz", None),
]:
    try:
        importlib.import_module(mod_name, pkg_name)
    except Exception as e:
        print(
            "import_err:",
            mod_name,
            pkg_name,
            "->",
            type(e).__name__,
            str(e),
            getattr(e, "name", None),
        )

# ---------------------------------------------------------------------------
# 6. importlib.util.find_spec (built-in & error cases)
# ---------------------------------------------------------------------------
sp_sys = importlib.util.find_spec("sys")
sp_math = importlib.util.find_spec("math")
sp_imp = importlib.util.find_spec("importlib")
sp_imp_util = importlib.util.find_spec("importlib.util")
sp_imp_rel = importlib.util.find_spec(".util", "importlib")
sp_json = importlib.util.find_spec("json")
print(
    "find_spec_builtin:",
    sp_sys.name,
    repr(sp_sys.parent),
    sp_sys.submodule_search_locations,
    sp_math.name,
    sp_imp.name,
    sp_imp.parent,
    sp_imp.submodule_search_locations is not None,
    sp_imp_util.name,
    sp_imp_util.parent,
    sp_imp_rel.name,
    sp_json.name,
    sp_json.submodule_search_locations is not None,
)
print(
    "find_spec_missing:",
    importlib.util.find_spec("nonexistent_3189_xyz"),
    importlib.util.find_spec("importlib.nonexistent_3189"),
    importlib.util.find_spec("json.nonexistent_3189"),
    importlib.util.find_spec(""),
)

for target_name in ["nonexistent_parent_3189.child", "sys.sub", "math.sub", "__main__"]:
    try:
        importlib.util.find_spec(target_name)
    except Exception as e:
        print(
            "find_spec_err:",
            target_name,
            "->",
            type(e).__name__,
            str(e),
            getattr(e, "name", None),
        )

# Halted module in sys.modules returns None; user module with __spec__=None raises ValueError.
sys.modules["halted_3189"] = None
print("find_spec_halted:", importlib.util.find_spec("halted_3189"))
del sys.modules["halted_3189"]

fake_mod = types.ModuleType("fake_no_spec_3189")
sys.modules["fake_no_spec_3189"] = fake_mod
try:
    importlib.util.find_spec("fake_no_spec_3189")
except ValueError as e:
    print("find_spec_none_spec:", str(e))
del fake_mod.__spec__
try:
    importlib.util.find_spec("fake_no_spec_3189")
except ValueError as e:
    print("find_spec_del_spec:", str(e))
del sys.modules["fake_no_spec_3189"]

# ---------------------------------------------------------------------------
# 7. Filesystem modules, packages, relative import_module, reload, find_spec,
#    spec_from_file_location, module_from_spec, SourceFileLoader
# ---------------------------------------------------------------------------
with tempfile.TemporaryDirectory() as tmpdir:
    # Write a standalone module + a nested package tree on sys.path.
    mod_a_path = os.path.join(tmpdir, "mod_a_3189.py")
    with open(mod_a_path, "w") as f:
        f.write(
            '"""Initial docstring."""\n'
            "VALUE = 1\n"
            "KEEP = 'preserved'\n"
            "def read_val():\n"
            "    return VALUE\n"
        )

    pkg_dir = os.path.join(tmpdir, "pkg_3189")
    deep_dir = os.path.join(pkg_dir, "deep")
    os.makedirs(deep_dir)
    with open(os.path.join(pkg_dir, "__init__.py"), "w") as f:
        f.write("PKG_VER = 1\n")
    with open(os.path.join(pkg_dir, "other.py"), "w") as f:
        f.write("OTHER = 'from_other'\n")
    with open(os.path.join(deep_dir, "__init__.py"), "w") as f:
        f.write("DEEP = True\n")
    with open(os.path.join(deep_dir, "leaf.py"), "w") as f:
        f.write(
            "from .. import other\n"
            "LEAF_TAG = 'v1:' + other.OTHER\n"
        )

    # Also write a standalone module and package NOT on sys.path for spec_from_file_location.
    ext_dir = os.path.join(tmpdir, "external_only")
    ext_pkg_dir = os.path.join(ext_dir, "ext_pkg")
    os.makedirs(ext_pkg_dir)
    ext_mod_path = os.path.join(ext_dir, "dyn_mod.py")
    with open(ext_mod_path, "w") as f:
        f.write(
            '"""Dynamic module doc."""\n'
            "NUM = 41\n"
            "def inc(x):\n"
            "    return x + 1\n"
            "COMPUTED = inc(NUM)\n"
        )
    ext_init_path = os.path.join(ext_pkg_dir, "__init__.py")
    with open(ext_init_path, "w") as f:
        f.write("EXT_PKG_READY = True\n")

    sys.path.insert(0, tmpdir)
    try:
        # find_spec before importing filesystem modules
        sp_mod_a = importlib.util.find_spec("mod_a_3189")
        sp_pkg = importlib.util.find_spec("pkg_3189")
        sp_leaf = importlib.util.find_spec("pkg_3189.deep.leaf")
        sp_rel = importlib.util.find_spec(".other", "pkg_3189")
        sp_missing = importlib.util.find_spec("pkg_3189.does_not_exist")
        print(
            "fs_find_spec:",
            sp_mod_a.name,
            sp_mod_a.has_location,
            sp_mod_a.origin == os.path.abspath(mod_a_path),
            sp_mod_a.submodule_search_locations,
            sp_pkg.name,
            sp_pkg.parent,
            sp_pkg.submodule_search_locations == [os.path.abspath(pkg_dir)],
            sp_leaf.name,
            sp_leaf.parent,
            sp_rel.name,
            sp_missing,
        )

        # import_module (absolute & relative)
        mod_a = importlib.import_module("mod_a_3189")
        pkg = importlib.import_module("pkg_3189")
        leaf = importlib.import_module(".deep.leaf", "pkg_3189")
        other = importlib.import_module("..other", "pkg_3189.deep")
        pkg_self = importlib.import_module(".", "pkg_3189")
        pkg_up = importlib.import_module("..", "pkg_3189.deep")
        print(
            "fs_import_module:",
            mod_a.VALUE,
            mod_a.read_val(),
            leaf.__name__,
            leaf.LEAF_TAG,
            other is sys.modules["pkg_3189.other"],
            pkg_self is pkg,
            pkg_up is pkg,
        )

        # find_spec on already-loaded filesystem modules
        sp_mod_a_loaded = importlib.util.find_spec("mod_a_3189")
        sp_pkg_loaded = importlib.util.find_spec("pkg_3189")
        print(
            "fs_find_spec_loaded:",
            sp_mod_a_loaded.name,
            sp_mod_a_loaded.origin == os.path.abspath(mod_a_path),
            sp_pkg_loaded.submodule_search_locations == [os.path.abspath(pkg_dir)],
        )

        # find_spec through a non-package filesystem module
        try:
            importlib.util.find_spec("mod_a_3189.child")
        except ModuleNotFoundError as e:
            print("fs_not_pkg_err:", type(e).__name__, str(e), e.name)

        # reload: modify mod_a_3189.py on disk and verify in-place identity + updated code
        old_read_val = mod_a.read_val
        with open(mod_a_path, "w") as f:
            f.write(
                '"""Updated docstring."""\n'
                "VALUE = 100\n"
                "ADDED = 'new_attr'\n"
                "def read_val():\n"
                "    return VALUE + 23\n"
                "DERIVED = read_val()\n"
            )
        importlib.invalidate_caches()
        reloaded_a = importlib.reload(mod_a)
        print(
            "reload_mod:",
            reloaded_a is mod_a,
            mod_a.VALUE,
            mod_a.KEEP,
            mod_a.ADDED,
            mod_a.read_val(),
            mod_a.DERIVED,
            old_read_val(),
            mod_a.__doc__,
        )

        # reload a package submodule that performs a relative import
        # (change byte length so CPython's same-second .pyc size check invalidates)
        with open(os.path.join(deep_dir, "leaf.py"), "w") as f:
            f.write(
                "from .. import other\n"
                "LEAF_TAG = 'v2_updated:' + other.OTHER\n"
            )
        importlib.invalidate_caches()
        reloaded_leaf = importlib.reload(leaf)
        print("reload_sub:", reloaded_leaf is leaf, leaf.LEAF_TAG)

        # reload a package (__init__.py) and verify submodule attribute survives
        with open(os.path.join(pkg_dir, "__init__.py"), "w") as f:
            f.write("PKG_VER = 200\nPKG_RELOADED = True\n")
        importlib.invalidate_caches()
        reloaded_pkg = importlib.reload(pkg)
        print("reload_pkg:", reloaded_pkg is pkg, pkg.PKG_VER, pkg.PKG_RELOADED, pkg.other is other)

        # spec_from_file_location + module_from_spec + spec.loader.exec_module
        ext_spec = importlib.util.spec_from_file_location("custom_dyn_3189", ext_mod_path)
        print(
            "ext_spec:",
            ext_spec.name,
            ext_spec.parent,
            ext_spec.has_location,
            ext_spec.origin == os.path.abspath(ext_mod_path),
            ext_spec.submodule_search_locations,
            isinstance(ext_spec.loader, importlib.machinery.SourceFileLoader),
            ext_spec.cached == importlib.util.cache_from_source(os.path.abspath(ext_mod_path)),
        )
        ext_mod = importlib.util.module_from_spec(ext_spec)
        print(
            "ext_mod_pre:",
            ext_mod.__name__,
            repr(ext_mod.__package__),
            ext_mod.__spec__ is ext_spec,
            ext_mod.__loader__ is ext_spec.loader,
            ext_mod.__file__ == ext_spec.origin,
            hasattr(ext_mod, "__path__"),
            ext_mod.__doc__,
        )
        ext_spec.loader.exec_module(ext_mod)
        print(
            "ext_mod_post:",
            ext_mod.NUM,
            ext_mod.COMPUTED,
            ext_mod.inc(99),
            ext_mod.__doc__,
            ext_mod.inc.__globals__ is ext_mod.__dict__,
        )

        # spec_from_file_location for a package (__init__.py) with pathlib.Path
        ext_pkg_spec = importlib.util.spec_from_file_location(
            "custom_ext_pkg_3189", pathlib.Path(ext_init_path)
        )
        ext_pkg_mod = importlib.util.module_from_spec(ext_pkg_spec)
        ext_pkg_spec.loader.exec_module(ext_pkg_mod)
        print(
            "ext_pkg:",
            ext_pkg_spec.parent,
            ext_pkg_spec.submodule_search_locations == [os.path.abspath(ext_pkg_dir)],
            ext_pkg_mod.__package__,
            ext_pkg_mod.__path__ == [os.path.abspath(ext_pkg_dir)],
            ext_pkg_mod.EXT_PKG_READY,
        )

        # SourceFileLoader methods & equality
        ldr1 = importlib.machinery.SourceFileLoader("custom_dyn_3189", os.path.abspath(ext_mod_path))
        ldr2 = importlib.machinery.SourceFileLoader("custom_dyn_3189", os.path.abspath(ext_mod_path))
        print(
            "sfl:",
            ldr1 == ldr2,
            hash(ldr1) == hash(ldr2),
            ldr1.get_filename("custom_dyn_3189") == os.path.abspath(ext_mod_path),
            ldr1.is_package("custom_dyn_3189"),
            "COMPUTED" in ldr1.get_source("custom_dyn_3189"),
            ldr1.create_module(ext_spec) is None,
        )
        try:
            ldr1.get_filename("wrong_name")
        except ImportError as e:
            print("sfl_mismatch:", type(e).__name__, e.name)

        # spec_from_file_location edge cases
        print(
            "spec_edge:",
            importlib.util.spec_from_file_location("bad_ext", os.path.join(tmpdir, "x.txt")),
            importlib.util.spec_from_file_location("no_loc", None),
        )
    finally:
        sys.path.remove(tmpdir)
        for k in [
            "mod_a_3189",
            "pkg_3189",
            "pkg_3189.other",
            "pkg_3189.deep",
            "pkg_3189.deep.leaf",
        ]:
            sys.modules.pop(k, None)

# ---------------------------------------------------------------------------
# 8. spec_from_loader, custom loader, module_from_spec, meta_path & invalidate_caches
# ---------------------------------------------------------------------------
class CustomLoader:
    _ORIGIN = "custom-origin"

    def is_package(self, fullname):
        return fullname.endswith("_pkg")

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.LOADED_BY_CUSTOM = module.__name__


custom_loader = CustomLoader()
sp_custom = importlib.util.spec_from_loader("my_custom_mod", custom_loader)
print("spec_from_loader:", sp_custom.name, sp_custom.origin, sp_custom.submodule_search_locations, sp_custom.parent)
m_custom = importlib.util.module_from_spec(sp_custom)
sp_custom.loader.exec_module(m_custom)
print("custom_exec:", m_custom.__name__, m_custom.__spec__ is sp_custom, m_custom.LOADED_BY_CUSTOM)

sp_custom_pkg = importlib.util.spec_from_loader("my_custom_pkg", custom_loader)
print("spec_from_loader_pkg:", sp_custom_pkg.submodule_search_locations, sp_custom_pkg.parent)


class BadLoader:
    def exec_module(self, module):
        pass


sp_bad = importlib.machinery.ModuleSpec("bad_ldr_mod", BadLoader())
try:
    importlib.util.module_from_spec(sp_bad)
except ImportError as e:
    print("bad_loader_err:", str(e))


class TrackingFinder:
    def __init__(self):
        self.invalidated = 0

    def invalidate_caches(self):
        self.invalidated += 1

    def find_spec(self, fullname, path, target=None):
        if fullname == "virtual_meta_mod_3189":
            return importlib.util.spec_from_loader(fullname, custom_loader)
        return None


tracker = TrackingFinder()
sys.meta_path.insert(0, tracker)
try:
    importlib.invalidate_caches()
    print("invalidated:", tracker.invalidated)
    sp_v = importlib.util.find_spec("virtual_meta_mod_3189")
    print("meta_find_spec:", sp_v.name, sp_v.origin)
    m_v = importlib.import_module("virtual_meta_mod_3189")
    print("meta_import:", m_v.__name__, m_v.LOADED_BY_CUSTOM, sys.modules["virtual_meta_mod_3189"] is m_v)
finally:
    sys.meta_path.remove(tracker)
    sys.modules.pop("virtual_meta_mod_3189", None)

# ---------------------------------------------------------------------------
# 9. importlib.reload on built-in modules and error cases
# ---------------------------------------------------------------------------
import math

print("reload_builtin:", importlib.reload(math) is math)

for bad in [123, "math", None, object()]:
    try:
        importlib.reload(bad)
    except Exception as e:
        print("reload_type_err:", type(bad).__name__, "->", type(e).__name__, str(e))

unreg = types.ModuleType("unregistered_mod_3189")
try:
    importlib.reload(unreg)
except ImportError as e:
    print("reload_unreg:", type(e).__name__, str(e), e.name)

m1 = types.ModuleType("mismatch_mod_3189")
sys.modules["mismatch_mod_3189"] = types.ModuleType("other_3189")
try:
    importlib.reload(m1)
except ImportError as e:
    print("reload_mismatch:", type(e).__name__, str(e), e.name)
finally:
    sys.modules.pop("mismatch_mod_3189", None)

orphan = types.ModuleType("missing_parent_3189.child")
sys.modules["missing_parent_3189.child"] = orphan
try:
    importlib.reload(orphan)
except ImportError as e:
    print("reload_orphan:", type(e).__name__, str(e), e.name)
finally:
    sys.modules.pop("missing_parent_3189.child", None)
