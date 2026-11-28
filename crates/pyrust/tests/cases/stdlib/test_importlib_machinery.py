"""CPython 3.12.3 parity for the native `importlib.machinery` (issue #3493).

Covers the suffix constants, `ModuleSpec`, `BuiltinImporter`,
`FrozenImporter`, `FileFinder` (+ `path_hook`), the file loaders,
`NamespaceLoader` / `_NamespacePath`, `PathFinder`, and the interplay with
`importlib`, `importlib.util`, `pkgutil`, `runpy` and `linecache`.
Temporary paths are printed relative to their scratch directory.
"""

import importlib
import importlib.machinery as machinery
import importlib.util
import linecache
import os
import pkgutil
import runpy
import sys
import tempfile
import warnings

sys.dont_write_bytecode = True

TMP = tempfile.mkdtemp(prefix="machinery3493_")
TMP2 = tempfile.mkdtemp(prefix="machinery3493b_")


def rel(value):
    """Replace the scratch directories in `value` (str / list / tuple)."""
    if isinstance(value, str):
        for path, label in ((TMP2, "<tmp2>"), (TMP, "<tmp>")):
            value = value.replace(repr(path)[1:-1], label).replace(path, label)
        return value
    if isinstance(value, (list, tuple)):
        return type(value)(rel(item) for item in value)
    return value


def write(relpath, text, mode="w", encoding="utf-8"):
    path = os.path.join(TMP, relpath)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if "b" in mode:
        with open(path, mode) as f:
            f.write(text)
    else:
        with open(path, mode, encoding=encoding, newline="") as f:
            f.write(text)
    return path


def show_error(label, fn, *args, **kwargs):
    try:
        result = fn(*args, **kwargs)
    except Exception as exc:
        extra = []
        if isinstance(exc, ImportError):
            extra = [repr(exc.name), rel(exc.path)]
        print(label, type(exc).__name__, rel(str(exc)), *extra)
    else:
        print(label, "no error", rel(repr(result)))


# ---------------------------------------------------------------------------
# 1. Module surface and suffix constants
# ---------------------------------------------------------------------------
print("public:", [n for n in dir(machinery) if not n.startswith("_")])
print("doc:", machinery.__doc__)
print("attached:", importlib.machinery is machinery, sys.modules["importlib.machinery"] is machinery)
print("source:", machinery.SOURCE_SUFFIXES)
print("bytecode:", machinery.BYTECODE_SUFFIXES)
print(
    "aliases:",
    machinery.DEBUG_BYTECODE_SUFFIXES is machinery.BYTECODE_SUFFIXES,
    machinery.OPTIMIZED_BYTECODE_SUFFIXES is machinery.BYTECODE_SUFFIXES,
)
print("extension:", machinery.EXTENSION_SUFFIXES)
print("all:", machinery.all_suffixes())
print(
    "all_fresh:",
    machinery.all_suffixes() is not machinery.all_suffixes(),
    machinery.all_suffixes()
    == machinery.SOURCE_SUFFIXES + machinery.BYTECODE_SUFFIXES + machinery.EXTENSION_SUFFIXES,
)
show_error("all_args:", machinery.all_suffixes, 1)

# ---------------------------------------------------------------------------
# 2. Class structure
# ---------------------------------------------------------------------------
for cls in [
    machinery.ModuleSpec,
    machinery.BuiltinImporter,
    machinery.FrozenImporter,
    machinery.WindowsRegistryFinder,
    machinery.PathFinder,
    machinery.FileFinder,
    machinery.SourceFileLoader,
    machinery.SourcelessFileLoader,
    machinery.ExtensionFileLoader,
    machinery.NamespaceLoader,
]:
    print("class:", repr(cls), cls.__qualname__, [base.__name__ for base in cls.__mro__])
for cls, names in [
    (machinery.ModuleSpec, ["__init__", "__repr__", "__eq__", "cached", "parent", "has_location"]),
    (machinery.BuiltinImporter, ["find_spec", "create_module", "exec_module", "get_code", "is_package", "load_module"]),
    (machinery.FrozenImporter, ["find_spec", "create_module", "exec_module", "get_source", "_resolve_filename"]),
    (machinery.PathFinder, ["invalidate_caches", "_path_hooks", "_path_importer_cache", "_get_spec", "find_spec", "find_distributions"]),
    (machinery.FileFinder, ["path_hook"]),
    (machinery.WindowsRegistryFinder, ["_open_registry", "_search_registry", "find_spec"]),
]:
    kinds = []
    for name in names:
        kind = type(vars(cls)[name]).__name__
        kinds.append(kind if kind in ("classmethod", "staticmethod", "property") else "function")
    print("kinds:", cls.__name__, kinds)
