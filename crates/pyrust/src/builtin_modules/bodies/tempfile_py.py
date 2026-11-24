"""Temporary files.

This module provides generic, low- and high-level interfaces for
creating temporary files and directories.
"""

import errno as _errno
import io as _io
import os as _os
import shutil as _shutil
import sys as _sys
import types as _types
from random import Random as _Random

__all__ = [
    "NamedTemporaryFile",
    "TemporaryFile",
    "SpooledTemporaryFile",
    "TemporaryDirectory",
    "mkstemp",
    "mkdtemp",
    "mktemp",
    "TMP_MAX",
    "gettempprefix",
    "tempdir",
    "gettempdir",
    "gettempprefixb",
    "gettempdirb",
]

TMP_MAX = 10000
template = "tmp"
tempdir = None


def _fsencode(s):
    if isinstance(s, bytes):
        return s
    s = _os.fspath(s)
    if isinstance(s, bytes):
        return s
    return s.encode("utf-8")


def _fsdecode(s):
    if isinstance(s, str):
        return s
    s = _os.fspath(s)
    if isinstance(s, str):
        return s
    return s.decode("utf-8")


def _exists(fn):
    try:
        _os.lstat(_fsdecode(fn))
    except OSError:
        return False
    return True


def _infer_return_type(*args):
    """Look at the type of all args and divine their implied return type."""
    return_type = None
    for arg in args:
        if arg is None:
            continue
        if hasattr(arg, "__fspath__"):
            arg = _os.fspath(arg)
        if isinstance(arg, bytes):
            if return_type is str:
                raise TypeError(
                    "Can't mix bytes and non-bytes in path components."
                )
            return_type = bytes
        else:
            if return_type is bytes:
                raise TypeError(
                    "Can't mix bytes and non-bytes in path components."
                )
            return_type = str
    if return_type is None:
        cur = _read_module_tempdir()
        if cur is None or isinstance(cur, str):
            return str
        return bytes
    return return_type


def _read_module_tempdir():
    mod = _sys.modules.get("tempfile")
    if mod is not None:
        return getattr(mod, "tempdir", None)
    return tempdir


def _write_module_tempdir(val):
    global tempdir
    tempdir = val
    mod = _sys.modules.get("tempfile")
    if mod is not None:
        mod.tempdir = val


def _read_module_template():
    mod = _sys.modules.get("tempfile")
    if mod is not None:
        return getattr(mod, "template", template)
    return template


def _sanitize_params(prefix, suffix, dir):
    """Common parameter processing for most APIs in this module."""
    output_type = _infer_return_type(prefix, suffix, dir)
    if suffix is None:
        suffix = output_type()
    if prefix is None:
        tmpl = _read_module_template()
        if output_type is str:
            prefix = _fsdecode(tmpl)
        else:
            prefix = _fsencode(tmpl)
    if dir is None:
        if output_type is str:
            dir = gettempdir()
        else:
            dir = gettempdirb()
    else:
        dir = _os.fspath(dir)
    return prefix, suffix, dir, output_type


class _RandomNameSequence:
    """Generates an endless sequence of unpredictable 8-char strings."""

    characters = "abcdefghijklmnopqrstuvwxyz0123456789_"

    @property
    def rng(self):
        cur_pid = _os.getpid()
        if cur_pid != getattr(self, "_rng_pid", None):
            self._rng = _Random()
            self._rng_pid = cur_pid
        return self._rng

    def __iter__(self):
        return self

    def __next__(self):
        return "".join(self.rng.choices(self.characters, k=8))


def _candidate_tempdir_list():
    dirlist = []
    for envname in ("TMPDIR", "TEMP", "TMP"):
        dirname = _os.getenv(envname)
        if dirname:
            dirlist.append(dirname)

    if _os.name == "nt":
        sysroot = _os.getenv("SYSTEMROOT")
        candidates = [_os.path.expanduser(r"~\AppData\Local\Temp")]
        if sysroot:
            candidates.append(_os.path.join(sysroot, "Temp"))
        candidates.extend([r"c:\temp", r"c:\tmp", r"\temp", r"\tmp"])
        dirlist.extend(candidates)
    else:
        dirlist.extend(["/tmp", "/var/tmp", "/usr/tmp"])

    try:
        dirlist.append(_os.getcwd())
    except (AttributeError, OSError):
        dirlist.append(_os.curdir)

    return dirlist


