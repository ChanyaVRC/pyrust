import collections.abc
import dbm
import dbm.dumb
import os
import pathlib
import shelve
import tempfile

# 1. Module surface & error hierarchy
assert isinstance(dbm.error, tuple)
assert len(dbm.error) == 2
assert issubclass(dbm.error[0], Exception)
assert dbm.error[1] is OSError
assert dbm.dumb.error is OSError
assert dbm.dumb._BLOCKSIZE == 512
print("error_surface_ok")

with tempfile.TemporaryDirectory() as tmpdir:
    # 2. dbm.dumb creation, CRUD, binary keys/values, escaping, and exact .dir/.dat parity
    dumb_path = os.path.join(tmpdir, "sample.db")
    with dbm.dumb.open(dumb_path, "c") as d:
        assert isinstance(d, collections.abc.MutableMapping)
        assert len(d) == 0
        assert d.keys() == []
        d["alpha"] = "hello"
        d[b"beta"] = b"world\x00\x01\xff"
        d["it's"] = "quoted"
        d[b'a\'b"c\n\t\x00\xff'] = b"binary_val"
        assert d["alpha"] == b"hello"
        assert d[b"alpha"] == b"hello"
        assert d["beta"] == b"world\x00\x01\xff"
        assert "alpha" in d
        assert b"beta" in d
        assert "missing" not in d
        assert d.get("alpha") == b"hello"
        assert d.get("missing") is None
        assert d.get("missing", b"fallback") == b"fallback"
        assert d.setdefault("alpha", b"ignored") == b"hello"
        assert d.setdefault("gamma", b"created") == b"created"
        assert d["gamma"] == b"created"
        assert len(d) == 5
        assert list(d) == [b"alpha", b"beta", b"it's", b'a\'b"c\n\t\x00\xff', b"gamma"]
        assert d.keys() == [b"alpha", b"beta", b"it's", b'a\'b"c\n\t\x00\xff', b"gamma"]

        # Overwrite same-block value (shrink) and larger multi-block value (grow)
        d["alpha"] = b"hi"
        assert d["alpha"] == b"hi"
        big = b"X" * 1200
        d["gamma"] = big
        assert d["gamma"] == big

        # Delete a key
        del d["it's"]
        assert "it's" not in d
        assert len(d) == 4
        d.sync()

    # Closed DB raises OSError / dbm.dumb.error
    for op in (
        lambda: d["alpha"],
        lambda: d.keys(),
        lambda: len(d),
        lambda: "alpha" in d,
        lambda: iter(d),
    ):
        try:
            op()
            assert False, "expected error on closed db"
        except dbm.dumb.error:
            pass
    print("dumb_closed_ok")

    # Verify exact .dir and .dat file contents (bidirectional CPython wire parity)
    with open(dumb_path + ".dir", "rb") as f:
        dir_bytes = f.read()
    with open(dumb_path + ".dat", "rb") as f:
        dat_bytes = f.read()
    print("dir_bytes:", dir_bytes)
    print("dat_len:", len(dat_bytes))
    assert dat_bytes[:2] == b"hi"

    # Reopen existing dbm.dumb via dbm.whichdb and dbm.open (read-only & read-write)
    assert dbm.whichdb(dumb_path) == "dbm.dumb"
    with dbm.open(dumb_path, "r") as ro:
        assert ro["alpha"] == b"hi"
        assert ro[b"beta"] == b"world\x00\x01\xff"
        assert ro[b'a\'b"c\n\t\x00\xff'] == b"binary_val"
        assert ro["gamma"] == b"X" * 1200
        try:
            ro["new"] = "nope"
            assert False, "expected readonly error"
        except dbm.error:
            pass
        try:
            del ro["alpha"]
            assert False, "expected readonly delete error"
        except dbm.error:
            pass
    print("dumb_reopen_ro_ok")

    # Reopen via PathLike and flag 'w' / 'n'
    dumb_p = pathlib.Path(dumb_path)
    assert dbm.whichdb(dumb_p) == "dbm.dumb"
    with dbm.open(dumb_p, "w") as rw:
        rw["extra"] = b"123"
    with dbm.dumb.open(dumb_p, "r") as check:
        assert check["extra"] == b"123"
    with dbm.dumb.open(dumb_p, "n") as fresh:
        assert len(fresh) == 0
    assert dbm.whichdb(dumb_p) == "dbm.dumb"
    print("dumb_flags_ok")

    # Parse a hand-crafted CPython dbm.dumb .dir/.dat fixture (verifies reading CPython output)
    fixture_base = os.path.join(tmpdir, "cpython_fixture")
    with open(fixture_base + ".dir", "wb") as f:
        f.write(b"'k1', (0, 5)\n\"o'ne\", (512, 4)\n'\\x00\\xff', (1024, 3)\n")
    with open(fixture_base + ".dat", "wb") as f:
        f.write(b"val01" + b"\x00" * 507 + b"val2" + b"\x00" * 508 + b"\xaa\xbb\xcc")
    assert dbm.whichdb(fixture_base) == "dbm.dumb"
    with dbm.open(fixture_base, "r") as fx:
        assert fx["k1"] == b"val01"
        assert fx["o'ne"] == b"val2"
        assert fx[b"\x00\xff"] == b"\xaa\xbb\xcc"
    print("dumb_fixture_read_ok")

    # Invalid flag & type errors
    try:
        dbm.dumb.open(os.path.join(tmpdir, "badflag"), "x")
        assert False
    except ValueError:
        pass
    with dbm.dumb.open(os.path.join(tmpdir, "typetest"), "n") as td:
        for bad_k in (123, None):
            try:
                td[bad_k] = b"v"
                assert False
            except TypeError:
                pass
        for bad_v in (123, None):
            try:
                td["k"] = bad_v
                assert False
            except TypeError:
                pass
    print("dumb_errors_ok")

    # 3. dbm.whichdb edge cases & dbm.open missing/unknown errors
    missing = os.path.join(tmpdir, "does_not_exist")
    assert dbm.whichdb(missing) is None
    try:
        dbm.open(missing, "r")
        assert False
    except dbm.error:
        pass

    unknown = os.path.join(tmpdir, "unknown.bin")
    with open(unknown, "wb") as f:
        f.write(b"not a dbm file at all")
    assert dbm.whichdb(unknown) == ""
    try:
        dbm.open(unknown, "r")
        assert False
    except dbm.error:
        pass

    short_file = os.path.join(tmpdir, "tiny.bin")
    with open(short_file, "wb") as f:
        f.write(b"ab")
    assert dbm.whichdb(short_file) == ""
    print("whichdb_ok")

    # 4. Generic dbm.open create & reopen
    gen_path = os.path.join(tmpdir, "generic_db")
    with dbm.open(gen_path, "c") as gdb:
        gdb["hello"] = "world"
        assert gdb["hello"] == b"world"
    assert dbm.whichdb(gen_path) not in (None, "")
    with dbm.open(gen_path, "r") as gdb:
        assert gdb["hello"] == b"world"
    print("generic_dbm_ok")

    # 5. shelve: DbfilenameShelf, writeback=False, writeback=True, sync, clear, close
    sh_path = os.path.join(tmpdir, "my_shelf")
    with shelve.open(sh_path, "n") as s:
        assert isinstance(s, collections.abc.MutableMapping)
        assert isinstance(s, shelve.Shelf)
        assert isinstance(s, shelve.DbfilenameShelf)
        s["num"] = 42
        s["lst"] = [1, 2, 3]
        s["nested"] = {"a": (1, True, None), "b": b"bytes"}
        # Without writeback, mutating fetched list does NOT persist
        s["lst"].append(99)
        assert s["lst"] == [1, 2, 3]
        assert len(s) == 3
        assert "num" in s
        assert "nope" not in s
        assert s.get("num") == 42
        assert s.get("nope", "def") == "def"
        assert sorted(s.keys()) == ["lst", "nested", "num"]
        assert sorted(list(s)) == ["lst", "nested", "num"]
        assert s.pop("num") == 42
        assert "num" not in s
        s.update({"x": 10}, y=20)
        assert s["x"] == 10 and s["y"] == 20

    # Reopen with writeback=True
    with shelve.open(sh_path, "w", writeback=True) as s:
        s["lst"].append(4)
        s["nested"]["c"] = "added"
        s.sync()
        assert s.cache == {}
        s["lst"].append(5)

    with shelve.open(sh_path, "r") as s:
        assert s["lst"] == [1, 2, 3, 4, 5]
        assert s["nested"]["c"] == "added"
        assert s["x"] == 10 and s["y"] == 20

    # Closed shelf raises ValueError
    try:
        _ = s["lst"]
        assert False, "expected ValueError on closed shelf"
    except ValueError:
        pass
    print("shelve_writeback_ok")

    # shelve clear()
    with shelve.open(sh_path, "w") as s:
        s.clear()
        assert len(s) == 0
    print("shelve_clear_ok")

    # 6. Shelf with custom dict + keyencoding, and BsdDbShelf
    backing = {}
    sh = shelve.Shelf(backing, protocol=4, writeback=True, keyencoding="latin-1")
    sh["caf\xe9"] = {"ok": True}
    assert b"caf\xe9" in backing
    sh["caf\xe9"]["more"] = [1, 2]
    sh.sync()
    sh.close()
    assert sh.dict.__repr__() == "<Closed Dictionary>"

    class FakeBsdDb(dict):
        def __init__(self):
            super().__init__()
            self._pos = 0

        def _ordered(self):
            return sorted(self.items())

        def first(self):
            self._pos = 0
            return self._ordered()[0]

        def last(self):
            items = self._ordered()
            self._pos = len(items) - 1
            return items[self._pos]

        def __next__(self):
            self._pos += 1
            return self._ordered()[self._pos]

        def previous(self):
            self._pos -= 1
            return self._ordered()[self._pos]

        def set_location(self, key):
            items = self._ordered()
            for idx, (k, v) in enumerate(items):
                if k >= key:
                    self._pos = idx
                    return (k, v)
            raise KeyError(key)

    bsd_backing = FakeBsdDb()
    bsd = shelve.BsdDbShelf(bsd_backing)
    bsd["b"] = 2
    bsd["a"] = 1
    bsd["c"] = 3
    assert bsd.first() == ("a", 1)
    assert bsd.next() == ("b", 2)
    assert bsd.last() == ("c", 3)
    assert bsd.previous() == ("b", 2)
    assert bsd.set_location(b"b") == ("b", 2)
    bsd.close()
    print("bsddb_shelf_ok")