print("hash_none:", machinery.ModuleSpec.__hash__)
print("origins:", machinery.BuiltinImporter._ORIGIN, machinery.FrozenImporter._ORIGIN)
print(
    "registry:",
    machinery.WindowsRegistryFinder.REGISTRY_KEY,
    machinery.WindowsRegistryFinder.REGISTRY_KEY_DEBUG,
    machinery.WindowsRegistryFinder.DEBUG_BUILD,
)
print("docs:", machinery.ModuleSpec.__doc__.splitlines()[0], machinery.NamespaceLoader.__doc__)
print("pathfinder_doc:", machinery.PathFinder.__doc__)
print("filefinder_doc:", machinery.FileFinder.__doc__.splitlines()[0])

import _frozen_importlib
import _frozen_importlib_external

print(
    "frozen_identity:",
    _frozen_importlib.ModuleSpec is machinery.ModuleSpec,
    _frozen_importlib_external.PathFinder is machinery.PathFinder,
    _frozen_importlib_external.WindowsRegistryFinder is machinery.WindowsRegistryFinder,
    _frozen_importlib_external._NamespaceLoader is machinery.NamespaceLoader,
    issubclass(machinery.SourceFileLoader, _frozen_importlib_external.FileLoader),
    issubclass(machinery.SourceFileLoader, _frozen_importlib_external.SourceLoader),
    issubclass(machinery.ExtensionFileLoader, _frozen_importlib_external._LoaderBasics),
)

# ---------------------------------------------------------------------------
# 3. ModuleSpec
# ---------------------------------------------------------------------------
spec = machinery.ModuleSpec("pkg.mod", None, origin="/x/mod.py")
print("spec_repr:", repr(spec))
print(
    "spec_attrs:",
    spec.name,
    spec.loader,
    spec.origin,
    spec.loader_state,
    spec.submodule_search_locations,
    spec.parent,
    spec.has_location,
    spec.cached,
)
print("spec_vars:", list(vars(spec)))
pkg_spec = machinery.ModuleSpec("pkg", "LDR", origin="/x/pkg/__init__.py", is_package=True, loader_state={"k": 1})
print("pkg_repr:", repr(pkg_spec))
print("pkg_attrs:", pkg_spec.submodule_search_locations, pkg_spec.parent, pkg_spec.loader_state)
print("top_parent:", repr(machinery.ModuleSpec("top", None).parent))
print("is_package_truthy:", machinery.ModuleSpec("a", None, is_package=1).submodule_search_locations, machinery.ModuleSpec("a", None, is_package=0).submodule_search_locations)

loc = machinery.ModuleSpec("loc", None, origin=os.path.join("dir", "loc.py"))
print("loc_cached_before:", loc.cached)
loc.has_location = 1
print("loc_has_location:", loc.has_location, loc._set_fileattr)
print("loc_cached:", loc.cached == importlib.util.cache_from_source(os.path.join("dir", "loc.py")))
loc.cached = "custom.pyc"
print("loc_cached_set:", loc.cached, loc._cached)
pyc = machinery.ModuleSpec("pyc", None, origin="dir/pyc.pyc")
pyc.has_location = True
txt = machinery.ModuleSpec("txt", None, origin="dir/data.txt")
txt.has_location = True
print("cached_kinds:", pyc.cached, txt.cached)

same = machinery.ModuleSpec("pkg.mod", None, origin="/x/mod.py")
print("spec_eq:", spec == same, spec != same, spec == pkg_spec, spec == "pkg.mod", spec.__eq__(42))
same.has_location = True
print("spec_eq_location:", spec == same)
try:
    hash(spec)
except TypeError as exc:
    print("spec_hash:", exc)


class SubSpec(machinery.ModuleSpec):
    pass


print("sub_repr:", repr(SubSpec("sub", None, origin="o")))
show_error("spec_noargs:", machinery.ModuleSpec)
show_error("spec_toomany:", machinery.ModuleSpec, "a", None, "origin")
show_error("spec_badkw:", machinery.ModuleSpec, "a", None, bogus=1)
try:
    spec.parent = "x"