def _get_default_tempdir():
    namer = _RandomNameSequence()
    dirlist = _candidate_tempdir_list()

    for d in dirlist:
        if d != _os.curdir:
            d = _os.path.abspath(d)
        for _ in range(100):
            name = next(namer)
            filename = _os.path.join(d, name)
            try:
                fd = _mkstemp_open(filename)
                try:
                    try:
                        _os.write(fd, b"blat")
                    finally:
                        _os.close(fd)
                finally:
                    _os.unlink(filename)
                return d
            except FileExistsError:
                pass
            except PermissionError:
                if (
                    _os.name == "nt"
                    and _os.path.isdir(d)
                    and _os.access(d, _os.W_OK)
                ):
                    continue
                break
            except OSError:
                break
    raise FileNotFoundError(
        _errno.ENOENT,
        "No usable temporary directory found in %s" % (dirlist,),
    )


_name_sequence = None


def _get_candidate_names():
    global _name_sequence
    if _name_sequence is None:
        _name_sequence = _RandomNameSequence()
    return _name_sequence


def gettempprefix():
    """The default prefix for temporary directories as string."""
    return _fsdecode(_read_module_template())


def gettempprefixb():
    """The default prefix for temporary directories as bytes."""
    return _fsencode(_read_module_template())


def _gettempdir():
    cur = _read_module_tempdir()
    if cur is None:
        cur = _get_default_tempdir()
        _write_module_tempdir(cur)
    return cur


def gettempdir():
    """Returns tempfile.tempdir as str."""
    return _fsdecode(_gettempdir())


def gettempdirb():
    """Returns tempfile.tempdir as bytes."""
    return _fsencode(_gettempdir())


def _mkstemp_inner(dir, pre, suf, output_type):
    dir_str = _os.path.abspath(_fsdecode(dir))
    pre_str = _fsdecode(pre)
    suf_str = _fsdecode(suf)
    names = _get_candidate_names()

    for _ in range(TMP_MAX):
        name = next(names)
        file_str = _os.path.join(dir_str, pre_str + name + suf_str)
        try:
            fd = _mkstemp_open(file_str)
        except FileExistsError:
            continue
        except PermissionError:
            if (
                _os.name == "nt"
                and _os.path.isdir(dir_str)
                and _os.access(dir_str, _os.W_OK)
            ):
                continue
            raise
        out_name = _fsencode(file_str) if output_type is bytes else file_str
        return fd, out_name

    raise FileExistsError(_errno.EEXIST, "No usable temporary file name found")


def mkstemp(suffix=None, prefix=None, dir=None, text=False):
    """Create and return a unique temporary file as (fd, pathname)."""
    prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)
    return _mkstemp_inner(dir, prefix, suffix, output_type)


def mkdtemp(suffix=None, prefix=None, dir=None):
    """Create and return a unique temporary directory."""
    prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)
    dir_str = _fsdecode(dir)
    pre_str = _fsdecode(prefix)
    suf_str = _fsdecode(suffix)
    names = _get_candidate_names()

    for _ in range(TMP_MAX):
        name = next(names)
        file_str = _os.path.join(dir_str, pre_str + name + suf_str)
        try:
            _os.mkdir(file_str, 0o700)
        except FileExistsError:
            continue
        except PermissionError:
            if (
                _os.name == "nt"
                and _os.path.isdir(dir_str)
                and _os.access(dir_str, _os.W_OK)
            ):
                continue
            raise
        abs_path = _os.path.abspath(file_str)
        return _fsencode(abs_path) if output_type is bytes else abs_path

    raise FileExistsError(
        _errno.EEXIST, "No usable temporary directory name found"
    )


def mktemp(suffix="", prefix=template, dir=None):
    """Return a unique temporary file name without creating it (unsafe)."""
    if dir is None:
        dir = gettempdir()
    names = _get_candidate_names()
    for _ in range(TMP_MAX):
        name = next(names)
        file_str = _os.path.join(dir, prefix + name + suffix)
        if not _exists(file_str):
            return file_str
    raise FileExistsError(
        _errno.EEXIST, "No usable temporary filename found"
    )


def _open_exclusive_stream(
    dir, pre, suf, output_type, mode, encoding, errors, newline,
    unlink_immediately=False,
):
    dir_str = _os.path.abspath(_fsdecode(dir))
    pre_str = _fsdecode(pre)
    suf_str = _fsdecode(suf)
    names = _get_candidate_names()

    for _ in range(TMP_MAX):
        name = next(names)
        file_str = _os.path.join(dir_str, pre_str + name + suf_str)
        display_name = (
            _fsencode(file_str) if output_type is bytes else file_str
        )
        try:
            stream = _open_native_temp(
                file_str,
                mode,
                encoding,
                errors,
                newline,
                unlink_immediately,
                display_name,
            )
        except FileExistsError:
            continue
        except PermissionError:
            if (
                _os.name == "nt"
                and _os.path.isdir(dir_str)
                and _os.access(dir_str, _os.W_OK)
            ):
                continue
            raise
        return stream, display_name

    raise FileExistsError(_errno.EEXIST, "No usable temporary file name found")


