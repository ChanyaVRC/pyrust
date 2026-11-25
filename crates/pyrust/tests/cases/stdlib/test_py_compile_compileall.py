import compileall
import enum
import importlib.util
import io
import marshal
import os
import py_compile
import re
import sys
import tempfile

# 1. PycInvalidationMode enum parity
print(
    "enum:",
    issubclass(py_compile.PycInvalidationMode, enum.Enum),
    py_compile.PycInvalidationMode.TIMESTAMP.value,
    py_compile.PycInvalidationMode.CHECKED_HASH.value,
    py_compile.PycInvalidationMode.UNCHECKED_HASH.value,
)
print(
    "exc_base:",
    issubclass(py_compile.PyCompileError, Exception),
)

with tempfile.TemporaryDirectory() as tmpdir:
    src_path = os.path.join(tmpdir, "hello.py")
    with open(src_path, "w", encoding="utf-8") as f:
        f.write("VALUE = 42\ndef greet(name):\n    return f'hi {name}'\n")

    # 2. py_compile.compile with default cfile (__pycache__)
    cfile = py_compile.compile(src_path, doraise=True)
    print(
        "default_cfile:",
        os.path.isfile(cfile),
        os.path.basename(os.path.dirname(cfile)),
        cfile == importlib.util.cache_from_source(src_path),
    )

    with open(cfile, "rb") as f:
        pyc_data = f.read()
    magic = pyc_data[:4]
    bitfield = int.from_bytes(pyc_data[4:8], "little")
    code_obj = marshal.loads(pyc_data[16:])
    print(
        "pyc_timestamp:",
        magic == importlib.util.MAGIC_NUMBER,
        bitfield,
        code_obj.co_name,
        "VALUE" in code_obj.co_names,
        "greet" in code_obj.co_names,
    )

    # 3. CHECKED_HASH and UNCHECKED_HASH invalidation modes
    c_checked = os.path.join(tmpdir, "checked.pyc")
    py_compile.compile(
        src_path,
        cfile=c_checked,
        doraise=True,
        invalidation_mode=py_compile.PycInvalidationMode.CHECKED_HASH,
    )
    with open(c_checked, "rb") as f:
        d_checked = f.read()
    c_unchecked = os.path.join(tmpdir, "unchecked.pyc")
    py_compile.compile(
        src_path,
        cfile=c_unchecked,
        doraise=True,
        invalidation_mode=py_compile.PycInvalidationMode.UNCHECKED_HASH,
    )
    with open(c_unchecked, "rb") as f:
        d_unchecked = f.read()
    print(
        "pyc_hash_modes:",
        int.from_bytes(d_checked[4:8], "little"),
        int.from_bytes(d_unchecked[4:8], "little"),
        d_checked[8:16] == d_unchecked[8:16],
        len(d_checked[8:16]),
    )

    # 4. Syntax error handling: doraise=True vs doraise=False
    bad_path = os.path.join(tmpdir, "broken.py")
    with open(bad_path, "w", encoding="utf-8") as f:
        f.write("def oops(:\n    pass\n")
    try:
        py_compile.compile(bad_path, doraise=True)
    except py_compile.PyCompileError as exc:
        print(
            "doraise_err:",
            exc.exc_type_name,
            exc.file == bad_path,
            isinstance(exc.msg, str) and len(exc.msg) > 0,
        )

    res_quiet = py_compile.compile(bad_path, doraise=False, quiet=2)
    print("doraise_false:", res_quiet is None)

    # 5. compileall.compile_file and compileall.compile_dir
    pkg_dir = os.path.join(tmpdir, "pkg")
    sub_dir = os.path.join(pkg_dir, "sub")
    os.makedirs(sub_dir)
    a_py = os.path.join(pkg_dir, "a.py")
    b_py = os.path.join(sub_dir, "b.py")
    skip_py = os.path.join(pkg_dir, "ignore_me.py")
    with open(a_py, "w", encoding="utf-8") as f:
        f.write("A = 1\n")
    with open(b_py, "w", encoding="utf-8") as f:
        f.write("B = 2\n")
    with open(skip_py, "w", encoding="utf-8") as f:
        f.write("def broken_ignored(:\n")

    ok_dir = compileall.compile_dir(
        pkg_dir,
        rx=re.compile(r"ignore_me\.py$"),
        quiet=2,
        force=True,
    )
    print(
        "compile_dir:",
        ok_dir,
        os.path.isfile(importlib.util.cache_from_source(a_py)),
        os.path.isfile(importlib.util.cache_from_source(b_py)),
        os.path.isfile(importlib.util.cache_from_source(skip_py)),
    )

    # Legacy mode (.pyc next to .py)
    ok_legacy = compileall.compile_file(a_py, legacy=True, quiet=2, force=True)
    print("compile_file_legacy:", ok_legacy, os.path.isfile(a_py + "c"))

    # Failure returns False
    ok_bad = compileall.compile_file(bad_path, quiet=2, force=True)
    print("compile_file_bad:", ok_bad)

    # compile_path on a temporary sys.path
    orig_path = list(sys.path)
    try:
        sys.path[:] = [pkg_dir]
        # Remove skip_py so compile_path succeeds cleanly
        os.remove(skip_py)
        ok_path = compileall.compile_path(skip_curdir=1, maxlevels=1, quiet=2, force=True)
        print("compile_path:", ok_path)
    finally:
        sys.path[:] = orig_path