except AttributeError:
    print("spec_parent_readonly: AttributeError")

# ---------------------------------------------------------------------------
# 4. BuiltinImporter and FrozenImporter
# ---------------------------------------------------------------------------
BI = machinery.BuiltinImporter
sys_spec = BI.find_spec("sys")
print("bi_sys:", repr(sys_spec), sys_spec.loader is BI, sys_spec.has_location, sys_spec.cached)
print("bi_sys_path:", BI.find_spec("sys", ["ignored"]) is not None)
print("bi_missing:", BI.find_spec("no_such_module_3493"), BI.find_spec("json"))
print("bi_queries:", BI.is_package("sys"), BI.get_source("sys"), BI.get_code("builtins"))
show_error("bi_is_package_err:", BI.is_package, "json")
show_error("bi_get_code_err:", BI.get_code, "no_such_module_3493")
show_error("bi_get_source_err:", BI.get_source, "os")
print("bi_create:", BI.create_module(sys_spec) is sys, BI.exec_module(sys))
show_error("bi_create_err:", BI.create_module, machinery.ModuleSpec("json", BI))
show_error("bi_find_spec_type:", BI.find_spec, 42)
show_error("bi_find_spec_arity:", BI.find_spec)

FI = machinery.FrozenImporter
print("fi_missing:", FI.find_spec("no_such_module_3493"))
show_error("fi_get_code_err:", FI.get_code, "no_such_module_3493")
show_error("fi_get_source_err:", FI.get_source, "no_such_module_3493")
show_error("fi_is_package_err:", FI.is_package, "no_such_module_3493")
print("fi_create_module:", type(FI.create_module(machinery.ModuleSpec("fz", FI))).__name__)

# ---------------------------------------------------------------------------
# 5. Files on disk
# ---------------------------------------------------------------------------
mod_a = write("mod_a.py", "VALUE = 41\ndef bump():\n    return VALUE + 1\n")
pkg_init = write(os.path.join("pkg_b", "__init__.py"), "PKG = 'pkg_b'\n")
pkg_sub = write(os.path.join("pkg_b", "sub.py"), "from . import PKG\nSUB = PKG + '.sub'\n")
write(os.path.join("pkg_b", "data.txt"), "resource text\n")
ns_part = write(os.path.join("ns_c", "part.py"), "PART = 'c'\n")
os.makedirs(os.path.join(TMP2, "ns_c"))
with open(os.path.join(TMP2, "ns_c", "other.py"), "w") as f:
    f.write("OTHER = 'c2'\n")
crlf = write("crlf_mod.py", b"A = 1\r\nB = 2\r\n", mode="wb")
latin = write("latin_mod.py", "# -*- coding: latin-1 -*-\nNAME = '\xe9t\xe9'\n", encoding="latin-1")
main_dir = os.path.join(TMP, "app_dir")
write(os.path.join("app_dir", "__main__.py"), "RESULT = 'ran app_dir'\n")
script = write("script_3493.py", "import sys\nANSWER = 6 * 7\nNAME = __name__\n")
write("run_mod_3493.py", "VALUE = 'run_module'\nNAME = __name__\n")
write("lazy_3493.py", "import sys\nsys.modules.setdefault('lazy_log_3493', []).append('executed')\nLAZY = 'yes'\n")
write("Mixed_Case.py", "X = 1\n")

