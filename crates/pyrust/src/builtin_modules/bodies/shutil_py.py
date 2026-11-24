"""Utility functions for copying and archiving files and directory trees."""

import fnmatch
import os
import sys

_WINDOWS = os.name == "nt"

COPY_BUFSIZE = 1024 * 1024 if _WINDOWS else 64 * 1024

# CMD defaults in Windows 10/11
_WIN_DEFAULT_PATHEXT = ".COM;.EXE;.BAT;.CMD;.VBS;.JS;.WS;.MSC"

__all__ = [
    "copyfileobj",
    "copyfile",
    "copymode",
    "copystat",
    "copy",
    "copy2",
    "copytree",
    "move",
    "rmtree",
    "Error",
    "SpecialFileError",
    "ExecError",
    "ReadError",
    "ignore_patterns",
    "which",
    "get_terminal_size",
    "SameFileError",
]


class Error(OSError):
    pass


class SameFileError(Error):
    """Raised when source and destination are the same file."""


class SpecialFileError(OSError):
    """Raised when trying to do a kind of operation (e.g. copying) which is
    not supported on a special file (e.g. a named pipe)."""


class ExecError(OSError):
    """Raised when a command could not be executed."""


class ReadError(OSError):
    """Raised when an archive cannot be read."""


def _path_str(p):
    p = os.fspath(p)
    if isinstance(p, bytes):
        return p.decode("utf-8")
    return p


def _s_imode(mode):
    return mode & 0o7777


def copyfileobj(fsrc, fdst, length=0):
    """Copy data from file-like object fsrc to file-like object fdst."""
    if not length:
        length = COPY_BUFSIZE
    fsrc_read = fsrc.read
    fdst_write = fdst.write
    while True:
        buf = fsrc_read(length)
        if not buf:
            break
        fdst_write(buf)


def _samefile(src, dst):
    src_s = _path_str(src)
    dst_s = _path_str(dst)
    try:
        return os.path.samefile(src_s, dst_s)
    except OSError:
        return False


def _islink(fn):
    return os.path.islink(_path_str(fn))


def copyfile(src, dst, *, follow_symlinks=True):
    """Copy data from src to dst.

    If follow_symlinks is not set and src is a symbolic link, a new
    symlink will be created instead of copying the file it points to.
    """
    src_s = _path_str(src)
    dst_s = _path_str(dst)

    if _samefile(src_s, dst_s):
        raise SameFileError("{!r} and {!r} are the same file".format(src, dst))

    for fn in (src_s, dst_s):
        if _is_fifo(fn):
            raise SpecialFileError("`%s` is a named pipe" % fn)

    if not follow_symlinks and _islink(src_s):
        os.symlink(os.readlink(src_s), dst_s)
    else:
        try:
            _copyfile_native(src_s, dst_s)
        except IsADirectoryError as e:
            if not os.path.exists(dst_s):
                raise FileNotFoundError(
                    "Directory does not exist: %s" % dst_s
                ) from e
            raise

    return dst


def copymode(src, dst, *, follow_symlinks=True):
    """Copy mode bits from src to dst.

    If follow_symlinks is not set, symlinks aren't followed if and only
    if both `src` and `dst` are symlinks.
    """
    src_s = _path_str(src)
    dst_s = _path_str(dst)
    if not follow_symlinks and _islink(src_s) and _islink(dst_s):
        if _WINDOWS:
            st = os.lstat(src_s)
            os.chmod(dst_s, _s_imode(st.st_mode))
        else:
            # Linux does not support lchmod; match CPython by doing nothing.
            return
    else:
        if _WINDOWS and _islink(dst_s):
            dst_s = os.path.realpath(dst_s)
        st = os.stat(src_s)
        os.chmod(dst_s, _s_imode(st.st_mode))


def copystat(src, dst, *, follow_symlinks=True):
    """Copy file metadata (permission bits, atime, mtime) from src to dst."""
    src_s = _path_str(src)
    dst_s = _path_str(dst)
    follow = follow_symlinks or not (_islink(src_s) and _islink(dst_s))
    st = os.stat(src_s) if follow else os.lstat(src_s)
    mode = _s_imode(st.st_mode)
    _copystat_times(src_s, dst_s, follow)
    if follow:
        target = os.path.realpath(dst_s) if (_WINDOWS and _islink(dst_s)) else dst_s
        os.chmod(target, mode)
    elif _WINDOWS:
        os.chmod(dst_s, mode)


