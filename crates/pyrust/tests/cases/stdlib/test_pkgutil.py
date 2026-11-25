# CPython 3.12 parity tests for the `pkgutil` module (issue #3229).

import importlib.machinery
import os
import pathlib
import pkgutil
import sys
import tempfile
import warnings
import zipfile

# 1. Module metadata, __all__, and ModuleInfo
print("in sys.modules:", "pkgutil" in sys.modules)
core_all = [
    "get_importer",
    "iter_importers",
    "walk_packages",
    "iter_modules",
    "get_data",
    "read_code",
    "extend_path",
    "ModuleInfo",
]
print("core __all__ present:", all(n in pkgutil.__all__ for n in core_all))
for name in pkgutil.__all__:
    assert hasattr(pkgutil, name), f"missing {name}"
assert hasattr(pkgutil, "resolve_name")

mi = pkgutil.ModuleInfo(None, "alpha.beta", True)
print(
    "ModuleInfo:",
    mi,
    mi._fields,
    mi.module_finder,
    mi.name,
    mi.ispkg,
    tuple(mi),
    isinstance(mi, tuple),
    pkgutil.ModuleInfo.__module__,
)

with tempfile.TemporaryDirectory() as tmpdir:
    # Build a multi-root package tree and a zip archive
    root1 = os.path.join(tmpdir, "root1")
    root2 = os.path.join(tmpdir, "root2")
    os.makedirs(root1)
    os.makedirs(root2)

    # root1/mod_one.py
    with open(os.path.join(root1, "mod_one.py"), "w", encoding="utf-8") as f:
        f.write("VALUE = 1\n")

    # root1/not_a_pkg/ignored.py (no __init__.py)
    os.makedirs(os.path.join(root1, "not_a_pkg"))
    with open(
        os.path.join(root1, "not_a_pkg", "ignored.py"), "w", encoding="utf-8"
    ) as f:
        f.write("X = 0\n")

    # root1/notes.txt (not a module)
    with open(os.path.join(root1, "notes.txt"), "w", encoding="utf-8") as f:
        f.write("hello\n")

    # root1/pkg_alpha/ with subpackage and binary resources
    pkg_alpha = os.path.join(root1, "pkg_alpha")
    pkg_sub = os.path.join(pkg_alpha, "sub")
    pkg_res_dir = os.path.join(pkg_alpha, "resources")
    os.makedirs(pkg_sub)
    os.makedirs(pkg_res_dir)

    with open(os.path.join(pkg_alpha, "__init__.py"), "w", encoding="utf-8") as f:
        f.write("PKG_TAG = 'alpha'\n")
    with open(os.path.join(pkg_alpha, "helper.py"), "w", encoding="utf-8") as f:
        f.write(
            "class Box:\n"
            "    class Inner:\n"
            "        ANSWER = 42\n"
            "def double(n):\n"
            "    return n * 2\n"
        )
    with open(os.path.join(pkg_sub, "__init__.py"), "w", encoding="utf-8") as f:
        f.write("SUB_TAG = 'sub'\n")
    with open(os.path.join(pkg_sub, "leaf.py"), "w", encoding="utf-8") as f:
        f.write("LEAF = 'ok'\n")
    with open(os.path.join(pkg_alpha, "data.bin"), "wb") as f:
        f.write(b"\x00\x01hello\xff")
    with open(os.path.join(pkg_res_dir, "nested.txt"), "wb") as f:
        f.write(b"nested-payload\n")

    # Broken packages for walk_packages onerror tests
    broken_imp = os.path.join(root1, "pkg_broken_imp")
    os.makedirs(broken_imp)
    with open(os.path.join(broken_imp, "__init__.py"), "w", encoding="utf-8") as f:
        f.write("raise ImportError('deliberate import error')\n")

    broken_rt = os.path.join(root2, "pkg_broken_rt")
    os.makedirs(broken_rt)
    with open(os.path.join(broken_rt, "__init__.py"), "w", encoding="utf-8") as f:
        f.write("raise RuntimeError('deliberate runtime error')\n")

    # root2/mod_one.py (duplicate name) and root2/mod_two.py
    with open(os.path.join(root2, "mod_one.py"), "w", encoding="utf-8") as f:
        f.write("VALUE = 2\n")
    with open(os.path.join(root2, "mod_two.py"), "w", encoding="utf-8") as f:
        f.write("VALUE = 20\n")

    # Extend-path directories and .pkg file
    ext_a = os.path.join(tmpdir, "ext_a")
    ext_b = os.path.join(tmpdir, "ext_b")
    os.makedirs(os.path.join(ext_a, "ns_demo"))
    os.makedirs(os.path.join(ext_b, "ns_demo"))
    with open(
        os.path.join(ext_a, "ns_demo", "__init__.py"), "w", encoding="utf-8"
    ) as f:
        f.write("# init\n")
    with open(os.path.join(ext_b, "ns_demo.pkg"), "w", encoding="utf-8") as f:
        f.write("# comment line\n\n/virtual/extra/path\n")

    # Zip archive for get_importer
    zip_path = os.path.join(tmpdir, "bundle.zip")
    with zipfile.ZipFile(zip_path, "w") as zf:
        zf.writestr("zipmod.py", "Z = 99\n")

    # 2. get_importer
    imp1 = pkgutil.get_importer(root1)
    imp1_path = pkgutil.get_importer(pathlib.Path(root1))
    imp_missing = pkgutil.get_importer(os.path.join(tmpdir, "no_such_dir"))
    imp_file = pkgutil.get_importer(os.path.join(root1, "mod_one.py"))
    imp_zip = pkgutil.get_importer(zip_path)
    print(
        "get_importer:",
        isinstance(imp1, importlib.machinery.FileFinder),
        imp1 is imp1_path,
        imp_missing is None,
        imp_file is None,
        imp_zip is not None,
    )

    # Custom path_hooks
    hook_token = os.path.join(tmpdir, "custom_hook_target_3229")

    class CustomHookFinder:
        def iter_modules(self, prefix=""):
            yield prefix + "hooked_mod", False

    def my_hook(path):
        if path == hook_token:
            return CustomHookFinder()
        raise ImportError()

    sys.path_hooks.insert(0, my_hook)
    try:
        imp_custom = pkgutil.get_importer(hook_token)
        print(
            "custom hook:",
            isinstance(imp_custom, CustomHookFinder),
            [(m.name, m.ispkg) for m in pkgutil.iter_modules([hook_token], "pre.")],
        )
    finally:
        sys.path_hooks.remove(my_hook)
        sys.path_importer_cache.pop(hook_token, None)

    # 3. iter_modules
    mods_r1 = [(m.name, m.ispkg) for m in pkgutil.iter_modules([root1])]
    print("iter_modules root1:", mods_r1)
    mods_pref = [(m.name, m.ispkg) for m in pkgutil.iter_modules([root1], "p.")]
    print("iter_modules prefix:", mods_pref)
    mods_dedup = [(m.name, m.ispkg) for m in pkgutil.iter_modules([root1, root2])]
    print("iter_modules dedup:", mods_dedup)
    try:
        list(pkgutil.iter_modules(root1))
    except ValueError as e:
        print("iter_modules str err:", type(e).__name__, str(e))

    # 4. walk_packages, iter_importers, get_data, resolve_name, extend_path
    sys.path.insert(0, root1)
    try:
        # walk_packages without onerror ignores ImportError in pkg_broken_imp
        walked = [(m.name, m.ispkg) for m in pkgutil.walk_packages([root1])]
        print("walk_packages root1:", walked)

        # walk_packages with onerror records broken package name
        errors_seen = []
        walked_err = [
            (m.name, m.ispkg)
            for m in pkgutil.walk_packages([root1], onerror=errors_seen.append)
        ]
        print("walk_packages onerror ImportError:", errors_seen, len(walked_err))

        # walk_packages with RuntimeError propagates when onerror is None, caught when supplied
        sys.path.insert(0, root2)
        try:
            try:
                list(pkgutil.walk_packages([root2]))
            except RuntimeError as e:
                print("walk_packages RuntimeError propagated:", str(e))

            rt_errors = []
            walked_r2 = [
                (m.name, m.ispkg)
                for m in pkgutil.walk_packages([root2], onerror=rt_errors.append)
            ]
            print("walk_packages RuntimeError caught:", rt_errors, walked_r2)
        finally:
            sys.path.remove(root2)
            sys.modules.pop("pkg_broken_rt", None)

        # iter_importers
        try:
            list(pkgutil.iter_importers(".relative"))
        except ImportError as e:
            print("iter_importers relative:", type(e).__name__, str(e))

        pkg_imps = list(pkgutil.iter_importers("pkg_alpha.helper"))
        print("iter_importers dotted count:", len(pkg_imps), pkg_imps[0] is not None)
        non_pkg_imps = list(pkgutil.iter_importers("math.sin"))
        print("iter_importers non-pkg:", non_pkg_imps)

        # get_data
        raw1 = pkgutil.get_data("pkg_alpha", "data.bin")
        raw2 = pkgutil.get_data("pkg_alpha", "resources/nested.txt")
        print("get_data:", raw1, raw2)
        print(
            "get_data none:",
            pkgutil.get_data("nonexistent_pkg_3229", "x.txt"),
            pkgutil.get_data("sys", "x.txt"),
        )
        try:
            pkgutil.get_data("pkg_alpha", "missing.bin")
        except OSError as e:
            print("get_data missing:", type(e).__name__)

        # resolve_name
        print(
            "resolve_name colon:",
            pkgutil.resolve_name("pkg_alpha.helper:double")(21),
            pkgutil.resolve_name("pkg_alpha.helper:Box.Inner.ANSWER"),
            pkgutil.resolve_name("pkg_alpha.helper:").__name__,
            pkgutil.resolve_name("math:sqrt")(9),
            pkgutil.resolve_name("math:").__name__,
        )
        print(
            "resolve_name dotted:",
            pkgutil.resolve_name("pkg_alpha.helper.double")(15),
            pkgutil.resolve_name("pkg_alpha.helper.Box.Inner.ANSWER"),
            pkgutil.resolve_name("pkg_alpha.sub.leaf").LEAF,
            pkgutil.resolve_name("math.sqrt")(16),
        )
        for bad in ["", ".a", "a.", "a..b", "123", "a:b:c", "a:.b", "a:123"]:
            try:
                pkgutil.resolve_name(bad)
            except ValueError as e:
                print("resolve_name bad:", repr(bad), type(e).__name__)
        try:
            pkgutil.resolve_name("pkg_alpha.helper:no_such_attr")
        except AttributeError:
            print("resolve_name missing attr: AttributeError")
        try:
            pkgutil.resolve_name("no_such_mod_3229:attr")
        except ImportError:
            print("resolve_name missing mod: ImportError")

        # get_loader / find_loader (present in Python 3.12, removed in 3.14)
        if hasattr(pkgutil, "find_loader") and hasattr(pkgutil, "get_loader"):
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", DeprecationWarning)
                assert pkgutil.find_loader("pkg_alpha.helper") is not None
                assert pkgutil.get_loader("pkg_alpha.helper") is not None

        # extend_path
        print("extend_path non-list:", pkgutil.extend_path(("a",), "ns_demo"))
        sys.path.insert(0, ext_b)
        sys.path.insert(0, ext_a)
        try:
            orig = [os.path.join(ext_a, "ns_demo")]
            extended = pkgutil.extend_path(orig, "ns_demo")
            print(
                "extend_path:",
                len(orig),
                len(extended),
                os.path.join(ext_a, "ns_demo") in extended,
                os.path.join(ext_b, "ns_demo") in extended,
                "/virtual/extra/path" in extended,
            )
        finally:
            sys.path.remove(ext_a)
            sys.path.remove(ext_b)
    finally:
        sys.path.remove(root1)
        for modname in [
            "pkg_alpha",
            "pkg_alpha.helper",
            "pkg_alpha.sub",
            "pkg_alpha.sub.leaf",
            "pkg_broken_imp",
        ]:
            sys.modules.pop(modname, None)
        for p in [root1, root2, ext_a, ext_b, zip_path]:
            sys.path_importer_cache.pop(p, None)