# ---------------------------------------------------------------------------
# 6. FileFinder
# ---------------------------------------------------------------------------
details = (machinery.SourceFileLoader, machinery.SOURCE_SUFFIXES)
finder = machinery.FileFinder(TMP, details)
print("ff_repr:", repr(finder) == "FileFinder(%r)" % TMP, finder.path == TMP)
print("ff_loaders:", [(suffix, loader.__name__) for suffix, loader in finder._loaders])
print("ff_initial:", finder._path_mtime, finder._path_cache)
a_spec = finder.find_spec("mod_a")
print(
    "ff_mod:",
    a_spec.name,
    rel(a_spec.origin),
    type(a_spec.loader).__name__,
    a_spec.loader.name,
    rel(a_spec.loader.path),
    a_spec.submodule_search_locations,
    a_spec.has_location,
    a_spec.parent == "",
    a_spec.cached == importlib.util.cache_from_source(mod_a),
)
print("ff_mtime:", type(finder._path_mtime).__name__, "mod_a.py" in finder._path_cache)
b_spec = finder.find_spec("pkg_b")
print("ff_pkg:", b_spec.name, rel(b_spec.origin), rel(b_spec.submodule_search_locations), b_spec.parent)
c_spec = finder.find_spec("ns_c")
print("ff_ns:", c_spec.name, c_spec.loader, c_spec.origin, rel(c_spec.submodule_search_locations), c_spec.has_location)
print("ff_missing:", finder.find_spec("missing_3493"), finder.find_spec("data"))
print("ff_dotted:", rel(finder.find_spec("anything.mod_a").origin), finder.find_spec("anything.mod_a").name)
print("ff_case:", finder.find_spec("mixed_case"), finder.find_spec("Mixed_Case").name)
sub_finder = machinery.FileFinder(os.path.join(TMP, "pkg_b"), details)
print("ff_sub:", sub_finder.find_spec("pkg_b.sub").name, sub_finder.find_spec("pkg_b.sub").parent)
late = write("late_3493.py", "LATE = True\n")
finder.invalidate_caches()
print("ff_invalidate:", finder._path_mtime)
print("ff_late:", finder.find_spec("late_3493").name)
print("ff_cwd:", machinery.FileFinder(".", details).path == os.getcwd(), machinery.FileFinder("", details).path == os.getcwd())
multi = machinery.FileFinder(
    TMP,
    (machinery.ExtensionFileLoader, machinery.EXTENSION_SUFFIXES),
    (machinery.SourceFileLoader, machinery.SOURCE_SUFFIXES),
    (machinery.SourcelessFileLoader, machinery.BYTECODE_SUFFIXES),
)
print("ff_multi:", [loader.__name__ for _, loader in multi._loaders][-2:], type(multi.find_spec("mod_a").loader).__name__)
print("ff_no_loaders:", machinery.FileFinder(TMP).find_spec("mod_a"), machinery.FileFinder(TMP).find_spec("ns_c").submodule_search_locations == [os.path.join(TMP, "ns_c")])
show_error("ff_bad_detail:", machinery.FileFinder, TMP, (machinery.SourceFileLoader,))
gone = machinery.FileFinder(os.path.join(TMP, "does_not_exist"), details)
print("ff_gone:", gone.find_spec("mod_a"), gone._path_mtime, gone._path_cache)

hook = machinery.FileFinder.path_hook(details)
print("hook:", hook.__name__, hook.__qualname__, callable(hook), hook.__doc__)
hooked = hook(TMP)
print("hook_call:", type(hooked).__name__, hooked.path == TMP, [s for s, _ in hooked._loaders])
show_error("hook_file:", hook, mod_a)
show_error("hook_missing:", hook, os.path.join(TMP, "nope"))
print("hook_empty:", hook("").path == os.getcwd())


class SubFinder(machinery.FileFinder):
    pass


print("hook_subclass:", type(SubFinder.path_hook(details)(TMP)).__name__)


class LoggingFinder(machinery.FileFinder):
    def _get_spec(self, loader_class, fullname, path, smsl, target):
        print("  _get_spec:", loader_class.__name__, fullname, rel(path), rel(smsl), target)
        return super()._get_spec(loader_class, fullname, path, smsl, target)


LoggingFinder(TMP, details).find_spec("pkg_b", "TARGET")
LoggingFinder(TMP, details).find_spec("mod_a")

# ---------------------------------------------------------------------------
# 7. SourceFileLoader
# ---------------------------------------------------------------------------
SFL = machinery.SourceFileLoader
loader = SFL("mod_a", mod_a)
print("sfl_attrs:", loader.name, rel(loader.path), rel(loader.get_filename()), rel(loader.get_filename("mod_a")))
show_error("sfl_wrong_name:", loader.get_filename, "other")
print("sfl_data:", loader.get_data(mod_a))
print("sfl_source:", repr(loader.get_source("mod_a")))
code = loader.get_code("mod_a")
print("sfl_code:", type(code).__name__, rel(code.co_filename), code.co_name)
namespace = {}
exec(code, namespace)
print("sfl_exec_code:", namespace["VALUE"], namespace["bump"]())
code2 = loader.source_to_code(b"Z = 5\n", "<made-up>")
ns2 = {}
exec(code2, ns2)
print("sfl_source_to_code:", code2.co_filename, ns2["Z"])
print("sfl_source_to_code_str:", loader.source_to_code("W = 'w'", "<str>").co_filename)
try:
    loader.source_to_code("def (", "<bad>")
