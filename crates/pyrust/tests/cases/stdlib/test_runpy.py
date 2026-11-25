# CPython 3.12 parity tests for the `runpy` module (issue #3229).

import os
import pathlib
import runpy
import sys
import tempfile
import zipfile

# 1. Module metadata and __all__
print("in sys.modules:", "runpy" in sys.modules)
print("__all__:", runpy.__all__)
for name in runpy.__all__:
    assert hasattr(runpy, name), f"missing {name}"

with tempfile.TemporaryDirectory() as tmpdir:
    # Standalone module on sys.path
    mod_file = os.path.join(tmpdir, "sample_mod_3229.py")
    with open(mod_file, "w", encoding="utf-8") as f:
        f.write(
            '"""Sample module docstring."""\n'
            "import sys\n"
            "SEEN_NAME = __name__\n"
            "SEEN_PACKAGE = __package__\n"
            "SEEN_FILE = __file__\n"
            "SEEN_ARGV0 = sys.argv[0]\n"
            "SEEN_IN_SYS_MODS = __name__ in sys.modules\n"
            "SEEN_SYS_MOD_SAME = (\n"
            "    __name__ in sys.modules and sys.modules[__name__].__dict__ is globals()\n"
            ")\n"
            "SEED_PLUS_ONE = globals().get('SEED', 0) + 1\n"
            "def compute(x):\n"
            "    return x + SEED_PLUS_ONE\n"
        )

    # Module that raises an exception
    err_mod_file = os.path.join(tmpdir, "boom_mod_3229.py")
    with open(err_mod_file, "w", encoding="utf-8") as f:
        f.write("raise RuntimeError('kaboom_3229')\n")

    # Package with __main__.py and relative import
    pkg_dir = os.path.join(tmpdir, "run_pkg_3229")
    os.makedirs(pkg_dir)
    with open(os.path.join(pkg_dir, "__init__.py"), "w", encoding="utf-8") as f:
        f.write("PKG_INIT = 'loaded'\n")
    with open(os.path.join(pkg_dir, "helper.py"), "w", encoding="utf-8") as f:
        f.write("HELPER_VAL = 77\n")
    with open(os.path.join(pkg_dir, "__main__.py"), "w", encoding="utf-8") as f:
        f.write(
            "from . import helper\n"
            "MAIN_NAME = __name__\n"
            "MAIN_PKG = __package__\n"
            "MAIN_RESULT = helper.HELPER_VAL + globals().get('BONUS', 0)\n"
        )

    # Package without __main__.py
    empty_pkg_dir = os.path.join(tmpdir, "no_main_pkg_3229")
    os.makedirs(empty_pkg_dir)
    with open(os.path.join(empty_pkg_dir, "__init__.py"), "w", encoding="utf-8") as f:
        f.write("X = 1\n")

    # Directory with __main__.py + sibling module for run_path(dir)
    dir_app = os.path.join(tmpdir, "dir_app")
    os.makedirs(dir_app)
    with open(os.path.join(dir_app, "util_3229.py"), "w", encoding="utf-8") as f:
        f.write("TAG = 'from_dir_util'\n")
    with open(os.path.join(dir_app, "__main__.py"), "w", encoding="utf-8") as f:
        f.write(
            "import sys\n"
            "DIR_NAME = __name__\n"
            "DIR_PKG = __package__\n"
            "DIR_ARGV0 = sys.argv[0]\n"
            "import util_3229\n"
            "DIR_TAG = util_3229.TAG\n"
        )

    # Directory without __main__.py
    empty_dir = os.path.join(tmpdir, "empty_dir")
    os.makedirs(empty_dir)

    # Standalone script for run_path(file)
    script_file = os.path.join(tmpdir, "standalone_script.py")
    with open(script_file, "w", encoding="utf-8") as f:
        f.write(
            '"""Script doc."""\n'
            "import sys\n"
            "SCRIPT_NAME = __name__\n"
            "SCRIPT_PKG = __package__\n"
            "SCRIPT_FILE = __file__\n"
            "SCRIPT_ARGV0 = sys.argv[0]\n"
            "SCRIPT_IN_MODS = __name__ in sys.modules\n"
            "SCRIPT_VAL = globals().get('BASE', 10) * 3\n"
        )

    # Zip archive with __main__.py and sibling helper
    zip_app = os.path.join(tmpdir, "app_3229.zip")
    with zipfile.ZipFile(zip_app, "w") as zf:
        zf.writestr("zip_helper_3229.py", "ZIP_VAL = 123\n")
        zf.writestr(
            "__main__.py",
            "import importlib, sys\n"
            "ZIP_NAME = __name__\n"
            "ZIP_ARGV0 = sys.argv[0]\n"
            "zip_helper_3229 = importlib.import_module('zip_helper_3229')\n"
            "ZIP_OUT = zip_helper_3229.ZIP_VAL + globals().get('DELTA', 0)\n",
        )

    orig_argv0 = sys.argv[0]
    sys.path.insert(0, tmpdir)
    try:
        # 2. run_module with alter_sys=False (default)
        g1 = runpy.run_module("sample_mod_3229", init_globals={"SEED": 9})
        print(
            "run_module basic:",
            g1["SEEN_NAME"],
            repr(g1["SEEN_PACKAGE"]),
            g1["SEEN_FILE"] == os.path.abspath(mod_file),
            g1["SEEN_ARGV0"] == orig_argv0,
            g1["SEEN_IN_SYS_MODS"],
            g1["SEED_PLUS_ONE"],
            g1["compute"](5),
            g1["__doc__"],
            g1["__spec__"].name,
            g1["__loader__"] is not None,
            "sample_mod_3229" in sys.modules,
        )

        # Custom run_name overrides init_globals['__name__']
        g2 = runpy.run_module(
            "sample_mod_3229",
            init_globals={"__name__": "ignored", "SEED": 4},
            run_name="custom_run_3229",
        )
        print(
            "run_module run_name:",
            g2["SEEN_NAME"],
            g2["SEEN_IN_SYS_MODS"],
            g2["SEED_PLUS_ONE"],
        )

        # 3. run_module with alter_sys=True
        g3 = runpy.run_module(
            "sample_mod_3229",
            init_globals={"SEED": 2},
            run_name="temp_sys_mod_3229",
            alter_sys=True,
        )
        print(
            "run_module alter_sys:",
            g3["SEEN_NAME"],
            g3["SEEN_ARGV0"] == os.path.abspath(mod_file),
            g3["SEEN_IN_SYS_MODS"],
            g3["SEEN_SYS_MOD_SAME"],
            sys.argv[0] == orig_argv0,
            "temp_sys_mod_3229" in sys.modules,
        )

        # alter_sys=True restores sys.argv[0] and sys.modules even on exception
        try:
            runpy.run_module(
                "boom_mod_3229", run_name="boom_temp_3229", alter_sys=True
            )
        except RuntimeError as e:
            print(
                "run_module exc restore:",
                str(e),
                sys.argv[0] == orig_argv0,
                "boom_temp_3229" not in sys.modules,
            )

        # 4. run_module on package with __main__.py
        gp1 = runpy.run_module("run_pkg_3229", init_globals={"BONUS": 3})
        print(
            "run_module pkg default:",
            gp1["MAIN_NAME"],
            gp1["MAIN_PKG"],
            gp1["MAIN_RESULT"],
        )
        gp2 = runpy.run_module(
            "run_pkg_3229", init_globals={"BONUS": 23}, run_name="__main__"
        )
        print(
            "run_module pkg __main__:",
            gp2["MAIN_NAME"],
            gp2["MAIN_PKG"],
            gp2["MAIN_RESULT"],
        )

        # 5. run_module error cases
        for bad_mod in [
            ".relative",
            "missing_mod_3229",
            "no_main_pkg_3229",
            "sys",
        ]:
            try:
                runpy.run_module(bad_mod)
            except ImportError as e:
                print("run_module err:", bad_mod, type(e).__name__)

        # 6. run_path on standalone script (str and Path)
        rp1 = runpy.run_path(script_file, init_globals={"BASE": 7})
        print(
            "run_path script:",
            rp1["SCRIPT_NAME"],
            repr(rp1["SCRIPT_PKG"]),
            rp1["SCRIPT_FILE"] == script_file,
            rp1["SCRIPT_ARGV0"] == script_file,
            rp1["SCRIPT_IN_MODS"],
            rp1["SCRIPT_VAL"],
            rp1["__doc__"],
            sys.argv[0] == orig_argv0,
            "<run_path>" not in sys.modules,
        )

        rp2 = runpy.run_path(
            pathlib.Path(script_file),
            init_globals={"BASE": 5},
            run_name="my.custom.runner",
        )
        print(
            "run_path Path & run_name:",
            rp2["SCRIPT_NAME"],
            rp2["SCRIPT_PKG"],
            rp2["SCRIPT_VAL"],
            "my.custom.runner" not in sys.modules,
        )

        # 7. run_path on directory containing __main__.py
        rdir = runpy.run_path(dir_app, run_name="dir_runner")
        print(
            "run_path dir:",
            rdir["DIR_NAME"],
            repr(rdir["DIR_PKG"]),
            rdir["DIR_ARGV0"] == dir_app,
            rdir["DIR_TAG"],
            sys.argv[0] == orig_argv0,
            dir_app not in sys.path,
            "dir_runner" not in sys.modules,
        )

        # 8. run_path on zip archive containing __main__.py
        rzip = runpy.run_path(zip_app, init_globals={"DELTA": 7}, run_name="zip_runner")
        print(
            "run_path zip:",
            rzip["ZIP_NAME"],
            rzip["ZIP_ARGV0"] == zip_app,
            rzip["ZIP_OUT"],
            sys.argv[0] == orig_argv0,
            zip_app not in sys.path,
            "zip_runner" not in sys.modules,
        )

        # 9. run_path error cases
        try:
            runpy.run_path(os.path.join(tmpdir, "missing_script_3229.py"))
        except FileNotFoundError:
            print("run_path missing file: FileNotFoundError")

        try:
            runpy.run_path(empty_dir)
        except ImportError:
            print("run_path empty dir: ImportError")
    finally:
        sys.path.remove(tmpdir)
        for modname in [
            "run_pkg_3229",
            "run_pkg_3229.helper",
            "no_main_pkg_3229",
            "util_3229",
            "zip_helper_3229",
        ]:
            sys.modules.pop(modname, None)
        for p in [tmpdir, dir_app, empty_dir, zip_app]:
            sys.path_importer_cache.pop(p, None)