class _TemporaryFileCloser:
    """Close and optionally unlink a temporary file cleanly across platforms."""

    def __init__(self, file, name, delete=True, delete_on_close=True):
        self.file = file
        self.name = name
        self.delete = delete
        self.delete_on_close = delete_on_close
        self.cleanup_called = False
        self.close_called = False

    def cleanup(self):
        if not self.cleanup_called:
            self.cleanup_called = True
            try:
                if not self.close_called:
                    self.close_called = True
                    self.file.close()
            finally:
                if self.delete:
                    try:
                        _os.unlink(_fsdecode(self.name))
                    except FileNotFoundError:
                        pass

    def close(self):
        if not self.close_called:
            self.close_called = True
            try:
                self.file.close()
            finally:
                if self.delete and self.delete_on_close:
                    self.cleanup()

    def __del__(self):
        self.cleanup()


class _TemporaryFileWrapper:
    """Temporary file wrapper managing cleanup on close / context exit."""

    def __init__(self, file, name, delete=True, delete_on_close=True):
        self.file = file
        self.name = name
        self.delete = delete
        self._closer = _TemporaryFileCloser(
            file, name, delete, delete_on_close
        )

    def __getattr__(self, name):
        file = self.__dict__["file"]
        return getattr(file, name)

    def __enter__(self):
        self.file.__enter__()
        return self

    def __exit__(self, exc, value, tb):
        result = self.file.__exit__(exc, value, tb)
        self._closer.cleanup()
        return result

    def close(self):
        self._closer.close()

    def writelines(self, lines):
        for line in lines:
            self.file.write(line)

    def __iter__(self):
        for line in self.file:
            yield line

    def __next__(self):
        return next(self.file)


def NamedTemporaryFile(
    mode="w+b",
    buffering=-1,
    encoding=None,
    newline=None,
    suffix=None,
    prefix=None,
    dir=None,
    delete=True,
    *,
    errors=None,
    delete_on_close=True,
):
    """Create and return a named temporary file."""
    prefix, suffix, dir, output_type = _sanitize_params(prefix, suffix, dir)
    stream, name = _open_exclusive_stream(
        dir,
        prefix,
        suffix,
        output_type,
        mode,
        encoding,
        errors,
        newline,
        unlink_immediately=False,
    )
    return _TemporaryFileWrapper(stream, name, delete, delete_on_close)


if _os.name != "posix" or _sys.platform == "cygwin":
    TemporaryFile = NamedTemporaryFile
else:

    def TemporaryFile(
        mode="w+b",
        buffering=-1,
        encoding=None,
        newline=None,
        suffix=None,
        prefix=None,
        dir=None,
        *,
        errors=None,
    ):
        """Create and return an unnamed temporary file."""
        prefix, suffix, dir, output_type = _sanitize_params(
            prefix, suffix, dir
        )
        stream, _name = _open_exclusive_stream(
            dir,
            prefix,
            suffix,
            output_type,
            mode,
            encoding,
            errors,
            newline,
            unlink_immediately=True,
        )
        return stream