except SyntaxError as exc:
    print("sfl_source_to_code_syntax:", type(exc).__name__, exc.filename, exc.lineno)
print("sfl_is_package:", loader.is_package("mod_a"), SFL("pkg_b", pkg_init).is_package("pkg_b"), SFL("pkg_b.__init__", pkg_init).is_package("pkg_b.__init__"))
stats = loader.path_stats(mod_a)
print("sfl_stats:", sorted(stats), stats["size"] == os.path.getsize(mod_a), stats["mtime"] == os.stat(mod_a).st_mtime)
try:
    loader.path_stats(os.path.join(TMP, "missing.py"))
except OSError as exc:
    print("sfl_stats_missing:", type(exc).__name__, exc.errno, rel(exc.filename))
print("sfl_create_module:", loader.create_module(a_spec))
print("sfl_eq:", loader == SFL("mod_a", mod_a), loader == SFL("mod_a", pkg_init), loader == SFL("other", mod_a), loader == 42, loader != SFL("mod_a", mod_a))
print("sfl_hash:", hash(loader) == hash(SFL("mod_a", mod_a)), hash(loader) == hash("mod_a") ^ hash(mod_a))
print("sfl_crlf:", repr(SFL("crlf_mod", crlf).get_source("crlf_mod")))
print("sfl_latin:", ascii(SFL("latin_mod", latin).get_source("latin_mod")))
missing_loader = SFL("missing_mod", os.path.join(TMP, "missing_mod.py"))
try:
    missing_loader.get_source("missing_mod")
except ImportError as exc:
    print("sfl_missing_source:", exc, exc.name, type(exc.__cause__).__name__, exc.__suppress_context__)
try:
    missing_loader.get_data(missing_loader.path)
except OSError as exc:
    print("sfl_missing_data:", type(exc).__name__, exc.errno, rel(exc.filename))
try:
    missing_loader.get_code("missing_mod")
except OSError as exc:
    print("sfl_missing_code:", type(exc).__name__, exc.errno, rel(exc.filename))
out = os.path.join(TMP, "made", "deeper", "out.bin")
print("sfl_set_data:", loader.set_data(out, b"\x00\x01binary"), open(out, "rb").read())
loader.set_data(out, b"replaced")
print("sfl_set_data_again:", open(out, "rb").read())
print("sfl_cache_bytecode:", loader._cache_bytecode(mod_a, os.path.join(TMP, "made", "cb.bin"), b"cb"), open(os.path.join(TMP, "made", "cb.bin"), "rb").read())

module = importlib.util.module_from_spec(a_spec)
print("sfl_module_pre:", module.__name__, rel(module.__file__), module.__loader__ is a_spec.loader, hasattr(module, "VALUE"))
a_spec.loader.exec_module(module)
print("sfl_module_post:", module.VALUE, module.bump(), module.bump.__globals__ is module.__dict__)
pkg_module = importlib.util.module_from_spec(b_spec)
b_spec.loader.exec_module(pkg_module)
print("sfl_pkg_module:", pkg_module.PKG, pkg_module.__package__, rel(pkg_module.__path__), rel(pkg_module.__file__))


class NoCodeLoader(SFL):
    def get_code(self, fullname):
        return None


show_error("sfl_no_code:", NoCodeLoader("mod_a", mod_a).exec_module, importlib.util.module_from_spec(a_spec))


class TracingLoader(SFL):
    def get_data(self, path):
        print("  get_data:", rel(path))
        return super().get_data(path)

    def source_to_code(self, data, path, *, _optimize=-1):
        print("  source_to_code:", type(data).__name__, rel(path))
        return super().source_to_code(data, path)


traced = TracingLoader("mod_a", mod_a)
print("sfl_traced_source:", len(traced.get_source("mod_a")))
print("sfl_traced_code:", type(traced.get_code("mod_a")).__name__)

