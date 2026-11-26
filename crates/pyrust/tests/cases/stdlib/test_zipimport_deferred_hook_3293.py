# Issue #3293: pyrust no longer imports `zipimport` at startup.  The hook must
# still claim an archive that appears on `sys.path`, with no explicit
# `import zipimport` and no read of `sys.path_hooks` beforehand — the script
# below never names either until the very end.

import os
import sys
import tempfile
import zipfile

with tempfile.TemporaryDirectory() as tmpdir:
    archive = os.path.join(tmpdir, "bundle_3293.zip")
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("zipped_mod_3293.py", "VALUE = 41 + 1\n")
        zf.writestr("zipped_pkg_3293/__init__.py", "from .inner import NAME\n")
        zf.writestr("zipped_pkg_3293/inner.py", "NAME = 'inner'\n")
        zf.writestr("nested_3293/deep_mod_3293.py", "DEPTH = 'nested'\n")

    # A directory and a missing entry are never claimed by the zip hook.
    plain_dir = os.path.join(tmpdir, "plain_3293")
    os.mkdir(plain_dir)
    with open(os.path.join(plain_dir, "plain_mod_3293.py"), "w") as fh:
        fh.write("KIND = 'directory'\n")
    missing = os.path.join(tmpdir, "missing_3293")
    sys.path.insert(0, missing)
    sys.path.insert(0, plain_dir)
    import plain_mod_3293

    print(plain_mod_3293.KIND)
    try:
        import does_not_exist_3293
    except ImportError as exc:
        print(type(exc).__name__)
    print(sys.path_importer_cache.get(missing, "absent"))

    # The archive itself, and a directory inside the archive.
    sys.path.insert(0, archive)
    sys.path.insert(0, os.path.join(archive, "nested_3293"))
    import zipped_mod_3293
    import zipped_pkg_3293
    import deep_mod_3293

    print(zipped_mod_3293.VALUE, zipped_pkg_3293.NAME, deep_mod_3293.DEPTH)
    print(type(sys.path_importer_cache[archive]).__name__)
    print(type(sys.path_importer_cache[archive]).__module__)

    import zipimport

    print(isinstance(sys.path_importer_cache[archive], zipimport.zipimporter))
    print(sys.path_hooks[0] is zipimport.zipimporter, sys.path_hooks.count(zipimport.zipimporter))

    for entry in (archive, os.path.join(archive, "nested_3293"), plain_dir, missing):
        sys.path.remove(entry)
    sys.path_importer_cache.clear()
