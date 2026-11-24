"""Filename globbing utility."""

import fnmatch
import itertools
import os
import re

__all__ = ["glob", "iglob", "escape"]

_magic_check = re.compile("([*?[])")


def _to_str(p):
    if isinstance(p, bytes):
        return p.decode("ISO-8859-1")
    return p


def _from_str(s, is_bytes):
    if is_bytes:
        return s.encode("ISO-8859-1")
    return s


def _split(path):
    is_bytes = isinstance(path, bytes)
    head, tail = os.path.split(_to_str(path))
    return _from_str(head, is_bytes), _from_str(tail, is_bytes)


def _splitdrive(path):
    is_bytes = isinstance(path, bytes)
    drive, tail = os.path.splitdrive(_to_str(path))
    return _from_str(drive, is_bytes), _from_str(tail, is_bytes)


def _join_path(a, b):
    is_bytes = isinstance(a, bytes) or isinstance(b, bytes)
    return _from_str(os.path.join(_to_str(a), _to_str(b)), is_bytes)


def glob(pathname, *, root_dir=None, dir_fd=None, recursive=False,
         include_hidden=False):
    """Return a list of paths matching a pathname pattern.

    The pattern may contain simple shell-style wildcards a la
    fnmatch. Unlike fnmatch, filenames starting with a
    dot are special cases that are not matched by '*' and '?'
    patterns by default.

    If `include_hidden` is true, the patterns '*', '?', '**' will match hidden
    directories.

    If `recursive` is true, the pattern '**' will match any files and
    zero or more directories and subdirectories.
    """
    return list(
        iglob(
            pathname,
            root_dir=root_dir,
            dir_fd=dir_fd,
            recursive=recursive,
            include_hidden=include_hidden,
        )
    )


def iglob(pathname, *, root_dir=None, dir_fd=None, recursive=False,
          include_hidden=False):
    """Return an iterator which yields the paths matching a pathname pattern.

    The pattern may contain simple shell-style wildcards a la
    fnmatch. However, unlike fnmatch, filenames starting with a
    dot are special cases that are not matched by '*' and '?'
    patterns.

    If recursive is true, the pattern '**' will match any files and
    zero or more directories and subdirectories.
    """
    if dir_fd is not None:
        raise NotImplementedError("dir_fd is not supported on this platform")
    if root_dir is not None:
        root_dir = os.fspath(root_dir)
        if isinstance(pathname, bytes) != isinstance(root_dir, bytes):
            raise TypeError(
                "pathname and root_dir must have the same type (str or bytes)"
            )
    else:
        root_dir = pathname[:0]
    it = _iglob(
        pathname,
        root_dir,
        dir_fd,
        recursive,
        False,
        include_hidden=include_hidden,
    )
    if not pathname or (recursive and _isrecursive(pathname[:2])):
        try:
            s = next(it)  # skip empty string
            if s:
                it = itertools.chain((s,), it)
        except StopIteration:
            pass
    return it


def _iglob(pathname, root_dir, dir_fd, recursive, dironly,
           include_hidden=False):
    dirname, basename = _split(pathname)
    if not has_magic(pathname):
        if basename:
            if _lexists(_join(root_dir, pathname), dir_fd):
                yield pathname
        else:
            # Patterns ending with a slash should match only directories
            if _isdir(_join(root_dir, dirname), dir_fd):
                yield pathname
        return
    if not dirname:
        if recursive and _isrecursive(basename):
            yield from _glob2(
                root_dir, basename, dir_fd, dironly,
                include_hidden=include_hidden,
            )
        else:
            yield from _glob1(
                root_dir, basename, dir_fd, dironly,
                include_hidden=include_hidden,
            )
        return
    # `os.path.split()` returns the argument itself as a dirname if it is a
    # drive or UNC path. Prevent an infinite recursion if a drive or UNC path
    # contains magic characters (i.e. r'\\?\C:').
    if dirname != pathname and has_magic(dirname):
        dirs = _iglob(
            dirname, root_dir, dir_fd, recursive, True,
            include_hidden=include_hidden,
        )
    else:
        dirs = [dirname]
    if has_magic(basename):
        if recursive and _isrecursive(basename):
            glob_in_dir = _glob2
        else:
            glob_in_dir = _glob1
    else:
        glob_in_dir = _glob0
    for dirname in dirs:
        for name in glob_in_dir(
            _join(root_dir, dirname),
            basename,
            dir_fd,
            dironly,
            include_hidden=include_hidden,
        ):
            yield _join_path(dirname, name)


