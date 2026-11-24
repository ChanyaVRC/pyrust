"""Filename matching with shell patterns.

fnmatch(FILENAME, PATTERN) matches according to the local convention.
fnmatchcase(FILENAME, PATTERN) always takes case in account.

The functions operate by translating the pattern into a regular
expression.  They cache the compiled regular expressions for speed.

The function translate(PATTERN) returns a regular expression
corresponding to PATTERN.  (It does not compile it.)
"""

import os
import re

__all__ = ["filter", "fnmatch", "fnmatchcase", "translate"]

_pattern_cache = {}
_CACHE_MAX = 32768


def _compile_pattern(pat):
    if isinstance(pat, bytes):
        is_bytes = True
    elif isinstance(pat, str):
        is_bytes = False
    else:
        raise TypeError(
            "expected str or bytes, not %s" % type(pat).__name__
        )
    key = (is_bytes, pat)
    cached = _pattern_cache.get(key)
    if cached is not None:
        return cached
    if is_bytes:
        pat_str = pat.decode("ISO-8859-1")
        res_str = translate(pat_str)
    else:
        res_str = translate(pat)
    compiled = (is_bytes, re.compile(res_str).match)
    if len(_pattern_cache) >= _CACHE_MAX:
        _pattern_cache.clear()
    _pattern_cache[key] = compiled
    return compiled


def _match_with(is_bytes, match, name):
    if is_bytes:
        if not isinstance(name, bytes):
            raise TypeError("cannot use a bytes pattern on a string-like object")
        return match(name.decode("ISO-8859-1")) is not None
    if not isinstance(name, str):
        raise TypeError("cannot use a string pattern on a bytes-like object")
    return match(name) is not None


def fnmatch(name, pat):
    """Test whether FILENAME matches PATTERN.

    Patterns are Unix shell style:

    *       matches everything
    ?       matches any single character
    [seq]   matches any character in seq
    [!seq]  matches any char not in seq

    An initial period in FILENAME is not special.
    Both FILENAME and PATTERN are first case-normalized
    if the operating system requires it.
    If you don't want this, use fnmatchcase(FILENAME, PATTERN).
    """
    name = os.path.normcase(name)
    pat = os.path.normcase(pat)
    return fnmatchcase(name, pat)


def filter(names, pat):
    """Construct a list from those elements of the iterable NAMES that match PAT."""
    result = []
    pat = os.path.normcase(pat)
    is_bytes, match = _compile_pattern(pat)
    if os.name == "posix":
        # normcase on POSIX is a no-op; avoid calling it in the inner loop.
        for name in names:
            if _match_with(is_bytes, match, name):
                result.append(name)
    else:
        for name in names:
            if _match_with(is_bytes, match, os.path.normcase(name)):
                result.append(name)
    return result


def fnmatchcase(name, pat):
    """Test whether FILENAME matches PATTERN, including case.

    This is a version of fnmatch() which doesn't case-normalize
    its arguments.
    """
    is_bytes, match = _compile_pattern(pat)
    return _match_with(is_bytes, match, name)


def translate(pat):
    """Translate a shell PATTERN to a regular expression.

    There is no way to quote meta-characters.
    """
    STAR = object()
    res = []
    add = res.append
    i, n = 0, len(pat)
    while i < n:
        c = pat[i]
        i = i + 1
        if c == "*":
            # compress consecutive `*` into one
            if (not res) or res[-1] is not STAR:
                add(STAR)
        elif c == "?":
            add(".")
        elif c == "[":
            j = i
            if j < n and pat[j] == "!":
                j = j + 1
            if j < n and pat[j] == "]":
                j = j + 1
            while j < n and pat[j] != "]":
                j = j + 1
            if j >= n:
                add("\\[")
            else:
                stuff = pat[i:j]
                if "-" not in stuff:
                    stuff = stuff.replace("\\", r"\\")
                else:
                    chunks = []
                    k = i + 2 if pat[i] == "!" else i + 1
                    while True:
                        k = pat.find("-", k, j)
                        if k < 0:
                            break
                        chunks.append(pat[i:k])
                        i = k + 1
                        k = k + 3
                    chunk = pat[i:j]
                    if chunk:
                        chunks.append(chunk)
                    else:
                        chunks[-1] += "-"
                    # Remove empty ranges -- invalid in RE.
                    for k in range(len(chunks) - 1, 0, -1):
                        if chunks[k - 1][-1] > chunks[k][0]:
                            chunks[k - 1] = chunks[k - 1][:-1] + chunks[k][1:]
                            del chunks[k]
                    # Escape backslashes and hyphens for set difference (--).
                    # Hyphens that create ranges shouldn't be escaped.
                    stuff = "-".join(
                        s.replace("\\", r"\\").replace("-", r"\-")
                        for s in chunks
                    )
                # Escape set operations (&&, ~~ and ||).
                stuff = re.sub(r"([&~|])", r"\\\1", stuff)
                i = j + 1
                if not stuff:
                    # Empty range: never match.
                    add("(?!)")
                elif stuff == "!":
                    # Negated empty range: match any character.
                    add(".")
                else:
                    if stuff[0] == "!":
                        stuff = "^" + stuff[1:]
                    elif stuff[0] in ("^", "["):
                        stuff = "\\" + stuff
                    add("[" + stuff + "]")
        else:
            add(re.escape(c))

    # Deal with STARs.
    inp = res
    res = []
    add = res.append
    i, n = 0, len(inp)
    # Fixed pieces at the start?
    while i < n and inp[i] is not STAR:
        add(inp[i])
        i += 1
    # Now deal with STAR fixed STAR fixed ...
    # For an interior `STAR fixed` pairing, use an atomic group `(?>.*?fixed)`
    # matching CPython 3.12.
    while i < n:
        i += 1
        if i == n:
            add(".*")
            break
        fixed = []
        while i < n and inp[i] is not STAR:
            fixed.append(inp[i])
            i += 1
        fixed = "".join(fixed)
        if i == n:
            add(".*")
            add(fixed)
        else:
            add("(?>.*?" + fixed + ")")
    res = "".join(res)
    return r"(?s:" + res + r")\Z"
