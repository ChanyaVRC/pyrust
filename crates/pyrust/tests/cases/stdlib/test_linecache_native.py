# CPython 3.12.3 parity tests for the native `linecache` module (issue #3445).
#
# Everything printed here is compared byte-for-byte with CPython, so paths
# inside the temporary directory are shown relative to it.

import functools
import linecache
import os
import sys
import tempfile
import traceback

print("module attrs:", [n for n in ("cache", "getline", "getlines", "clearcache",
                                    "checkcache", "updatecache", "lazycache")
                        if hasattr(linecache, n)])
print("__all__:", linecache.__all__)
print("cache type:", type(linecache.cache).__name__)
print("module imports:", all(hasattr(linecache, n) for n in ("os", "sys", "functools", "tokenize")))
print("doc:", linecache.__doc__.splitlines()[0])
for fn in (linecache.getline, linecache.getlines, linecache.checkcache,
           linecache.updatecache, linecache.lazycache, linecache.clearcache):
    print("fn:", fn.__name__, fn.__module__, callable(fn))

with tempfile.TemporaryDirectory() as tmpdir:
    tmpdir = os.path.realpath(tmpdir)

    def rel(path):
        return os.path.relpath(path, tmpdir).replace(os.sep, "/") if os.path.isabs(path) else path

    def write(name, data):
        path = os.path.join(tmpdir, name)
        with open(path, "wb") as f:
            f.write(data)
        return path

    def entry_summary(key):
        entry = linecache.cache[key]
        if len(entry) == 1:
            return ("lazy", type(entry[0]).__name__)
        size, mtime, lines, fullname = entry
        return (size, mtime is None, lines, rel(fullname))

    # 1. The cache is the live, mutable module dict.
    linecache.clearcache()
    cache_obj = linecache.cache
    print("cache empty:", len(linecache.cache))
    manual = os.path.join(tmpdir, "manual_entry.py")
    linecache.cache[manual] = (10, None, ["m1\n", "m2\n"], manual)
    print("manual getlines:", linecache.getlines(manual))
    print("manual getline:", repr(linecache.getline(manual, 2)), repr(linecache.getline(manual, 3)))
    # mtime None entries survive checkcache even if the file never existed
    linecache.checkcache(manual)
    linecache.checkcache()
    print("manual kept:", manual in linecache.cache)
    # replacing the entry is seen on the very next call
    linecache.cache[manual] = (10, None, ["replaced\n"], manual)
    print("replaced:", linecache.getlines(manual))
    del linecache.cache[manual]
    print("deleted manual:", linecache.getlines(manual))
    # clearcache clears the same dict object
    linecache.cache["x"] = (1, None, ["x\n"], "x")
    linecache.clearcache()
    print("clearcache same object:", linecache.cache is cache_obj, len(cache_obj))
    # externally cleared dict
    linecache.cache["y"] = (1, None, ["y\n"], "y")
    cache_obj.clear()
    print("external clear:", len(linecache.cache), linecache.getlines("y"))

    # 2. getline / getlines on a real file; trailing newline; bounds.
    p_basic = write("sample.py", b"alpha = 1\nbeta = 2\ngamma = 3")
    lines = linecache.getlines(p_basic)
    print("lines:", lines)
    print("in range:", [repr(linecache.getline(p_basic, n)) for n in (1, 2, 3)])
    print("out of range:", [repr(linecache.getline(p_basic, n)) for n in (0, -1, -5, 4, 999)])
    print("bool lineno:", repr(linecache.getline(p_basic, True)))
    try:
        linecache.getline(p_basic, "1")
    except TypeError as exc:
        print("str lineno:", type(exc).__name__)
    try:
        linecache.getline(p_basic, None)
    except TypeError as exc:
        print("None lineno:", type(exc).__name__)
    print("float lineno:", end=" ")
    try:
        print(repr(linecache.getline(p_basic, 2.0)))
    except TypeError as exc:
        print(type(exc).__name__)
    st = os.stat(p_basic)
    entry = linecache.cache[p_basic]
    print("entry:", type(entry).__name__, len(entry), entry[0] == st.st_size,
          entry[1] == st.st_mtime, entry[2] is lines or entry[2] == lines,
          entry[3] == p_basic)
    print("cached lines identity:", linecache.getlines(p_basic) is linecache.cache[p_basic][2])

    p_nl = write("with_newline.py", b"one\ntwo\n")
    print("trailing newline kept:", linecache.getlines(p_nl))
    p_empty = write("empty.py", b"")
    print("empty file:", linecache.getlines(p_empty), p_empty in linecache.cache)
    print("empty getline:", repr(linecache.getline(p_empty, 1)))
    p_blank = write("blank.py", b"\n\n")
    print("blank lines:", linecache.getlines(p_blank))
    p_crlf = write("crlf.py", b"first = 1\r\nsecond = 2\r\nthird")
    print("crlf:", linecache.getlines(p_crlf))
    p_cr = write("cr.py", b"a\rb\rc")
    print("cr only:", linecache.getlines(p_cr))
    p_ff = write("ff.py", b"a\x0cb\nc\n")
    print("form feed:", linecache.getlines(p_ff))

    # 3. Pseudo / empty / missing / directory filenames.
    print("pseudo:", [linecache.getlines(n) for n in ("", "<string>", "<stdin>", "<frozen os>", "<>")])
    print("pseudo getline:", repr(linecache.getline("<string>", 1)), repr(linecache.getline("", 1)))
    print("pseudo not cached:", [n in linecache.cache for n in ("", "<string>", "<stdin>")])
    print("not pseudo:", linecache.getlines("<half"), linecache.getlines("half>"))
    print("missing:", linecache.getlines(os.path.join(tmpdir, "missing.py")))
    print("directory:", linecache.getlines(tmpdir))
    print("directory not cached:", tmpdir in linecache.cache)
    print("missing relative:", linecache.getlines("definitely_missing_3445.py"))
    print("updatecache pseudo:", linecache.updatecache("<string>"), linecache.updatecache(""))

    # 4. Encoding handling.
    p_latin = write("latin.py", b"# -*- coding: latin-1 -*-\nword = 'caf\xe9'\n")
    print("latin-1:", linecache.getlines(p_latin))
    p_bom = write("bom.py", b"\xef\xbb\xbfname = 'bom'\nsecond = 2\n")
    print("utf-8 bom:", linecache.getlines(p_bom))
    p_utf8 = write("utf8.py", "name = 'café あ'\n".encode("utf-8"))
    print("utf-8:", linecache.getlines(p_utf8))
    p_cp = write("cp1252.py", b"# coding: cp1252\nprice = '\x80 5'\n")
    print("cp1252:", linecache.getlines(p_cp))
    p_bad = write("bad_utf8.py", b"x = '\xff\xfe'\n")
    print("bad utf-8:", linecache.getlines(p_bad), p_bad in linecache.cache)
    p_badenc = write("bad_enc.py", b"# coding: no-such-encoding-3445\nx = 1\n")
    print("unknown encoding:", linecache.getlines(p_badenc), p_badenc in linecache.cache)
    p_badcookie = write("bad_cookie.py", b"# coding: ascii\nx = '\xe9'\n")
    print("ascii cookie mismatch:", linecache.getlines(p_badcookie))

    # 5. sys.path resolution of relative names.
    sub = os.path.join(tmpdir, "pathdir")
    os.mkdir(sub)
    with open(os.path.join(sub, "rel_mod_3445.py"), "wb") as f:
        f.write(b"from_sys_path = True\nsecond_line = 42")
    print("before sys.path:", linecache.getlines("rel_mod_3445.py"))
    sys.path.insert(0, sub)
    try:
        print("sys.path lines:", linecache.getlines("rel_mod_3445.py"))
        print("sys.path entry:", rel(linecache.cache["rel_mod_3445.py"][3]))
        print("sys.path line2:", repr(linecache.getline("rel_mod_3445.py", 2)))
    finally:
        sys.path.remove(sub)
    # a non-string sys.path entry is skipped
    sys.path.insert(0, 12345)
    sys.path.insert(0, None)
    sys.path.insert(1, sub)
    linecache.clearcache()
    try:
        print("odd sys.path:", linecache.getlines("rel_mod_3445.py"))
    finally:
        sys.path.remove(None)
        sys.path.remove(12345)
        sys.path.remove(sub)
    # absolute missing paths are never searched in sys.path
    sys.path.insert(0, sub)
    try:
        print("absolute missing:", linecache.getlines(os.path.join(tmpdir, "rel_mod_3445.py")))
    finally:
        sys.path.remove(sub)

    # 6. checkcache: size/mtime changes, deletions, filename argument.
    linecache.clearcache()
    p_chk = write("chk.py", b"v1 = 1\n")
    p_chk2 = write("chk2.py", b"w1 = 1\n")
    linecache.getlines(p_chk)
    linecache.getlines(p_chk2)
    write("chk.py", b"v1 = 1\nv2 = 22\n")
    print("stale before check:", repr(linecache.getline(p_chk, 2)))
    linecache.checkcache("never-cached")
    print("unknown name:", sorted(rel(k) for k in linecache.cache))
    linecache.checkcache(p_chk)
    print("evicted by name:", sorted(rel(k) for k in linecache.cache))
    print("reloaded:", repr(linecache.getline(p_chk, 2)))
    # same size, different mtime
    old = linecache.cache[p_chk2]
    os.utime(p_chk2, (old[1] + 100, old[1] + 100))
    linecache.checkcache()
    print("mtime evicted:", sorted(rel(k) for k in linecache.cache))
    linecache.getlines(p_chk2)
    os.remove(p_chk2)
    linecache.checkcache(None)
    print("deleted evicted:", sorted(rel(k) for k in linecache.cache))
    # an entry whose real file changed but whose mtime is None is kept
    linecache.cache[p_chk] = (0, None, ["kept\n"], p_chk)
    linecache.checkcache()
    print("mtime None kept:", linecache.cache[p_chk][2])
    # fake entry pointing at a missing file with an mtime is dropped
    linecache.cache["fake"] = (1, 1.0, ["f\n"], os.path.join(tmpdir, "no_such_file"))
    linecache.checkcache("fake")
    print("fake dropped:", "fake" in linecache.cache)
    # malformed entries
    linecache.cache["bad"] = (1, 2)
    try:
        linecache.checkcache("bad")
    except ValueError as exc:
        print("short entry:", exc)
    linecache.cache["bad"] = (1, 2, 3, 4, 5)
    try:
        linecache.checkcache("bad")
    except ValueError as exc:
        print("long entry:", exc)
    del linecache.cache["bad"]

    # 7. updatecache re-reads and replaces entries.
    linecache.clearcache()
    p_up = write("up.py", b"old\n")
    print("update first:", linecache.updatecache(p_up))
    write("up.py", b"newer content\nmore")
    print("getlines stale:", linecache.getlines(p_up))
    print("update refresh:", linecache.updatecache(p_up))
    os.remove(p_up)
    print("update deleted:", linecache.updatecache(p_up), p_up in linecache.cache)
    print("update returns cached list:", linecache.updatecache(p_basic) is linecache.cache[p_basic][2])

    # 8. PEP 302 loaders through updatecache and lazycache.
    class Loader:
        def __init__(self, sources):
            self.sources = sources
            self.calls = []

        def get_source(self, name):
            self.calls.append(name)
            value = self.sources[name]
            if isinstance(value, BaseException):
                raise value
            return value

    class NoSource:
        pass

    class Spec:
        def __init__(self, name, loader):
            self.name = name
            self.loader = loader

    sources = {
        "m_text": "l1 = 1\nl2 = 2\n",
        "m_nonl": "l1 = 1\nl2 = 2",
        "m_none": None,
        "m_empty": "",
        "m_blank": "\n\n",
        "m_crlf": "a\r\nb\rc\x0cd e\n",
        "m_import": ImportError("nope"),
        "m_os": OSError("nope"),
        "m_file": FileNotFoundError("nope"),
        "m_value": ValueError("boom"),
        "m_mem": MemoryError(),
    }
    loader = Loader(sources)
    virt = os.path.join(tmpdir, "virtual_3445.py")

    def fresh():
        linecache.clearcache()
        loader.calls.clear()

    for name in ("m_text", "m_nonl", "m_none", "m_empty", "m_blank", "m_crlf"):
        fresh()
        globs = {"__name__": name, "__loader__": loader}
        result = linecache.updatecache(virt, globs)
        print("updatecache loader", name, result, loader.calls,
              entry_summary(virt) if virt in linecache.cache else "uncached")
    for name in ("m_import", "m_os", "m_file"):
        fresh()
        globs = {"__name__": name, "__loader__": loader}
        result = linecache.updatecache(virt, globs)
        print("updatecache swallowed", name, result, loader.calls,
              entry_summary(virt) if virt in linecache.cache else "uncached")
    for name in ("m_value",):
        fresh()
        globs = {"__name__": name, "__loader__": loader}
        try:
            linecache.updatecache(virt, globs)
        except ValueError as exc:
            print("updatecache propagated", name, exc, loader.calls, entry_summary(virt))
    fresh()
    print("getlines MemoryError from loader:", end=" ")
    try:
        print(linecache.getlines(virt, {"__name__": "m_mem", "__loader__": loader}))
    except MemoryError:
        print("MemoryError")
    # the thunk stays registered after an exception it swallowed
    fresh()
    print("lazy after ImportError:", linecache.updatecache(virt, {"__name__": "m_import", "__loader__": loader}),
          entry_summary(virt))
    fresh()
    print("getline via module_globals:",
          repr(linecache.getline(virt, 2, {"__name__": "m_text", "__loader__": loader})))
    print("getline cached now:", repr(linecache.getline(virt, 1)), loader.calls)

    # lazycache semantics
    fresh()
    globs = {"__name__": "m_text", "__loader__": loader}
    print("lazycache register:", linecache.lazycache(virt, globs), entry_summary(virt), loader.calls)
    print("lazycache repeat:", linecache.lazycache(virt, globs), loader.calls)
    print("lazycache other globals:", linecache.lazycache(virt, {"__name__": "other"}))
    entry = linecache.cache[virt]
    print("thunk:", isinstance(entry[0], functools.partial), entry[0].args, entry[0].func == loader.get_source)
    linecache.checkcache(virt)
    linecache.checkcache()
    print("lazy survives checkcache:", len(linecache.cache[virt]))
    print("lazy getlines:", linecache.getlines(virt), loader.calls)
    print("realised entry:", entry_summary(virt))
    print("lazycache after realised:", linecache.lazycache(virt, globs))
    print("getlines cached:", linecache.getlines(virt), loader.calls)

    fresh()
    print("lazycache invalid:", [
        linecache.lazycache("<string>", globs),
        linecache.lazycache("", globs),
        linecache.lazycache(virt, None),
        linecache.lazycache(virt, {}),
        linecache.lazycache(virt, {"__name__": "x"}),
        linecache.lazycache(virt, {"__name__": "x", "__loader__": None}),
        linecache.lazycache(virt, {"__name__": "x", "__loader__": NoSource()}),
        linecache.lazycache(virt, {"__name__": "", "__loader__": loader}),
        linecache.lazycache(virt, {"__loader__": loader}),
    ], len(linecache.cache))
    # __spec__ takes precedence over __name__ / __loader__
    fresh()
    spec_loader = Loader({"spec_name": "s1\ns2\n"})
    other_loader = Loader({"m_text": "should not be used\n"})
    globs = {"__name__": "m_text", "__loader__": other_loader,
             "__spec__": Spec("spec_name", spec_loader)}
    print("lazycache spec:", linecache.lazycache(virt, globs))
    print("spec lines:", linecache.getlines(virt), spec_loader.calls, other_loader.calls)
    fresh()
    globs = {"__name__": "m_text", "__loader__": loader, "__spec__": Spec("", None)}
    print("empty spec name falls back:", linecache.lazycache(virt, globs), linecache.getlines(virt), loader.calls)
    fresh()
    globs = {"__name__": "m_text", "__loader__": loader, "__spec__": None}
    print("None spec:", linecache.lazycache(virt, globs), linecache.getlines(virt))
    fresh()
    globs = {"__name__": "m_text", "__loader__": other_loader, "__spec__": Spec("spec_name", None)}
    print("spec loader None -> __loader__:", linecache.lazycache(virt, globs), other_loader.calls)
    # lazycache on an existing file entry leaves it alone
    fresh()
    linecache.getlines(p_basic)
    before = linecache.cache[p_basic]
    print("lazycache on loaded:", linecache.lazycache(p_basic, {"__name__": "m_text", "__loader__": loader}),
          linecache.cache[p_basic] is before)
    # an existing real file wins over the loader
    fresh()
    print("real file beats loader:", linecache.getlines(p_basic, {"__name__": "m_text", "__loader__": loader}),
          loader.calls)
    # a lazy entry for a file that exists on disk is replaced by the file
    fresh()
    linecache.lazycache(p_basic, {"__name__": "m_text", "__loader__": loader})
    print("lazy then file:", linecache.getlines(p_basic), loader.calls, len(linecache.cache[p_basic]))
    # a lazy entry is kept (and reused) when the thunk fails with ImportError
    fresh()
    globs = {"__name__": "m_import", "__loader__": loader}
    linecache.lazycache(virt, globs)
    print("failing lazy:", linecache.getlines(virt, globs), linecache.getlines(virt, globs), loader.calls)

    # 9. Argument handling.
    for label, call in [
        ("getline()", lambda: linecache.getline()),
        ("getline(1)", lambda: linecache.getline("a")),
        ("getlines()", lambda: linecache.getlines()),
        ("lazycache(1)", lambda: linecache.lazycache("a")),
        ("updatecache()", lambda: linecache.updatecache()),
        ("clearcache(1)", lambda: linecache.clearcache(1)),
        ("getlines(unhashable)", lambda: linecache.getlines([])),
        ("getlines(int)", lambda: linecache.getlines(5)),
        ("getlines(None)", lambda: linecache.getlines(None)),
    ]:
        try:
            print(label, "->", call())
        except Exception as exc:
            print(label, "->", type(exc).__name__)
    print("keyword args:",
          linecache.getline(filename=p_basic, lineno=1),
          linecache.getlines(filename=p_basic, module_globals=None) == linecache.getlines(p_basic))
    print("return values:", linecache.clearcache(), linecache.checkcache())

    # 10. Patching a function is visible to its callers, like in CPython.
    linecache.clearcache()
    real_update = linecache.updatecache

    def raise_memory(*args, **kwargs):
        raise MemoryError

    linecache.cache["keep"] = (1, None, ["k\n"], "keep")
    linecache.updatecache = raise_memory
    try:
        print("memory error handled:", linecache.getlines(p_basic), len(linecache.cache))
    finally:
        linecache.updatecache = real_update
    real_getlines = linecache.getlines
    linecache.getlines = lambda filename, module_globals=None: ["patched\n"]
    try:
        print("patched getlines:", repr(linecache.getline("anything", 1)))
    finally:
        linecache.getlines = real_getlines
    print("restored:", hasattr(linecache, "getlines"))

    # a rebound cache is the one the functions use
    original_cache = linecache.cache
    linecache.cache = {"rebound": (1, None, ["r\n"], "rebound")}
    print("rebound cache:", linecache.getlines("rebound"))
    linecache.clearcache()
    print("rebound cleared:", linecache.cache, len(original_cache) > 0)
    linecache.cache = original_cache
    linecache.clearcache()

    # 11. Integration with traceback.
    tb_script = write("tb_target.py", b"def trigger():\n    raise ValueError('from_tb_target')\n")
    ns = {}
    with open(tb_script, "r", encoding="utf-8") as f:
        exec(compile(f.read(), tb_script, "exec"), ns)
    try:
        ns["trigger"]()
    except ValueError as exc:
        frames = traceback.extract_tb(exc.__traceback__)
        print("traceback frames:", [(rel(f.filename), f.name, f.lineno, f.line) for f in frames[1:]])
        print("traceback cached:", tb_script in linecache.cache)
        print("format_exception:", [line.replace(tmpdir, "<TMP>").replace(os.sep, "/")
                                    for line in traceback.format_exception(exc)[-2:]])
    linecache.clearcache()
    # a swallowed loader error must not leak frames into a later traceback
    linecache.getlines(tmpdir)
    linecache.getlines(p_bad)
    linecache.getlines(p_badenc)
    try:
        ns["trigger"]()
    except ValueError as exc:
        frames = traceback.extract_tb(exc.__traceback__)
        print("clean traceback:", [(f.name, f.lineno) for f in frames[1:]])
    # ... also when linecache runs inside an except handler
    try:
        ns["trigger"]()
    except ValueError as exc:
        linecache.getlines(tmpdir)
        linecache.getlines(p_bad)
        frames = traceback.extract_tb(exc.__traceback__)
        print("handler traceback:", [(f.name, f.lineno) for f in frames[1:]])
    # traceback.print_exception populates the cache from module globals
    linecache.clearcache()
    g = {"__name__": "m_text", "__loader__": loader}
    print("getline with globals, missing file:",
          repr(linecache.getline(os.path.join(tmpdir, "nowhere.py"), 1, g)))

    linecache.clearcache()
print("done")
