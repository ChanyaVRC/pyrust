import importlib
import importlib.machinery
import importlib.util
import _frozen_importlib
import _frozen_importlib_external
import os
import sys
import tempfile
import types

# ---------------------------------------------------------------------------
# 1. _frozen_importlib and _frozen_importlib_external shims
# ---------------------------------------------------------------------------
print(
    "frozen_core:",
    _frozen_importlib.ModuleSpec is importlib.machinery.ModuleSpec,
    _frozen_importlib.BuiltinImporter is importlib.machinery.BuiltinImporter,
    _frozen_importlib.FrozenImporter is importlib.machinery.FrozenImporter,
    _frozen_importlib.spec_from_loader is importlib.util.spec_from_loader,
    _frozen_importlib.module_from_spec is importlib.util.module_from_spec,
)
print(
    "frozen_ext:",
    _frozen_importlib_external.PathFinder is importlib.machinery.PathFinder,
    _frozen_importlib_external.FileFinder is importlib.machinery.FileFinder,
    _frozen_importlib_external.SourceFileLoader is importlib.machinery.SourceFileLoader,
    _frozen_importlib_external.spec_from_file_location
    is importlib.util.spec_from_file_location,
    _frozen_importlib_external.MAGIC_NUMBER == importlib.util.MAGIC_NUMBER,
)

# ---------------------------------------------------------------------------
# 2. Live sys.meta_path finder & loader (import statement + import_module)
# ---------------------------------------------------------------------------
class VirtualLoader:
    def __init__(self, sources):
        self.sources = sources
        self.events = []

    def create_module(self, spec):
        self.events.append(("create", spec.name))
        if spec.name == "vmeta_custom_obj":
            m = types.ModuleType(spec.name)
            m.FROM_CREATE = True
            return m
        return None

    def exec_module(self, module):
        self.events.append(("exec", module.__name__))
        code = self.sources[module.__name__]
        exec(code, module.__dict__)


class VirtualMetaFinder:
    def __init__(self, sources, packages=()):
        self.loader = VirtualLoader(sources)
        self.sources = sources
        self.packages = set(packages)
        self.seen_paths = []

    def find_spec(self, fullname, path, target=None):
        if fullname in self.sources:
            self.seen_paths.append((fullname, list(path) if path is not None else None))
            is_pkg = fullname in self.packages
            spec = importlib.machinery.ModuleSpec(
                fullname,
                self.loader,
                origin=f"virtual://{fullname}",
                is_package=is_pkg,
            )
            if is_pkg:
                spec.submodule_search_locations = [f"vpath://{fullname}"]
            return spec
        return None


meta_finder = VirtualMetaFinder(
    sources={
        "vmeta_pkg": "PKG_VAL = 'root'\n",
        "vmeta_pkg.child": "from . import PKG_VAL\nCHILD_VAL = PKG_VAL + ':child'\n",
        "vmeta_custom_obj": "AFTER_EXEC = FROM_CREATE and True\n",
        "vmeta_broken": "BEFORE = 1\nraise RuntimeError('boom_meta')\n",
    },
    packages={"vmeta_pkg"},
)

sys.meta_path.insert(0, meta_finder)
try:
    import vmeta_pkg.child
    from vmeta_pkg.child import CHILD_VAL
    import vmeta_custom_obj

    print(
        "meta_stmt:",
        vmeta_pkg.PKG_VAL,
        vmeta_pkg.child.CHILD_VAL,
        CHILD_VAL,
        vmeta_pkg.child is sys.modules["vmeta_pkg.child"],
        vmeta_pkg.__package__,
        vmeta_pkg.child.__package__,
        vmeta_custom_obj.FROM_CREATE,
        vmeta_custom_obj.AFTER_EXEC,
    )
    print("meta_seen_paths:", meta_finder.seen_paths)
    print("meta_events:", meta_finder.loader.events)

    try:
        import vmeta_broken  # noqa: F401
    except RuntimeError as e:
        print("meta_broken:", str(e), "vmeta_broken" in sys.modules)
finally:
    sys.meta_path.remove(meta_finder)
    for k in ["vmeta_pkg", "vmeta_pkg.child", "vmeta_custom_obj", "vmeta_broken"]:
        sys.modules.pop(k, None)

# ---------------------------------------------------------------------------
# 3. Live sys.path_hooks & sys.path_importer_cache
# ---------------------------------------------------------------------------
class MemPathLoader:
    def __init__(self, source):
        self.source = source

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        exec(self.source, module.__dict__)


class MemPathFinder:
    def __init__(self, prefix, modules):
        self.prefix = prefix
        self.modules = modules

    def find_spec(self, fullname, target=None):
        if fullname in self.modules:
            src, is_pkg = self.modules[fullname]
            spec = importlib.machinery.ModuleSpec(
                fullname,
                MemPathLoader(src),
                origin=f"{self.prefix}/{fullname}.py",
                is_package=is_pkg,
            )
            if is_pkg:
                spec.submodule_search_locations = [self.prefix]
            return spec
        return None


hook_calls = []
mem_modules = {
    "phook_pkg": ("HOOK_PKG = 10\n", True),
    "phook_pkg.sub": ("from . import HOOK_PKG\nSUB_NUM = HOOK_PKG + 5\n", False),
    "phook_mod": ("ANSWER = 42\n", False),
}