def _glob1(dirname, pattern, dir_fd, dironly, include_hidden=False):
    names = _listdir(dirname, dir_fd, dironly)
    if include_hidden or not _ishidden(pattern):
        names = [x for x in names if include_hidden or not _ishidden(x)]
    return fnmatch.filter(names, pattern)


def _glob0(dirname, basename, dir_fd, dironly, include_hidden=False):
    if basename:
        if _lexists(_join(dirname, basename), dir_fd):
            return [basename]
    else:
        # `os.path.split()` returns an empty basename for paths ending with a
        # directory separator. 'q*x/' should match only directories.
        if _isdir(dirname, dir_fd):
            return [basename]
    return []


def glob0(dirname, pattern):
    return _glob0(dirname, pattern, None, False)


def glob1(dirname, pattern):
    return _glob1(dirname, pattern, None, False)


def _glob2(dirname, pattern, dir_fd, dironly, include_hidden=False):
    if not dirname or _isdir(dirname, dir_fd):
        yield pattern[:0]
    yield from _rlistdir(
        dirname, dir_fd, dironly, include_hidden=include_hidden
    )


def _listdir(dirname, dir_fd, dironly):
    is_bytes = isinstance(dirname, bytes)
    arg = _to_str(dirname) if dirname else os.curdir
    try:
        entries = os.listdir(arg)
    except OSError:
        return []
    result = []
    for entry in entries:
        if dironly:
            try:
                if not os.path.isdir(os.path.join(arg, entry)):
                    continue
            except OSError:
                continue
        result.append(_from_str(entry, is_bytes))
    return result


def _rlistdir(dirname, dir_fd, dironly, include_hidden=False):
    names = _listdir(dirname, dir_fd, dironly)
    for x in names:
        if include_hidden or not _ishidden(x):
            yield x
            path = _join(dirname, x) if dirname else x
            for y in _rlistdir(
                path, dir_fd, dironly, include_hidden=include_hidden
            ):
                yield _join(x, y)


def _lexists(pathname, dir_fd):
    try:
        return os.path.lexists(_to_str(pathname))
    except (OSError, ValueError):
        return False


def _isdir(pathname, dir_fd):
    try:
        return os.path.isdir(_to_str(pathname))
    except (OSError, ValueError):
        return False


def _join(dirname, basename):
    # It is common if dirname or basename is empty
    if not dirname or not basename:
        return dirname or basename
    return _join_path(dirname, basename)


def has_magic(s):
    if isinstance(s, bytes):
        return _magic_check.search(s.decode("ISO-8859-1")) is not None
    return _magic_check.search(s) is not None


def _ishidden(path):
    if not path:
        return False
    return path[0] in (".", 46)


def _isrecursive(pattern):
    if isinstance(pattern, bytes):
        return pattern == b"**"
    return pattern == "**"


def escape(pathname):
    """Escape all special characters."""
    # Escaping is done by wrapping any of "*?[" between square brackets.
    # Metacharacters do not work in the drive part and shouldn't be escaped.
    drive, tail = _splitdrive(pathname)
    if isinstance(tail, bytes):
        escaped = _magic_check.sub(r"[\1]", tail.decode("ISO-8859-1")).encode(
            "ISO-8859-1"
        )
    else:
        escaped = _magic_check.sub(r"[\1]", tail)
    return drive + escaped
