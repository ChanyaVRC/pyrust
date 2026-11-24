# Comprehensive CPython 3.12 parity tests for `shutil`.

import io
import os
import pathlib
import shutil
import tempfile

# --- Exception hierarchy ---
print("err-issub-oserror", issubclass(shutil.Error, OSError))
print("samefile-issub-error", issubclass(shutil.SameFileError, shutil.Error))
print("special-issub-oserror", issubclass(shutil.SpecialFileError, OSError))
print("exec-issub-oserror", issubclass(shutil.ExecError, OSError))
print("read-issub-oserror", issubclass(shutil.ReadError, OSError))

# --- copyfileobj ---
src_bio = io.BytesIO(b"hello world " * 100)
dst_bio = io.BytesIO()
shutil.copyfileobj(src_bio, dst_bio, length=17)
print("copyfileobj-bytes", dst_bio.getvalue() == b"hello world " * 100)

src_sio = io.StringIO("line1\nline2\nline3\n")
dst_sio = io.StringIO()
shutil.copyfileobj(src_sio, dst_sio)
print("copyfileobj-str", dst_sio.getvalue())

# --- get_terminal_size ---
saved_cols = os.environ.get("COLUMNS")
saved_lines = os.environ.get("LINES")
try:
    if "COLUMNS" in os.environ:
        del os.environ["COLUMNS"]
    if "LINES" in os.environ:
        del os.environ["LINES"]
    ts_default = shutil.get_terminal_size()
    print("ts-type", isinstance(ts_default, os.terminal_size))
    print("ts-len", len(ts_default))
    print("ts-fallback", shutil.get_terminal_size(fallback=(123, 45)))
    os.environ["COLUMNS"] = "132"
    os.environ["LINES"] = "50"
    ts_env = shutil.get_terminal_size(fallback=(10, 10))
    print("ts-env", ts_env, ts_env.columns, ts_env.lines)
    os.environ["COLUMNS"] = "notanint"
    os.environ["LINES"] = "-5"
    ts_bad = shutil.get_terminal_size(fallback=(77, 33))
    print("ts-bad-env", ts_bad)
finally:
    if saved_cols is None:
        if "COLUMNS" in os.environ:
            del os.environ["COLUMNS"]
    else:
        os.environ["COLUMNS"] = saved_cols
    if saved_lines is None:
        if "LINES" in os.environ:
            del os.environ["LINES"]
    else:
        os.environ["LINES"] = saved_lines


def _walk_rel(base):
    out = []
    for dirpath, dirs, files in os.walk(base):
        rel = os.path.relpath(dirpath, base).replace("\\", "/")
        out.append((rel, sorted(dirs), sorted(files)))
    out.sort()
    return out