with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    legacy = SFL("legacy_mod_3493", mod_a).load_module()
print("sfl_load_module:", legacy.VALUE, legacy.__name__, sys.modules["legacy_mod_3493"] is legacy, [(w.category.__name__, str(w.message)) for w in caught])
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    again = SFL("legacy_mod_3493", mod_a).load_module("legacy_mod_3493")
print("sfl_load_module_again:", again is legacy, len(caught))
del sys.modules["legacy_mod_3493"]

# ---------------------------------------------------------------------------
# 8. SourcelessFileLoader and ExtensionFileLoader
# ---------------------------------------------------------------------------
bad_magic = write("bad_magic.pyc", b"abcd" + b"\x00" * 12, mode="wb")
short = write("short.pyc", importlib.util.MAGIC_NUMBER + b"\x00\x00", mode="wb")
flags = write("flags.pyc", importlib.util.MAGIC_NUMBER + b"\x04\x00\x00\x00" + b"\x00" * 8, mode="wb")
SLL = machinery.SourcelessFileLoader
show_error("sll_bad_magic:", SLL("bad_magic", bad_magic).get_code, "bad_magic")
show_error("sll_short:", SLL("short", short).get_code, "short")
show_error("sll_flags:", SLL("flags", flags).get_code, "flags")
print("sll_source:", SLL("bad_magic", bad_magic).get_source("bad_magic"), SLL("pkg", os.path.join(TMP, "pkg", "__init__.pyc")).is_package("pkg"))
print("sll_data:", SLL("short", short).get_data(short) == importlib.util.MAGIC_NUMBER + b"\x00\x00")

EFL = machinery.ExtensionFileLoader
ext_path = os.path.join(TMP, "ext" + machinery.EXTENSION_SUFFIXES[0])
ext = EFL("ext", ext_path)
print("efl:", ext.name, rel(ext.path), rel(ext.get_filename("ext")), ext.get_code("ext"), ext.get_source("ext"), ext.is_package("ext"))
print("efl_pkg:", EFL("p", os.path.join(TMP, "__init__" + machinery.EXTENSION_SUFFIXES[-1])).is_package("p"))
print("efl_eq:", ext == EFL("ext", ext_path), hash(ext) == hash(EFL("ext", ext_path)), ext == SFL("ext", ext_path))
show_error("efl_wrong_name:", ext.get_filename, "nope")

# ---------------------------------------------------------------------------
# 9. NamespaceLoader and _NamespacePath
# ---------------------------------------------------------------------------
ns_dir = os.path.join(TMP, "ns_c")
ns_loader = machinery.NamespaceLoader("ns_c", [ns_dir], machinery.PathFinder._get_spec)
ns_path = ns_loader._path
print("nsl:", ns_loader.is_package("ns_c"), repr(ns_loader.get_source("ns_c")), ns_loader.create_module(None), ns_loader.exec_module(None))
ns_code = ns_loader.get_code("ns_c")
print("nsl_code:", type(ns_code).__name__, ns_code.co_filename)
print("nsp:", type(ns_path).__name__, rel(repr(ns_path)), rel(list(ns_path)), len(ns_path), ns_dir in ns_path, rel(ns_path[0]))
ns_path.append(TMP2)
print("nsp_append:", rel(list(ns_path)))
ns_path[1] = os.path.join(TMP2, "ns_c")
print("nsp_setitem:", rel(list(ns_path)), rel(ns_path._path))
print("nsp_parent:", ns_path._find_parent_path_names(), ns_path._get_parent_path() is sys.path)
try:
    machinery.NamespaceLoader("pkg_b_unimported_3493.nested", [ns_dir], None)
except KeyError as exc:
    print("nsp_nested_missing_parent:", exc)
sys.modules["fake_parent_3493"] = type(sys)("fake_parent_3493")
sys.modules["fake_parent_3493"].__path__ = [TMP]
nested = machinery.NamespaceLoader("fake_parent_3493.nested", [ns_dir], None)
print("nsp_nested_parent:", nested._path._find_parent_path_names(), rel(nested._path._get_parent_path()), rel(nested._path._last_parent_path))
recalc_log = []


def recalc_finder(name, parent_path):
    recalc_log.append((name, rel(parent_path)))
    spec = machinery.ModuleSpec(name, None)
    spec.submodule_search_locations = [os.path.join(TMP2, "ns_c")]
    return spec