def my_path_hook(entry):
    hook_calls.append(entry)
    if entry == "memhook://root":
        return MemPathFinder(entry, mem_modules)
    raise ImportError("not a memhook entry")


sys.path_hooks.insert(0, my_path_hook)
sys.path.insert(0, "memhook://root")
sys.path.insert(0, "memhook://rejected_entry_3244")
try:
    import phook_mod
    import phook_pkg.sub

    m_via_il = importlib.import_module("phook_pkg.sub")
    print(
        "path_hook_import:",
        phook_mod.ANSWER,
        phook_pkg.HOOK_PKG,
        phook_pkg.sub.SUB_NUM,
        m_via_il is phook_pkg.sub,
    )
    print(
        "path_importer_cache:",
        isinstance(sys.path_importer_cache.get("memhook://root"), MemPathFinder),
        "memhook://rejected_entry_3244" in sys.path_importer_cache,
        sys.path_importer_cache.get("memhook://rejected_entry_3244") is None,
    )
    # Second missing import should reuse cached None without re-invoking hook
    # for the rejected entry.
    before_count = hook_calls.count("memhook://rejected_entry_3244")
    try:
        import definitely_missing_hook_mod_3244  # noqa: F401
    except ModuleNotFoundError:
        pass
    after_count = hook_calls.count("memhook://rejected_entry_3244")
    print("rejected_cached_reused:", before_count == 1, after_count == 1)
finally:
    sys.path_hooks.remove(my_path_hook)
    sys.path.remove("memhook://root")
    sys.path.remove("memhook://rejected_entry_3244")
    sys.path_importer_cache.pop("memhook://root", None)
    sys.path_importer_cache.pop("memhook://rejected_entry_3244", None)
    for k in ["phook_mod", "phook_pkg", "phook_pkg.sub"]:
        sys.modules.pop(k, None)

# ---------------------------------------------------------------------------
# 4. PEP 420 implicit namespace packages (single, multi-dir, nested, precedence)
# ---------------------------------------------------------------------------
with tempfile.TemporaryDirectory() as tmpdir:
    r1 = os.path.join(tmpdir, "root1")
    r2 = os.path.join(tmpdir, "root2")
    os.makedirs(os.path.join(r1, "ns3244", "nested"))
    os.makedirs(os.path.join(r2, "ns3244", "nested"))
    os.makedirs(os.path.join(r1, "prec3244"))
    os.makedirs(os.path.join(r2, "prec3244"))

    with open(os.path.join(r1, "ns3244", "alpha.py"), "w", encoding="utf-8") as f:
        f.write("TAG = 'from_r1'\n")
    with open(os.path.join(r2, "ns3244", "beta.py"), "w", encoding="utf-8") as f:
        f.write("from . import alpha\nTAG = 'from_r2:' + alpha.TAG\n")
    with open(
        os.path.join(r1, "ns3244", "nested", "x.py"), "w", encoding="utf-8"
    ) as f:
        f.write("X = 100\n")
    with open(
        os.path.join(r2, "ns3244", "nested", "y.py"), "w", encoding="utf-8"
    ) as f:
        f.write("Y = 200\n")

    # Precedence test: r1/prec3244 has NO __init__.py, r2/prec3244 HAS __init__.py.
    # Regular package in r2 must win over namespace directory in r1!
    with open(
        os.path.join(r2, "prec3244", "__init__.py"), "w", encoding="utf-8"
    ) as f:
        f.write("REGULAR = True\n")

    sys.path.insert(0, r2)
    sys.path.insert(0, r1)
    try:
        import ns3244
        import ns3244.alpha
        from ns3244 import beta
        import ns3244.nested.x
        import ns3244.nested.y

        print(
            "pep420_top:",
            ns3244.__name__,
            ns3244.__package__,
            getattr(ns3244, "__file__", None) is None,
            len(list(ns3244.__path__)),
            os.path.abspath(os.path.join(r1, "ns3244")) in list(ns3244.__path__),
            os.path.abspath(os.path.join(r2, "ns3244")) in list(ns3244.__path__),
        )
        print(
            "pep420_subs:",
            ns3244.alpha.TAG,
            beta.TAG,
            ns3244.alpha.__package__,
            beta.__package__,
            ns3244.nested.__package__,
            len(list(ns3244.nested.__path__)),
            ns3244.nested.x.X,
            ns3244.nested.y.Y,
        )

        # find_spec on PEP 420 namespace package
        ns_spec = importlib.util.find_spec("ns3244")
        print(
            "pep420_spec:",
            ns_spec.name,
            ns_spec.origin is None,
            ns_spec.has_location,
            len(list(ns_spec.submodule_search_locations)),
        )

        # Precedence check
        import prec3244

        print(
            "pep420_precedence:",
            prec3244.REGULAR,
            prec3244.__file__ == os.path.abspath(os.path.join(r2, "prec3244", "__init__.py")),
        )
    finally:
        sys.path.remove(r1)
        sys.path.remove(r2)
        for k in [
            "ns3244",
            "ns3244.alpha",
            "ns3244.beta",
            "ns3244.nested",
            "ns3244.nested.x",
            "ns3244.nested.y",
            "prec3244",
        ]:
            sys.modules.pop(k, None)