# --- Filesystem operations inside an isolated temp directory ---
with tempfile.TemporaryDirectory() as root:
    # 1. copyfile, copy, copy2, copymode, copystat
    src = os.path.join(root, "src.txt")
    with open(src, "wb") as f:
        f.write(b"binary\x00content\nline2\n")

    dst1 = os.path.join(root, "dst1.txt")
    ret1 = shutil.copyfile(src, dst1)
    print("copyfile-ret", ret1 == dst1)
    with open(dst1, "rb") as f:
        print("copyfile-data", f.read() == b"binary\x00content\nline2\n")

    # SameFileError
    try:
        shutil.copyfile(src, src)
        print("samefile-err", "NO-ERROR")
    except shutil.SameFileError:
        print("samefile-err", "SameFileError")

    # copy to a directory joins basename
    dest_dir = os.path.join(root, "dest_dir")
    os.mkdir(dest_dir)
    ret_copy = shutil.copy(src, dest_dir)
    print(
        "copy-into-dir",
        os.path.basename(ret_copy),
        os.path.isfile(os.path.join(dest_dir, "src.txt")),
    )

    # copy2 with PathLike
    dst2 = pathlib.Path(root) / "dst2.txt"
    ret2 = shutil.copy2(pathlib.Path(src), dst2)
    print("copy2-path-ret", ret2 == dst2, os.path.isfile(os.fspath(dst2)))
    with open(os.fspath(dst2), "rb") as f:
        print("copy2-data", f.read() == b"binary\x00content\nline2\n")

    # copymode on POSIX checks exact permission bits
    if os.name == "posix":
        exec_src = os.path.join(root, "exec_src")
        exec_dst = os.path.join(root, "exec_dst")
        with open(exec_src, "w") as f:
            f.write("a")
        with open(exec_dst, "w") as f:
            f.write("b")
        os.chmod(exec_src, 0o751)
        shutil.copymode(exec_src, exec_dst)
        print("copymode-posix", oct(os.stat(exec_dst).st_mode & 0o777))
        shutil.copystat(exec_src, exec_dst)
        print("copystat-posix", oct(os.stat(exec_dst).st_mode & 0o777))

    # 2. copytree + ignore_patterns + dirs_exist_ok
    tree_src = os.path.join(root, "tree_src")
    os.makedirs(os.path.join(tree_src, "sub", "nested"))
    with open(os.path.join(tree_src, "keep.py"), "w") as f:
        f.write("keep")
    with open(os.path.join(tree_src, "skip.tmp"), "w") as f:
        f.write("skip")
    with open(os.path.join(tree_src, "sub", "data.txt"), "w") as f:
        f.write("data")
    with open(os.path.join(tree_src, "sub", "nested", "notes.log"), "w") as f:
        f.write("log")

    tree_dst = os.path.join(root, "tree_dst")
    ret_tree = shutil.copytree(
        tree_src,
        tree_dst,
        ignore=shutil.ignore_patterns("*.tmp", "*.log"),
    )
    print("copytree-ret", ret_tree == tree_dst)
    print("copytree-walk", _walk_rel(tree_dst))

    # copytree into existing dir fails by default
    try:
        shutil.copytree(tree_src, tree_dst)
        print("copytree-exists", "NO-ERROR")
    except FileExistsError:
        print("copytree-exists", "FileExistsError")

    # copytree with dirs_exist_ok=True succeeds and merges
    shutil.copytree(tree_src, tree_dst, dirs_exist_ok=True)
    print("copytree-merge", _walk_rel(tree_dst))

    # Custom copy_function
    copied_names = []

    def custom_copy(s, d):
        copied_names.append(os.path.basename(s))
        return shutil.copy2(s, d)

    tree_custom = os.path.join(root, "tree_custom")
    shutil.copytree(
        tree_src,
        tree_custom,
        ignore=shutil.ignore_patterns("*.tmp"),
        copy_function=custom_copy,
    )
    print("copytree-custom-fn", sorted(copied_names))

    # 3. rmtree (basic, ignore_errors, onexc, onerror)
    shutil.rmtree(tree_custom)
    print("rmtree-gone", not os.path.exists(tree_custom))

    missing_dir = os.path.join(root, "no_such_dir")
    try:
        shutil.rmtree(missing_dir)
        print("rmtree-missing", "NO-ERROR")
    except FileNotFoundError:
        print("rmtree-missing", "FileNotFoundError")

    shutil.rmtree(missing_dir, ignore_errors=True)
    print("rmtree-ignore-ok", True)

    seen_onexc = []

    def handle_onexc(func, path, exc):
        seen_onexc.append((path == missing_dir, isinstance(exc, FileNotFoundError)))

    shutil.rmtree(missing_dir, onexc=handle_onexc)
    print("rmtree-onexc", seen_onexc)

    seen_onerror = []

    def handle_onerror(func, path, exc_info):
        seen_onerror.append(
            (path == missing_dir, exc_info[0] is FileNotFoundError)
        )

    shutil.rmtree(missing_dir, onerror=handle_onerror)
    print("rmtree-onerror", seen_onerror)

    # When both onerror and onexc are passed, onexc wins
    both_log = []
    shutil.rmtree(
        missing_dir,
        onerror=lambda f, p, ei: both_log.append("onerror"),
        onexc=lambda f, p, e: both_log.append("onexc"),
    )
    print("rmtree-precedence", both_log)

    # 4. move (file rename, file into dir, dir rename, dir into dir, dest exists)
    mv_file = os.path.join(root, "mv_me.txt")
    with open(mv_file, "w") as f:
        f.write("moved")
    mv_target = os.path.join(root, "mv_renamed.txt")
    ret_mv = shutil.move(mv_file, mv_target)
    print(
        "move-file",
        ret_mv == mv_target,
        not os.path.exists(mv_file),
        os.path.isfile(mv_target),
    )

    mv_dir = os.path.join(root, "mv_dest_dir")
    os.mkdir(mv_dir)
    ret_mv2 = shutil.move(mv_target, mv_dir)
    print(
        "move-into-dir",
        os.path.basename(ret_mv2),
        not os.path.exists(mv_target),
        os.path.isfile(os.path.join(mv_dir, "mv_renamed.txt")),
    )

    # Moving a file into a dir that already has that name raises shutil.Error
    dup = os.path.join(root, "mv_renamed.txt")
    with open(dup, "w") as f:
        f.write("dup")
    try:
        shutil.move(dup, mv_dir)
        print("move-dup", "NO-ERROR")
    except shutil.Error:
        print("move-dup", "shutil.Error")

    # Moving a directory
    dir_a = os.path.join(root, "dir_a")
    os.mkdir(dir_a)
    with open(os.path.join(dir_a, "inside.txt"), "w") as f:
        f.write("hi")
    dir_b = os.path.join(root, "dir_b")
    shutil.move(dir_a, dir_b)
    print(
        "move-dir",
        not os.path.exists(dir_a),
        os.path.isfile(os.path.join(dir_b, "inside.txt")),
    )

    # 5. which
    bin_dir = os.path.join(root, "mybin")
    os.mkdir(bin_dir)
    if os.name == "nt":
        tool_path = os.path.join(bin_dir, "mytool.EXE")
        with open(tool_path, "w") as f:
            f.write("fake exe")
        found = shutil.which("mytool", path=bin_dir)
        print("which-found", found is not None and os.path.normcase(found) == os.path.normcase(tool_path))
        found_ext = shutil.which("mytool.EXE", path=bin_dir)
        print("which-found-ext", found_ext is not None and os.path.normcase(found_ext) == os.path.normcase(tool_path))
    else:
        tool_path = os.path.join(bin_dir, "mytool")
        with open(tool_path, "w") as f:
            f.write("#!/bin/sh\n")
        os.chmod(tool_path, 0o644)
        print("which-nonexec", shutil.which("mytool", path=bin_dir))
        os.chmod(tool_path, 0o755)
        found = shutil.which("mytool", path=bin_dir)
        print("which-found", found == tool_path)

    # Directories matching name must NOT be returned by which
    os.mkdir(os.path.join(bin_dir, "not_a_cmd"))
    print("which-dir-ignored", shutil.which("not_a_cmd", mode=os.F_OK, path=bin_dir))
    print("which-empty-path", shutil.which("mytool", path=""))
    print("which-missing", shutil.which("definitely_no_such_cmd_3148", path=bin_dir))

    # Direct path lookup (with directory component)
    direct = os.path.join(bin_dir, "direct_file")
    with open(direct, "w") as f:
        f.write("x")
    print(
        "which-direct-fok",
        shutil.which(direct, mode=os.F_OK) == direct,
    )

    # 6. Symlink handling (copyfile follow_symlinks=False, copytree symlinks=True, rmtree guard)
    sym_target = os.path.join(root, "sym_target.txt")
    with open(sym_target, "w") as f:
        f.write("target-data")
    sym_link = os.path.join(root, "sym_link.txt")
    can_symlink = True
    try:
        os.symlink(sym_target, sym_link)
    except OSError:
        can_symlink = False

    if can_symlink:
        sym_copy = os.path.join(root, "sym_copy.txt")
        shutil.copyfile(sym_link, sym_copy, follow_symlinks=False)
        print("sym-copyfile-islink", os.path.islink(sym_copy))
        print("sym-copyfile-readlink", os.readlink(sym_copy) == sym_target)

        sym_deref = os.path.join(root, "sym_deref.txt")
        shutil.copyfile(sym_link, sym_deref, follow_symlinks=True)
        print("sym-deref-not-link", not os.path.islink(sym_deref))
        with open(sym_deref, "r") as f:
            print("sym-deref-content", f.read())

        # rmtree refuses symlink to directory
        sym_dir_real = os.path.join(root, "sym_dir_real")
        os.mkdir(sym_dir_real)
        sym_dir_link = os.path.join(root, "sym_dir_link")
        os.symlink(sym_dir_real, sym_dir_link)
        try:
            shutil.rmtree(sym_dir_link)
            print("sym-rmtree", "NO-ERROR")
        except OSError:
            print("sym-rmtree", "OSError")
        print("sym-rmtree-target-safe", os.path.isdir(sym_dir_real))
        os.unlink(sym_dir_link)

        # copytree with symlinks=True preserves symlink
        st_src = os.path.join(root, "st_src")
        os.mkdir(st_src)
        os.symlink(sym_target, os.path.join(st_src, "ln.txt"))
        st_dst = os.path.join(root, "st_dst")
        shutil.copytree(st_src, st_dst, symlinks=True)
        print("sym-copytree-islink", os.path.islink(os.path.join(st_dst, "ln.txt")))
    else:
        print("symlink-tests-skipped", True)

print("cleaned-up", not os.path.exists(root))