tracked = machinery.NamespaceLoader("fake_parent_3493.nested", [ns_dir], recalc_finder)._path
print("nsp_recalc_same:", rel(list(tracked)), recalc_log)
sys.modules["fake_parent_3493"].__path__.append(TMP2)
print("nsp_recalc_changed:", rel(list(tracked)), recalc_log)
machinery.PathFinder.invalidate_caches()
print("nsp_recalc_epoch:", len(tracked), len(recalc_log))
del sys.modules["fake_parent_3493"]

# ---------------------------------------------------------------------------
# 10. PathFinder
# ---------------------------------------------------------------------------
PF = machinery.PathFinder
pf_mod = PF.find_spec("mod_a", [TMP])
print("pf_mod:", pf_mod.name, rel(pf_mod.origin), type(pf_mod.loader).__name__, pf_mod.has_location)
pf_pkg = PF.find_spec("pkg_b", [TMP])
print("pf_pkg:", rel(pf_pkg.origin), rel(pf_pkg.submodule_search_locations))
pf_sub = PF.find_spec("pkg_b.sub", pf_pkg.submodule_search_locations)
print("pf_sub:", pf_sub.name, rel(pf_sub.origin))
pf_ns = PF.find_spec("ns_c", [TMP, TMP2])
print("pf_ns:", pf_ns.name, pf_ns.loader, pf_ns.origin, type(pf_ns.submodule_search_locations).__name__, rel(list(pf_ns.submodule_search_locations)))
print("pf_ns_repr:", rel(repr(pf_ns)))
print("pf_missing:", PF.find_spec("missing_3493", [TMP]), PF.find_spec("mod_a", [42, b"bytes", TMP]).name)
print("pf_empty_ns:", PF.find_spec("ns_c", []))
raw = PF._get_spec("ns_c", [TMP])
print("pf_get_spec:", raw.loader, rel(raw.submodule_search_locations), type(raw.submodule_search_locations).__name__)
print("pf_get_spec_none:", rel(PF._get_spec("missing_3493", [TMP]).submodule_search_locations))
sys.path.insert(0, TMP)
try:
    print("pf_sys_path:", rel(PF.find_spec("mod_a").origin))
finally:
    sys.path.remove(TMP)


class VirtualLoader:
    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.VIRTUAL = module.__name__


class VirtualFinder:
    def __init__(self, entry):
        self.entry = entry
        self.invalidated = 0

    def find_spec(self, fullname, target=None):
        if fullname.startswith("virt_"):
            return importlib.util.spec_from_loader(fullname, VirtualLoader(), origin=self.entry + "/" + fullname)
        if fullname == "nsonly":
            spec = machinery.ModuleSpec(fullname, None)
            spec.submodule_search_locations = [self.entry + "/nsonly"]
            return spec
        if fullname == "broken":
            return machinery.ModuleSpec(fullname, None)
        return None

    def invalidate_caches(self):
        self.invalidated += 1


hook_log = []


def virtual_hook(entry):
    hook_log.append(entry)
    if entry.startswith("virt://"):
        return VirtualFinder(entry)
    raise ImportError("not virtual")


sys.path_hooks.insert(0, virtual_hook)
sys.path.insert(0, "virt://a")
try:
    v_spec = PF.find_spec("virt_one", ["virt://a"])
    print("pf_hook:", v_spec.name, v_spec.origin, type(sys.path_importer_cache["virt://a"]).__name__, hook_log)
    PF.find_spec("virt_two", ["virt://a"])
    print("pf_hook_cached:", hook_log)
    ns_only = PF.find_spec("nsonly", ["virt://a", "virt://b"])
    print("pf_hook_ns:", list(ns_only.submodule_search_locations))
    show_error("pf_hook_broken:", PF.find_spec, "broken", ["virt://a"])
    print("pf_hook_cache_lookup:", type(PF._path_importer_cache("virt://a")).__name__)
    finder_a = sys.path_importer_cache["virt://a"]
    sys.path_importer_cache["relative_3493"] = finder_a
    sys.path_importer_cache["none_3493"] = None
    epoch = sys.modules["_frozen_importlib_external"]._NamespacePath._epoch
    PF.invalidate_caches()
    print(
        "pf_invalidate:",
        "relative_3493" in sys.path_importer_cache,
        "none_3493" in sys.path_importer_cache,
        "virt://a" in sys.path_importer_cache,
        _frozen_importlib_external._NamespacePath._epoch - epoch,
    )
    v_module = importlib.import_module("virt_three")
    print("pf_import:", v_module.VIRTUAL, v_module.__spec__.origin)
