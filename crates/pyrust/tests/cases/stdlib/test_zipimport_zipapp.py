import io
import os
import sys
import tempfile
import zipapp
import zipfile
import zipimport

# 1. Exception hierarchy & sys.path_hooks registration
print(
    "hierarchy:",
    issubclass(zipimport.ZipImportError, ImportError),
    issubclass(zipapp.ZipAppError, ValueError),
    zipimport.zipimporter in sys.path_hooks,
)

with tempfile.TemporaryDirectory() as tmpdir:
    # 2. Build a ZIP archive containing a top-level module, a package with a
    # submodule, and a data resource, using both STORED and DEFLATED entries.
    zip_path = os.path.join(tmpdir, "bundle3249.zip")
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("zipmod3249.py", "ANSWER = 42\ndef double(x):\n    return x * 2\n")
        zf.writestr("pkg_in_zip3249/__init__.py", "PKG_TAG = 'zip-root'\n")
        zf.writestr(
            "pkg_in_zip3249/mod.py",
            "from . import PKG_TAG\nLABEL = PKG_TAG + ':mod'\n",
        )
        zf.writestr("pkg_in_zip3249/data.txt", b"hello-from-zip")

    # 3. Direct zipimporter API
    zi = zipimport.zipimporter(zip_path)
    print(
        "zi_attrs:",
        zi.archive == zip_path,
        zi.prefix == "",
        zi.is_package("zipmod3249"),
        zi.is_package("pkg_in_zip3249"),
    )
    print(
        "zi_source:",
        "ANSWER = 42" in zi.get_source("zipmod3249"),
        zi.get_filename("zipmod3249").endswith("zipmod3249.py"),
        zi.get_data(os.path.join(zip_path, "pkg_in_zip3249", "data.txt")),
    )
    code_obj = zi.get_code("zipmod3249")
    print("zi_code:", code_obj.co_name, "double" in code_obj.co_names)

    # Non-existent zip -> ZipImportError
    try:
        zipimport.zipimporter(os.path.join(tmpdir, "nope.zip"))
    except zipimport.ZipImportError:
        print("missing_zip_err: ok")

    # 4. Transparent import directly from .zip on sys.path!
    sys.path.insert(0, zip_path)
    try:
        import zipmod3249
        import pkg_in_zip3249
        from pkg_in_zip3249 import mod as zip_submod

        print(
            "zip_import:",
            zipmod3249.ANSWER,
            zipmod3249.double(21),
            pkg_in_zip3249.PKG_TAG,
            zip_submod.LABEL,
            zip_submod.__package__,
            isinstance(sys.path_importer_cache.get(zip_path), zipimport.zipimporter),
        )
    finally:
        sys.path.remove(zip_path)
        sys.path_importer_cache.pop(zip_path, None)
        for k in ["zipmod3249", "pkg_in_zip3249", "pkg_in_zip3249.mod"]:
            sys.modules.pop(k, None)

    # 5. zipapp.create_archive & get_interpreter
    app_src = os.path.join(tmpdir, "myapp")
    os.makedirs(app_src)
    with open(os.path.join(app_src, "runner.py"), "w", encoding="utf-8") as f:
        f.write("def run():\n    return 'ran'\n")
    with open(os.path.join(app_src, "skip.tmp"), "w", encoding="utf-8") as f:
        f.write("ignore\n")

    pyz_path = os.path.join(tmpdir, "myapp.pyz")
    zipapp.create_archive(
        app_src,
        target=pyz_path,
        interpreter="/usr/bin/env python3",
        main="runner:run",
        filter=lambda p: p.suffix != ".tmp",
        compressed=True,
    )
    print(
        "zipapp_interp:",
        zipapp.get_interpreter(pyz_path),
        os.path.isfile(pyz_path),
    )
    with zipfile.ZipFile(pyz_path, "r") as zf:
        names = sorted(zf.namelist())
        main_src = zf.read("__main__.py").decode("utf-8")
        print("zipapp_contents:", names, "runner.run()" in main_src)

    # Also verify a shebang-prefixed .pyz is importable via zipimporter!
    pyz_imp = zipimport.zipimporter(pyz_path)
    print("pyz_zipimporter:", "def run():" in pyz_imp.get_source("runner"))

    # Update interpreter by copying archive without shebang
    pyz_plain = os.path.join(tmpdir, "myapp_plain.pyz")
    zipapp.create_archive(pyz_path, target=pyz_plain, interpreter=None)
    print("zipapp_no_interp:", zipapp.get_interpreter(pyz_plain) is None)

    # Error when directory has no __main__.py and main=None
    try:
        zipapp.create_archive(app_src, target=os.path.join(tmpdir, "err.pyz"))
    except zipapp.ZipAppError:
        print("zipapp_no_main_err: ok")

    # Binary stream target
    buf = io.BytesIO()
    zipapp.create_archive(app_src, target=buf, interpreter="/usr/bin/python3", main="runner:run")
    buf.seek(0)
    print("zipapp_stream_interp:", zipapp.get_interpreter(buf))