def copy(src, dst, *, follow_symlinks=True):
    """Copy data and mode bits ("cp src dst"). Return the file's destination.

    The destination may be a directory.
    """
    src_s = _path_str(src)
    dst_s = _path_str(dst)
    if os.path.isdir(dst_s):
        dst = os.path.join(dst_s, os.path.basename(src_s))
    copyfile(src, dst, follow_symlinks=follow_symlinks)
    copymode(src, dst, follow_symlinks=follow_symlinks)
    return dst


def copy2(src, dst, *, follow_symlinks=True):
    """Copy data and metadata. Return the file's destination.

    The destination may be a directory.
    """
    src_s = _path_str(src)
    dst_s = _path_str(dst)
    if os.path.isdir(dst_s):
        dst = os.path.join(dst_s, os.path.basename(src_s))
    copyfile(src, dst, follow_symlinks=follow_symlinks)
    copystat(src, dst, follow_symlinks=follow_symlinks)
    return dst


def ignore_patterns(*patterns):
    """Function that can be used as copytree() ignore parameter."""

    def _ignore_patterns(path, names):
        ignored_names = []
        for pattern in patterns:
            ignored_names.extend(fnmatch.filter(names, pattern))
        return set(ignored_names)

    return _ignore_patterns


def copytree(
    src,
    dst,
    symlinks=False,
    ignore=None,
    copy_function=copy2,
    ignore_dangling_symlinks=False,
    dirs_exist_ok=False,
):
    """Recursively copy a directory tree and return the destination directory."""
    src_s = _path_str(src)
    dst_s = _path_str(dst)
    names = os.listdir(src_s)
    if ignore is not None:
        ignored_names = ignore(src_s, names)
    else:
        ignored_names = ()

    os.makedirs(dst_s, exist_ok=dirs_exist_ok)
    errors = []
    for name in names:
        if name in ignored_names:
            continue
        srcname = os.path.join(src_s, name)
        dstname = os.path.join(dst_s, name)
        try:
            if os.path.islink(srcname):
                linkto = os.readlink(srcname)
                if symlinks:
                    os.symlink(linkto, dstname)
                    copystat(srcname, dstname, follow_symlinks=False)
                else:
                    if not os.path.exists(srcname) and ignore_dangling_symlinks:
                        continue
                    if os.path.isdir(srcname):
                        copytree(
                            srcname,
                            dstname,
                            symlinks,
                            ignore,
                            copy_function,
                            ignore_dangling_symlinks,
                            dirs_exist_ok,
                        )
                    else:
                        copy_function(srcname, dstname)
            elif os.path.isdir(srcname):
                copytree(
                    srcname,
                    dstname,
                    symlinks,
                    ignore,
                    copy_function,
                    ignore_dangling_symlinks,
                    dirs_exist_ok,
                )
            else:
                copy_function(srcname, dstname)
        except Error as err:
            errors.extend(err.args[0])
        except OSError as why:
            errors.append((srcname, dstname, str(why)))
    try:
        copystat(src_s, dst_s)
    except OSError as why:
        if not _WINDOWS:
            errors.append((src_s, dst_s, str(why)))
    if errors:
        raise Error(errors)
    return dst


def _rmtree_unsafe(path, onexc):
    try:
        names = os.listdir(path)
    except OSError as err:
        onexc(os.listdir, path, err)
        names = []
    for name in names:
        fullname = os.path.join(path, name)
        if os.path.islink(fullname):
            try:
                os.unlink(fullname)
            except OSError as err:
                onexc(os.unlink, fullname, err)
        elif os.path.isdir(fullname):
            _rmtree_unsafe(fullname, onexc)
        else:
            try:
                os.unlink(fullname)
            except OSError as err:
                onexc(os.unlink, fullname, err)
    try:
        os.rmdir(path)
    except OSError as err:
        onexc(os.rmdir, path, err)


def rmtree(path, ignore_errors=False, onerror=None, *, onexc=None, dir_fd=None):
    """Recursively delete a directory tree."""
    if dir_fd is not None:
        raise NotImplementedError("dir_fd unavailable on this platform")

    path_s = _path_str(path)

    if ignore_errors:
        def _onexc(*args):
            pass
    elif onerror is None and onexc is None:
        def _onexc(*args):
            raise
    elif onexc is None:
        def _onexc(func, p, exc):
            if exc is None:
                exc_info = (None, None, None)
            else:
                exc_info = (type(exc), exc, getattr(exc, "__traceback__", None))
            return onerror(func, p, exc_info)
    else:
        _onexc = onexc

    try:
        if os.path.islink(path_s):
            raise OSError("Cannot call rmtree on a symbolic link")
    except OSError as err:
        _onexc(os.path.islink, path, err)
        return
    _rmtree_unsafe(path_s, _onexc)


rmtree.avoids_symlink_attacks = False