finally:
    sys.path_hooks.remove(virtual_hook)
    sys.path.remove("virt://a")
    for key in ["virt://a", "virt://b"]:
        sys.path_importer_cache.pop(key, None)
    sys.modules.pop("virt_three", None)

# ---------------------------------------------------------------------------
# 11. importlib / importlib.util interop
# ---------------------------------------------------------------------------
loc_spec = importlib.util.spec_from_file_location("mod_a_copy", mod_a)
print("util_sffl:", type(loc_spec.loader).__name__, loc_spec.loader.name, rel(loc_spec.origin), loc_spec.has_location)
copy = importlib.util.module_from_spec(loc_spec)
loc_spec.loader.exec_module(copy)
print("util_exec:", copy.VALUE, copy.__name__, rel(copy.__file__))
pkg_loc = importlib.util.spec_from_file_location("pkg_b", pkg_init)
print("util_sffl_pkg:", rel(pkg_loc.submodule_search_locations), pkg_loc.parent)
print("util_spec_from_loader:", repr(importlib.util.spec_from_loader("sys", BI)))

sys.path.insert(0, TMP)
try:
    found = importlib.util.find_spec("mod_a")
    print("util_find_spec:", found.name, rel(found.origin), type(found.loader).__name__)
    sub_found = importlib.util.find_spec("pkg_b.sub")
    print("util_find_spec_sub:", sub_found.name, rel(sub_found.origin))
    imported = importlib.import_module("pkg_b.sub")
    print("import_module:", imported.SUB)

    lazy_spec = importlib.util.find_spec("lazy_3493")
    lazy_loader = importlib.util.LazyLoader.factory(machinery.SourceFileLoader)(lazy_spec.name, lazy_spec.origin)
    lazy_spec.loader = lazy_loader
    lazy_module = importlib.util.module_from_spec(lazy_spec)
    sys.modules["lazy_3493"] = lazy_module
    lazy_loader.exec_module(lazy_module)
    print("lazy_before:", sys.modules.get("lazy_log_3493"))
    print("lazy_attr:", lazy_module.LAZY, sys.modules.get("lazy_log_3493"))

    import importlib.resources

    files = importlib.resources.files("pkg_b")
    print("resources:", files.joinpath("data.txt").read_text())
    reader = sub_found.loader.get_resource_reader("pkg_b.sub")
    print("resource_reader:", type(reader).__name__)

    # pkgutil
    importer = pkgutil.get_importer(TMP)
    print("pkgutil_importer:", type(importer).__name__, importer.path == TMP)
    print("pkgutil_iter:", sorted((name, ispkg) for _, name, ispkg in pkgutil.iter_modules([TMP]) if name.endswith(("_a", "_b", "3493"))))
    print("pkgutil_data:", pkgutil.get_data("pkg_b", "data.txt"))
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        print("pkgutil_loader:", type(pkgutil.get_loader("mod_a")).__name__)

    # runpy
    run_globals = runpy.run_module("run_mod_3493")
    print("runpy_module:", run_globals["VALUE"], run_globals["NAME"], type(run_globals["__loader__"]).__name__, rel(run_globals["__file__"]))
    path_globals = runpy.run_path(script)
    print("runpy_path:", path_globals["ANSWER"], path_globals["NAME"])
    dir_globals = runpy.run_path(main_dir)
    print("runpy_dir:", dir_globals["RESULT"], dir_globals["__name__"])
finally:
    sys.path.remove(TMP)
    for name in ["mod_a", "pkg_b", "pkg_b.sub", "lazy_3493", "lazy_log_3493", "run_mod_3493"]:
        sys.modules.pop(name, None)
    sys.path_importer_cache.pop(TMP, None)

# linecache through a loader's get_source
fake_globals = {"__name__": "mod_a", "__loader__": SFL("mod_a", mod_a)}
print("linecache_loader:", repr(linecache.getline("zz_fake_mod_a_3493.py", 2, fake_globals)))
print("linecache_file:", repr(linecache.getline(mod_a, 1)))

import shutil

shutil.rmtree(TMP)
shutil.rmtree(TMP2)
print("done")
