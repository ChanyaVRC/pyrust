# CPython 3.12 parity tests for the `linecache` module (issue #3229).

import linecache
import os
import sys
import tempfile
import traceback

# 1. Module metadata and __all__
print("in sys.modules:", "linecache" in sys.modules)
print("__all__:", linecache.__all__)
for name in linecache.__all__:
    assert hasattr(linecache, name), f"missing {name}"
assert isinstance(linecache.cache, dict)

with tempfile.TemporaryDirectory() as tmpdir:
    linecache.clearcache()
    assert len(linecache.cache) == 0

    # 2. Direct filesystem lookup, trailing newline synthesis, UTF-8, CRLF
    p_basic = os.path.join(tmpdir, "sample.py")
    with open(p_basic, "w", encoding="utf-8", newline="\n") as f:
        f.write("alpha = 1\nbeta = 2\ngamma = 3")  # no trailing newline

    lines = linecache.getlines(p_basic)
    print("basic lines:", lines)
    print(
        "getline in-range:",
        repr(linecache.getline(p_basic, 1)),
        repr(linecache.getline(p_basic, 2)),
        repr(linecache.getline(p_basic, 3)),
    )
    print(
        "getline out-of-range:",
        repr(linecache.getline(p_basic, 0)),
        repr(linecache.getline(p_basic, -5)),
        repr(linecache.getline(p_basic, 4)),
        repr(linecache.getline(p_basic, 999)),
    )

    # Cache entry structure: (size, mtime, lines, fullname)
    assert p_basic in linecache.cache
    entry = linecache.cache[p_basic]
    st = os.stat(p_basic)
    print(
        "cache entry valid:",
        len(entry),
        entry[0] == st.st_size,
        entry[1] == st.st_mtime,
        entry[2] == lines,
        entry[3] == p_basic,
    )

    # CRLF file
    p_crlf = os.path.join(tmpdir, "crlf.py")
    with open(p_crlf, "wb") as f:
        f.write(b"first = 1\r\nsecond = 2\r\n")
    print("crlf lines:", linecache.getlines(p_crlf))

    # PEP 263 coding cookie (latin-1)
    p_latin = os.path.join(tmpdir, "latin.py")
    with open(p_latin, "wb") as f:
        f.write(b"# -*- coding: latin-1 -*-\nword = 'caf\xe9'\n")
    print("latin1 line2:", repr(linecache.getline(p_latin, 2)))

    # Missing, pseudo, empty, and directory filenames
    p_missing = os.path.join(tmpdir, "does_not_exist.py")
    print(
        "missing/pseudo:",
        repr(linecache.getline(p_missing, 1)),
        linecache.getlines(p_missing),
        repr(linecache.getline("", 1)),
        repr(linecache.getline("<string>", 1)),
        repr(linecache.getline("<stdin>", 1)),
        linecache.getlines(tmpdir),
    )

    # 3. sys.path relative filename resolution
    p_rel_name = "rel_mod_3229.py"
    p_rel_full = os.path.join(tmpdir, p_rel_name)
    with open(p_rel_full, "w", encoding="utf-8", newline="\n") as f:
        f.write("from_sys_path = True\nsecond_line = 42\n")

    sys.path.insert(0, tmpdir)
    try:
        rel_lines = linecache.getlines(p_rel_name)
        print("sys.path lines:", rel_lines)
        print("sys.path line2:", repr(linecache.getline(p_rel_name, 2)))
        assert linecache.cache[p_rel_name][3] == p_rel_full
    finally:
        sys.path.remove(tmpdir)

    # 4. checkcache() and updatecache() on file modification and deletion
    # Modify file with a different byte size so stat.st_size definitely changes
    with open(p_basic, "w", encoding="utf-8", newline="\n") as f:
        f.write("alpha_updated = 100\nbeta_updated = 200\n")
    # Before checkcache, cached value is still returned
    print("before checkcache:", repr(linecache.getline(p_basic, 1)))
    linecache.checkcache(p_basic)
    assert p_basic not in linecache.cache
    print("after checkcache:", repr(linecache.getline(p_basic, 1)))

    # updatecache refreshes explicitly
    with open(p_basic, "w", encoding="utf-8", newline="\n") as f:
        f.write("refreshed_line = 999\n")
    updated = linecache.updatecache(p_basic)
    print("after updatecache:", updated)

    # Deleting a cached file causes checkcache() to evict it
    os.remove(p_crlf)
    linecache.checkcache()
    print("deleted evicted:", p_crlf not in linecache.cache)

    # 5. lazycache() with __loader__ and __spec__.loader
    class FakeLoader:
        def __init__(self, sources):
            self.sources = sources
            self.calls = []

        def get_source(self, name):
            self.calls.append(name)
            return self.sources.get(name)

    class FakeSpec:
        def __init__(self, name, loader):
            self.name = name
            self.loader = loader

    loader = FakeLoader({"virtual_mod": "v1 = 10\nv2 = 20\n", "empty_mod": None})
    v_fname = os.path.join(tmpdir, "nonexistent_virtual_3229.py")
    g_loader = {"__name__": "virtual_mod", "__loader__": loader}
    registered = linecache.lazycache(v_fname, g_loader)
    print("lazycache registered:", registered, len(linecache.cache[v_fname]), loader.calls)
    # Re-registering an existing lazy entry returns True without calling loader
    print("lazycache repeat:", linecache.lazycache(v_fname, g_loader), loader.calls)
    # checkcache leaves lazy entries alone
    linecache.checkcache(v_fname)
    print("still lazy after checkcache:", len(linecache.cache[v_fname]))
    # getlines realizes the thunk
    v_lines = linecache.getlines(v_fname)
    print("lazy realized:", v_lines, loader.calls)
    print("realized mtime is None:", linecache.cache[v_fname][1] is None)
    # Re-registering an already realized entry returns False
    print("lazycache after realize:", linecache.lazycache(v_fname, g_loader))
    # checkcache leaves mtime=None entries alone
    linecache.checkcache(v_fname)
    print("kept after checkcache:", v_fname in linecache.cache)

    # lazycache via __spec__.loader
    v_fname2 = os.path.join(tmpdir, "nonexistent_spec_3229.py")
    g_spec = {"__name__": "ignored_name", "__spec__": FakeSpec("virtual_mod", loader)}
    print("lazycache spec:", linecache.lazycache(v_fname2, g_spec))
    print("lazycache spec line2:", repr(linecache.getline(v_fname2, 2)))

    # lazycache where get_source returns None
    v_fname3 = os.path.join(tmpdir, "nonexistent_none_3229.py")
    g_none = {"__name__": "empty_mod", "__loader__": loader}
    print("lazycache none source:", linecache.getlines(v_fname3, g_none))

    # lazycache failure cases
    print(
        "lazycache invalid:",
        linecache.lazycache("<pseudo>", g_loader),
        linecache.lazycache("", g_loader),
        linecache.lazycache(os.path.join(tmpdir, "no_loader.py"), {"__name__": "x"}),
        linecache.lazycache(os.path.join(tmpdir, "no_globals.py"), None),
    )

    # 6. clearcache()
    linecache.clearcache()
    print("after clearcache:", len(linecache.cache))

    # 7. Integration with traceback
    tb_script = os.path.join(tmpdir, "tb_target.py")
    with open(tb_script, "w", encoding="utf-8", newline="\n") as f:
        f.write("def trigger():\n    raise ValueError('from_tb_target')\n")
    ns = {}
    with open(tb_script, "r", encoding="utf-8") as f:
        exec(compile(f.read(), tb_script, "exec"), ns)
    try:
        ns["trigger"]()
    except ValueError as exc:
        frames = traceback.extract_tb(exc.__traceback__)
        print("traceback integrated line:", frames[-1].name, repr(frames[-1].line))
        assert tb_script in linecache.cache

    linecache.clearcache()