class SpooledTemporaryFile(_io.IOBase):
    """Temporary file wrapper that rolls over from memory to disk past max_size."""

    _rolled = False

    def __init__(
        self,
        max_size=0,
        mode="w+b",
        buffering=-1,
        encoding=None,
        newline=None,
        suffix=None,
        prefix=None,
        dir=None,
        *,
        errors=None,
    ):
        if "b" in mode:
            self._file = _io.BytesIO()
        else:
            self._file = _io.StringIO("", newline)
        self._max_size = max_size
        self._rolled = False
        self._TemporaryFileArgs = {
            "mode": mode,
            "buffering": buffering,
            "suffix": suffix,
            "prefix": prefix,
            "encoding": encoding,
            "newline": newline,
            "dir": dir,
            "errors": errors,
        }

    __class_getitem__ = classmethod(_types.GenericAlias)

    def _check(self, file):
        if self._rolled:
            return
        max_size = self._max_size
        if max_size and file.tell() > max_size:
            self.rollover()

    def rollover(self):
        if self._rolled:
            return
        file = self._file
        newfile = self._file = TemporaryFile(**self._TemporaryFileArgs)
        del self._TemporaryFileArgs

        pos = file.tell()
        newfile.write(file.getvalue())
        newfile.seek(pos, 0)
        file.close()

        self._rolled = True

    def __enter__(self):
        if self._file.closed:
            raise ValueError("Cannot enter context with closed file")
        return self

    def __exit__(self, exc, value, tb):
        self._file.close()

    def __iter__(self):
        return self._file.__iter__()

    def __next__(self):
        return next(self._file)

    def close(self):
        self._file.close()

    @property
    def closed(self):
        return self._file.closed

    @property
    def encoding(self):
        if self._rolled:
            return self._file.encoding
        if "b" in self._TemporaryFileArgs["mode"]:
            raise AttributeError("encoding")
        return self._TemporaryFileArgs["encoding"] or "utf-8"

    @property
    def errors(self):
        if self._rolled:
            return self._file.errors
        if "b" in self._TemporaryFileArgs["mode"]:
            raise AttributeError("errors")
        return self._TemporaryFileArgs["errors"] or "strict"

    def fileno(self):
        self.rollover()
        return self._file.fileno()

    def flush(self):
        self._file.flush()

    def isatty(self):
        return self._file.isatty()

    @property
    def mode(self):
        try:
            return self._file.mode
        except AttributeError:
            return self._TemporaryFileArgs["mode"]

    @property
    def name(self):
        try:
            return self._file.name
        except AttributeError:
            return None

    @property
    def newlines(self):
        return getattr(self._file, "newlines", None)

    def readable(self):
        return self._file.readable()

    def read(self, *args):
        return self._file.read(*args)

    def read1(self, *args):
        return self._file.read1(*args)

    def readinto(self, b):
        return self._file.readinto(b)

    def readline(self, *args):
        return self._file.readline(*args)

    def readlines(self, *args):
        return self._file.readlines(*args)

    def seekable(self):
        return self._file.seekable()

    def seek(self, *args):
        return self._file.seek(*args)

    def tell(self):
        return self._file.tell()

    def truncate(self, size=None):
        if size is None:
            return self._file.truncate()
        if size > self._max_size:
            self.rollover()
        return self._file.truncate(size)

    def writable(self):
        return self._file.writable()

    def write(self, s):
        file = self._file
        rv = file.write(s)
        self._check(file)
        return rv

    def writelines(self, iterable):
        file = self._file
        rv = file.writelines(iterable)
        self._check(file)
        return rv


def _resetperms(path):
    try:
        if _os.name == "nt" or not _os.path.islink(path):
            _os.chmod(path, 0o700)
    except OSError:
        pass


class TemporaryDirectory:
    """Create and return a temporary directory usable as a context manager."""

    def __init__(
        self,
        suffix=None,
        prefix=None,
        dir=None,
        ignore_cleanup_errors=False,
        *,
        delete=True,
    ):
        self.name = mkdtemp(suffix, prefix, dir)
        self._ignore_cleanup_errors = ignore_cleanup_errors
        self._delete = delete
        self._cleaned = False

    @classmethod
    def _rmtree(cls, name, ignore_errors=False, repeated=False):
        def onexc(func, path, exc):
            if isinstance(exc, PermissionError):
                if repeated and path == name:
                    if ignore_errors:
                        return
                    raise
                try:
                    if path != name:
                        _resetperms(_os.path.dirname(path))
                    _resetperms(path)
                    try:
                        _os.unlink(path)
                    except IsADirectoryError:
                        cls._rmtree(path, ignore_errors=ignore_errors)
                    except PermissionError:
                        if not _os.path.isdir(path):
                            if ignore_errors:
                                return
                            raise
                        cls._rmtree(
                            path,
                            ignore_errors=ignore_errors,
                            repeated=(path == name),
                        )
                except FileNotFoundError:
                    pass
            elif isinstance(exc, FileNotFoundError):
                pass
            else:
                if not ignore_errors:
                    raise

        _shutil.rmtree(name, onexc=onexc)

    def __repr__(self):
        return "<{} {!r}>".format(self.__class__.__name__, self.name)

    def __enter__(self):
        return self.name

    def __exit__(self, exc, value, tb):
        if self._delete:
            self.cleanup()

    def cleanup(self):
        if not self._cleaned or _os.path.exists(_fsdecode(self.name)):
            self._cleaned = True
            self._rmtree(
                _fsdecode(self.name),
                ignore_errors=self._ignore_cleanup_errors,
            )

    def __del__(self):
        if getattr(self, "_delete", False) and not getattr(
            self, "_cleaned", True
        ):
            self.cleanup()

    __class_getitem__ = classmethod(_types.GenericAlias)