def _basename(path):
    """A basename() variant which first strips trailing separators."""
    path = _path_str(path)
    sep = os.path.sep + (os.path.altsep or "")
    return os.path.basename(path.rstrip(sep))


def _destinsrc(src, dst):
    src = os.path.abspath(_path_str(src))
    dst = os.path.abspath(_path_str(dst))
    if not src.endswith(os.path.sep):
        src += os.path.sep
    if not dst.endswith(os.path.sep):
        dst += os.path.sep
    if _WINDOWS:
        return os.path.normcase(dst).startswith(os.path.normcase(src))
    return dst.startswith(src)


def move(src, dst, copy_function=copy2):
    """Recursively move a file or directory to another location."""
    src_s = _path_str(src)
    dst_s = _path_str(dst)
    real_dst = dst
    real_dst_s = dst_s
    if os.path.isdir(dst_s):
        if _samefile(src_s, dst_s) and not os.path.islink(src_s):
            os.rename(src_s, dst_s)
            return
        real_dst_s = os.path.join(dst_s, _basename(src_s))
        real_dst = real_dst_s
        if os.path.exists(real_dst_s):
            raise Error("Destination path '%s' already exists" % real_dst_s)
    try:
        os.rename(src_s, real_dst_s)
    except OSError:
        if os.path.islink(src_s):
            linkto = os.readlink(src_s)
            os.symlink(linkto, real_dst_s)
            os.unlink(src_s)
        elif os.path.isdir(src_s):
            if _destinsrc(src_s, dst_s):
                raise Error(
                    "Cannot move a directory '%s' into itself '%s'."
                    % (src, dst)
                )
            copytree(src_s, real_dst_s, copy_function=copy_function, symlinks=True)
            rmtree(src_s)
        else:
            copy_function(src_s, real_dst_s)
            os.unlink(src_s)
    return real_dst


def get_terminal_size(fallback=(80, 24)):
    """Get the size of the terminal window as an os.terminal_size."""
    try:
        columns = int(os.environ["COLUMNS"])
    except (KeyError, ValueError):
        columns = 0

    try:
        lines = int(os.environ["LINES"])
    except (KeyError, ValueError):
        lines = 0

    if columns <= 0 or lines <= 0:
        try:
            queried = _query_terminal_size()
            size = os.terminal_size(queried)
        except (AttributeError, ValueError, OSError):
            size = os.terminal_size(fallback)
        if columns <= 0:
            columns = size.columns or fallback[0]
        if lines <= 0:
            lines = size.lines or fallback[1]

    return os.terminal_size((columns, lines))


def _access_check(fn, mode):
    fn_s = _path_str(fn)
    return (
        os.path.exists(fn_s)
        and os.access(fn_s, mode)
        and not os.path.isdir(fn_s)
    )


def _win_path_needs_curdir(cmd, mode):
    return (not (mode & os.X_OK)) or (
        "NoDefaultCurrentDirectoryInExePath" not in os.environ
    )


def which(cmd, mode=os.F_OK | os.X_OK, path=None):
    """Given a command, mode, and a PATH string, return the path which
    conforms to the given mode on the PATH, or None if there is no such file.
    """
    use_bytes = isinstance(cmd, bytes)
    cmd_s = cmd.decode("ISO-8859-1") if use_bytes else os.fspath(cmd)

    dirname, cmd_name = os.path.split(cmd_s)
    if dirname:
        dirs = [dirname]
    else:
        if path is None:
            path = os.environ.get("PATH", None)
            if path is None:
                path = ".:/bin:/usr/bin" if not _WINDOWS else ".;C:\\bin"

        if not path:
            return None

        if isinstance(path, bytes):
            path_s = path.decode("ISO-8859-1")
        else:
            path_s = os.fspath(path)

        dirs = path_s.split(os.pathsep)

        if sys.platform == "win32" and _win_path_needs_curdir(cmd_name, mode):
            dirs.insert(0, os.curdir)

    if sys.platform == "win32":
        pathext_source = os.getenv("PATHEXT") or _WIN_DEFAULT_PATHEXT
        pathext = [ext for ext in pathext_source.split(os.pathsep) if ext]
        files = [cmd_name] + [cmd_name + ext for ext in pathext]
        suffix = os.path.splitext(files[0])[1].upper()
        if mode & os.X_OK and not any(suffix == ext.upper() for ext in pathext):
            files.append(files.pop(0))
    else:
        files = [cmd_name]

    seen = set()
    for d in dirs:
        normdir = os.path.normcase(d)
        if normdir not in seen:
            seen.add(normdir)
            for thefile in files:
                name = os.path.join(d, thefile)
                if _access_check(name, mode):
                    return name.encode("ISO-8859-1") if use_bytes else name
    return None
