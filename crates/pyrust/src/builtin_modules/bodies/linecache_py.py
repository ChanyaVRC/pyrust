"""Cache lines from Python source files.

This is intended to read lines from modules imported -- hence if a filename
is not found, it will look down the module search path for a file by
that name.
"""

import functools
import os
import sys
import tokenize

__all__ = ["getline", "clearcache", "checkcache", "lazycache"]

# The cache. Maps filenames to either a 1-tuple thunk which will provide
# source code, or a 4-tuple (size, mtime, lines, fullname) once loaded.
cache = {}


def clearcache():
    """Clear the cache entirely."""
    cache.clear()


def getline(filename, lineno, module_globals=None):
    """Get a line for a Python source file from the cache.
    Update the cache if it doesn't contain an entry for this file already."""
    lines = getlines(filename, module_globals)
    if 1 <= lineno <= len(lines):
        return lines[lineno - 1]
    return ""


def getlines(filename, module_globals=None):
    """Get the lines for a Python source file from the cache.
    Update the cache if it doesn't contain an entry for this file already."""
    if filename in cache:
        entry = cache[filename]
        if len(entry) != 1:
            return cache[filename][2]

    try:
        return updatecache(filename, module_globals)
    except MemoryError:
        clearcache()
        return []


def checkcache(filename=None):
    """Discard cache entries that are out of date.
    (This is not checked upon each call!)"""
    if filename is None:
        filenames = list(cache.keys())
    else:
        filenames = [filename]

    for fname in filenames:
        try:
            entry = cache[fname]
        except KeyError:
            continue

        if len(entry) == 1:
            # Lazy cache entry; leave it lazy.
            continue
        size, mtime, lines, fullname = entry
        if mtime is None:
            continue  # No-op for files loaded via a loader.
        try:
            stat = os.stat(fullname)
        except (OSError, ValueError):
            cache.pop(fname, None)
            continue
        if size != stat.st_size or mtime != stat.st_mtime:
            cache.pop(fname, None)


def _normalize_lines(lines):
    """Normalize CRLF/CR line endings and ensure trailing newline on last line."""
    normalized = []
    for line in lines:
        if line.endswith("\r\n"):
            line = line[:-2] + "\n"
        elif line.endswith("\r"):
            line = line[:-1] + "\n"
        normalized.append(line)
    if normalized and not normalized[-1].endswith("\n"):
        normalized[-1] += "\n"
    return normalized


def updatecache(filename, module_globals=None):
    """Update a cache entry and return its list of lines.
    If something's wrong, discard the cache entry and return an empty list."""
    if filename in cache:
        if len(cache[filename]) != 1:
            cache.pop(filename, None)
    if not filename or (filename.startswith("<") and filename.endswith(">")):
        return []

    fullname = filename
    try:
        stat = os.stat(fullname)
    except OSError:
        basename = filename

        # Realise a lazy loader based lookup if there is one,
        # otherwise try to look up via module_globals right now.
        if lazycache(filename, module_globals):
            try:
                data = cache[filename][0]()
            except (ImportError, OSError):
                pass
            else:
                if data is None:
                    # The loader cannot find the source for this module.
                    return []
                cache[filename] = (
                    len(data),
                    None,
                    [line + "\n" for line in data.splitlines()],
                    fullname,
                )
                return cache[filename][2]

        # Try looking through the module search path (only for relative paths).
        if os.path.isabs(filename):
            return []

        for dirname in sys.path:
            try:
                fullname = os.path.join(dirname, basename)
            except (TypeError, AttributeError):
                continue
            try:
                stat = os.stat(fullname)
                break
            except (OSError, ValueError):
                pass
        else:
            return []
    except ValueError:
        return []

    try:
        with tokenize.open(fullname) as fp:
            lines = fp.readlines()
    except (OSError, UnicodeDecodeError, SyntaxError):
        return []
    except LookupError:
        try:
            with open(fullname, "r", encoding="utf-8") as fp:
                lines = fp.readlines()
        except (OSError, UnicodeDecodeError):
            return []

    lines = _normalize_lines(lines)
    size, mtime = stat.st_size, stat.st_mtime
    cache[filename] = (size, mtime, lines, fullname)
    return lines


def lazycache(filename, module_globals):
    """Seed the cache for filename with module_globals.

    The module loader will be asked for the source only when getlines is
    called, not immediately.

    If there is an entry in the cache already, it is not altered.

    Return True if a lazy load is registered in the cache, otherwise False.
    """
    if filename in cache:
        return len(cache[filename]) == 1
    if not filename or (filename.startswith("<") and filename.endswith(">")):
        return False
    if module_globals and "__name__" in module_globals:
        spec = module_globals.get("__spec__")
        name = getattr(spec, "name", None) or module_globals["__name__"]
        loader = getattr(spec, "loader", None)
        if loader is None:
            loader = module_globals.get("__loader__")
        get_source = getattr(loader, "get_source", None)

        if name and get_source:
            get_lines = functools.partial(get_source, name)
            cache[filename] = (get_lines,)
            return True
    return False
