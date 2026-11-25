# Parity test for Python 3.12 sqlite3 DB-API 2.0 module (issue #3225).

import collections.abc
import datetime
import io
import os
import pathlib
import sqlite3
import sqlite3.dbapi2
import tempfile


# ── 1. Module globals, constants, and sqlite3.dbapi2 ─────────────────────────
print("apilevel:", sqlite3.apilevel)
print("paramstyle:", sqlite3.paramstyle)
print("threadsafety_ok:", sqlite3.threadsafety in (0, 1, 3))
print("parse_flags:", sqlite3.PARSE_DECLTYPES, sqlite3.PARSE_COLNAMES)
print("legacy_tc:", sqlite3.LEGACY_TRANSACTION_CONTROL)
print(
    "version_ok:",
    isinstance(sqlite3.sqlite_version, str),
    isinstance(sqlite3.sqlite_version_info, tuple),
    len(sqlite3.sqlite_version_info) == 3,
    all(isinstance(x, int) for x in sqlite3.sqlite_version_info),
    sqlite3.sqlite_version == ".".join(str(x) for x in sqlite3.sqlite_version_info),
    sqlite3.sqlite_version_info >= (3, 7, 15),
)
print(
    "constants:",
    sqlite3.SQLITE_OK,
    sqlite3.SQLITE_ERROR,
    sqlite3.SQLITE_CONSTRAINT,
    sqlite3.SQLITE_DENY,
    sqlite3.SQLITE_IGNORE,
    sqlite3.SQLITE_ROW,
    sqlite3.SQLITE_DONE,
)
print("dbapi2_same:", sqlite3.dbapi2.Connection is sqlite3.Connection, sqlite3.dbapi2.Row is sqlite3.Row)
print("types:", sqlite3.Date is datetime.date, sqlite3.Time is datetime.time, sqlite3.Timestamp is datetime.datetime, sqlite3.Binary is memoryview)
print("fromticks:", sqlite3.DateFromTicks(86400 * 100) == datetime.date(*__import__("time").localtime(86400 * 100)[:3]))


# ── 2. Exception hierarchy & error attributes ────────────────────────────────
print(
    "exc_hierarchy:",
    issubclass(sqlite3.Warning, Exception),
    issubclass(sqlite3.Error, Exception),
    issubclass(sqlite3.InterfaceError, sqlite3.Error),
    issubclass(sqlite3.DatabaseError, sqlite3.Error),
    issubclass(sqlite3.DataError, sqlite3.DatabaseError),
    issubclass(sqlite3.OperationalError, sqlite3.DatabaseError),
    issubclass(sqlite3.IntegrityError, sqlite3.DatabaseError),
    issubclass(sqlite3.InternalError, sqlite3.DatabaseError),
    issubclass(sqlite3.ProgrammingError, sqlite3.DatabaseError),
    issubclass(sqlite3.NotSupportedError, sqlite3.DatabaseError),
)

con = sqlite3.connect(":memory:")
print(
    "conn_excs:",
    con.Warning is sqlite3.Warning,
    con.Error is sqlite3.Error,
    con.IntegrityError is sqlite3.IntegrityError,
    con.OperationalError is sqlite3.OperationalError,
    con.ProgrammingError is sqlite3.ProgrammingError,
)

con.execute("CREATE TABLE err_t (id INTEGER PRIMARY KEY, u TEXT NOT NULL UNIQUE)")
con.execute("INSERT INTO err_t VALUES (1, 'a')")

for label, sql, params in [
    ("unique", "INSERT INTO err_t VALUES (2, 'a')", ()),
    ("notnull", "INSERT INTO err_t VALUES (2, NULL)", ()),
    ("mismatch", "INSERT INTO err_t VALUES ('bad_int', 'b')", ()),
    ("notable", "SELECT * FROM no_such_table", ()),
    ("syntax", "THIS IS NOT SQL", ()),
]:
    try:
        con.execute(sql, params)
    except sqlite3.Error as e:
        print("err:", label, type(e).__name__, e.sqlite_errorcode, e.sqlite_errorname)

manual_err = sqlite3.OperationalError("manual")
print("manual_err:", manual_err.args, hasattr(manual_err, "sqlite_errorcode"), hasattr(manual_err, "sqlite_errorname"))
con.close()


# ── 3. DDL, DML, type roundtrip, parameters, metadata ────────────────────────
con = sqlite3.connect(":memory:")
cur = con.cursor()
print("cur_init:", cur.connection is con, cur.description, cur.rowcount, cur.lastrowid, cur.arraysize)

cur.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, i INT, f REAL, s TEXT, b BLOB, n TEXT)")
print("after_ddl:", cur.description, cur.rowcount, cur.lastrowid)

big_i64 = (1 << 62) + 123456789
neg_i64 = -(1 << 62) - 987654321
cur.execute(
    "INSERT INTO items (i, f, s, b, n) VALUES (?, ?, ?, ?, ?)",
    (big_i64, 3.25, "hello 世界", b"\x00\x01\xff", None),
)
print("after_ins1:", cur.rowcount, cur.lastrowid, con.total_changes)

cur.execute(
    "INSERT INTO items (i, f, s, b, n) VALUES (?, ?, ?, ?, ?)",
    [neg_i64, -0.5, "", b"", True],
)
print("after_ins2:", cur.rowcount, cur.lastrowid)

# Named parameters (:name, @name, $name) with extra unused key
cur.execute(
    "INSERT INTO items (i, f, s, b, n) VALUES (:i, @f, $s, :b, :n)",
    {"i": False, "f": 0.0, "s": "named", "b": bytearray(b"ba"), "n": "ok", "unused": 99},
)
print("after_named:", cur.rowcount, cur.lastrowid)

# Empty / whitespace / comment-only SQL
cur.execute("   -- just a comment\n")
print("after_empty:", cur.description, cur.rowcount)

cur.execute("SELECT id, i, f, s, b, n FROM items ORDER BY id")
print("sel_desc:", cur.description)
print("sel_rowcount:", cur.rowcount)
print("row1:", cur.fetchone())
cur.arraysize = 2
print("rows23:", cur.fetchmany())
print("after_end:", cur.fetchone(), cur.fetchall(), cur.fetchmany(5))

# Iterator protocol & concurrent cursors
c1 = con.execute("SELECT id FROM items ORDER BY id")
c2 = con.execute("SELECT s FROM items ORDER BY id DESC")
print("concurrent:", c1.fetchone(), c2.fetchone(), list(c1), list(c2))

# executemany with generator & dict rows
cur.executemany(
    "INSERT INTO items (i, f, s, b, n) VALUES (?, ?, ?, ?, ?)",
    ((k, float(k), f"m{k}", b"x", None) for k in range(3)),
)
print("many_pos:", cur.rowcount)
cur.executemany(
    "INSERT INTO items (i, f, s, b, n) VALUES (:i, 0.0, :s, X'', NULL)",
    [{"i": 10, "s": "d0"}, {"i": 11, "s": "d1"}],
)
print("many_dict:", cur.rowcount)

cur.execute("UPDATE items SET s = 'upd' WHERE i >= 10")
print("upd_rc:", cur.rowcount)
cur.execute("DELETE FROM items WHERE i >= 10")
print("del_rc:", cur.rowcount)

# Parameter & statement error cases
for label, fn in [
    ("wrong_count", lambda: cur.execute("SELECT ?, ?", (1,))),
    ("missing_named", lambda: cur.execute("SELECT :a, :b", {"a": 1})),
    ("dict_with_qmark", lambda: cur.execute("SELECT ?", {"a": 1})),
    ("bad_param_type", lambda: cur.execute("SELECT ?", (object(),))),
    ("multi_stmt", lambda: cur.execute("SELECT 1; SELECT 2")),
    ("many_select", lambda: cur.executemany("SELECT ?", [(1,), (2,)])),
    ("int_overflow", lambda: cur.execute("SELECT ?", (1 << 80,))),
]:
    try:
        fn()
    except Exception as e:
        print("param_err:", label, type(e).__name__)

# executescript
con.execute("DELETE FROM items")
con.execute("INSERT INTO items (i, f, s, b, n) VALUES (1, 1.0, 'pre', X'', NULL)")
print("before_script_tx:", con.in_transaction)
cur.executescript(
    """
    INSERT INTO items (i, f, s, b, n) VALUES (2, 2.0, 's1', X'', NULL);
    INSERT INTO items (i, f, s, b, n) VALUES (3, 3.0, 's2', X'', NULL);
    """
)
print("after_script:", con.in_transaction, [r[0] for r in con.execute("SELECT i FROM items ORDER BY i")])
con.close()


# ── 4. Transactions: commit, rollback, context manager, isolation_level, autocommit ─
con = sqlite3.connect(":memory:")
con.execute("CREATE TABLE tx (v INT)")
print("tx_after_ddl:", con.in_transaction)
con.execute("INSERT INTO tx VALUES (1)")
print("tx_after_dml:", con.in_transaction)
con.rollback()
print("tx_after_rb:", con.in_transaction, con.execute("SELECT * FROM tx").fetchall())

with con:
    con.execute("INSERT INTO tx VALUES (10)")
print("with_ok:", con.in_transaction, con.execute("SELECT * FROM tx").fetchall())

try:
    with con:
        con.execute("INSERT INTO tx VALUES (20)")
        raise ValueError("abort tx")
except ValueError:
    pass
print("with_err:", con.in_transaction, con.execute("SELECT * FROM tx").fetchall())

# isolation_level=None (legacy autocommit)
con.isolation_level = None
con.execute("INSERT INTO tx VALUES (30)")
print("iso_none:", con.isolation_level, con.in_transaction, con.execute("SELECT * FROM tx ORDER BY v").fetchall())

for valid_iso in ("", "DEFERRED", "IMMEDIATE", "EXCLUSIVE", "deferred"):
    con.isolation_level = valid_iso
try:
    con.isolation_level = "BOGUS"
except ValueError:
    print("bad_iso: ValueError")
con.close()

# PEP 249 autocommit=True and autocommit=False
c_ac_true = sqlite3.connect(":memory:", autocommit=True)
c_ac_true.execute("CREATE TABLE t (x INT)")
c_ac_true.execute("INSERT INTO t VALUES (1)")
print("ac_true:", c_ac_true.autocommit, c_ac_true.in_transaction)
c_ac_true.close()

c_ac_false = sqlite3.connect(":memory:", autocommit=False)
print("ac_false_init:", c_ac_false.autocommit, c_ac_false.in_transaction)
c_ac_false.execute("CREATE TABLE t (x INT)")
c_ac_false.execute("INSERT INTO t VALUES (1)")
c_ac_false.rollback()
print("ac_false_rb:", c_ac_false.in_transaction, c_ac_false.execute("SELECT * FROM sqlite_master").fetchall())
c_ac_false.execute("CREATE TABLE t (x INT)")
c_ac_false.execute("INSERT INTO t VALUES (2)")
c_ac_false.commit()
print("ac_false_commit:", c_ac_false.in_transaction, c_ac_false.execute("SELECT * FROM t").fetchall())
c_ac_false.close()


# ── 5. Row factory & text_factory ────────────────────────────────────────────
con = sqlite3.connect(":memory:")
cur_before = con.cursor()
con.row_factory = sqlite3.Row
cur_after = con.cursor()
print("rf_inherit:", cur_before.row_factory is None, cur_after.row_factory is sqlite3.Row)

row = con.execute("SELECT 10 AS Alpha, 'hi' AS Beta, 2.5 AS Gamma").fetchone()
row2 = con.execute("SELECT 10 AS Alpha, 'hi' AS Beta, 2.5 AS Gamma").fetchone()
row3 = con.execute("SELECT 10 AS alpha, 'hi' AS Beta, 2.5 AS Gamma").fetchone()
print("row_seq:", isinstance(row, collections.abc.Sequence), len(row), row.keys())
print("row_idx:", row[0], row[-1], row[1:3], row[::-1])
print("row_key:", row["Alpha"], row["alpha"], row["ALPHA"], row["BETA"])
print("row_iter:", list(row))
print("row_eq:", row == row2, row == row3, row == (10, "hi", 2.5), row != row3, hash(row) == hash(row2))
try:
    _ = row["missing"]
except IndexError as e:
    print("row_missing:", str(e))
try:
    _ = row[99]
except IndexError:
    print("row_oob: IndexError")

# Direct Row construction
raw_cur = cur_before.execute("SELECT 7 AS num, 'z' AS ch")
direct_row = sqlite3.Row(raw_cur, (7, "z"))
print("row_direct:", direct_row["num"], direct_row["CH"], direct_row.keys())

# text_factory
con.row_factory = None
con.execute("CREATE TABLE tf (x TEXT)")
con.execute("INSERT INTO tf VALUES (?)", (b"\xff\xfe",))
try:
    con.execute("SELECT CAST(x AS TEXT) FROM tf").fetchall()
except sqlite3.OperationalError:
    print("tf_bad_utf8: OperationalError")
con.text_factory = bytes
print("tf_bytes:", con.execute("SELECT 'abc'").fetchall())
con.text_factory = lambda b: b.decode("latin1")
print("tf_custom:", con.execute("SELECT CAST(x AS TEXT) FROM tf").fetchall())
con.close()


# ── 6. create_function, create_aggregate, create_collation ───────────────────
con = sqlite3.connect(":memory:")
con.create_function("my_add", 2, lambda a, b: a + b, deterministic=True)
con.create_function("my_sum", -1, lambda *xs: sum(xs))
con.create_function("my_none", 0, lambda: None)
con.create_function("my_blob", 1, lambda b: b[::-1])
print(
    "udfs:",
    con.execute("SELECT my_add(3, 4), my_sum(1, 2, 3, 4), my_none(), my_blob(X'010203')").fetchone(),
)

# Deterministic function usable in generated index / WHERE
con.execute("CREATE TABLE fn_t (x INT)")
con.execute("CREATE INDEX idx_fn ON fn_t (my_add(x, 1))")
con.execute("INSERT INTO fn_t VALUES (5), (10)")
print("det_idx:", con.execute("SELECT my_add(x, 1) FROM fn_t ORDER BY x").fetchall())

# UDF exception -> OperationalError
def boom(x):
    raise ValueError("udf boom")

con.create_function("boom", 1, boom)
try:
    con.execute("SELECT boom(1)").fetchall()
except sqlite3.OperationalError as e:
    print("udf_err:", type(e).__name__, e.sqlite_errorcode, e.sqlite_errorname)

# Unregister UDF
con.create_function("my_add", 2, None)
try:
    con.execute("SELECT my_add(1, 2)").fetchall()
except sqlite3.OperationalError:
    print("udf_unreg: OperationalError")

# create_aggregate
agg_events = []

class MyAvg:
    def __init__(self):
        agg_events.append("init")
        self.total = 0.0
        self.count = 0

    def step(self, val):
        if val is not None:
            self.total += val
            self.count += 1

    def finalize(self):
        agg_events.append("fin")
        return self.total / self.count if self.count else None

con.create_aggregate("my_avg", 1, MyAvg)
con.execute("CREATE TABLE nums (grp TEXT, v REAL)")
print("agg_empty:", con.execute("SELECT my_avg(v) FROM nums").fetchall(), agg_events)
con.executemany("INSERT INTO nums VALUES (?, ?)", [("a", 10), ("a", 20), ("b", 5), ("b", 15), ("b", 25)])
print("agg_grouped:", con.execute("SELECT grp, my_avg(v) FROM nums GROUP BY grp ORDER BY grp").fetchall())

class BadStepAgg:
    def step(self, x):
        raise RuntimeError("step fail")

    def finalize(self):
        return 0

con.create_aggregate("bad_agg", 1, BadStepAgg)
try:
    con.execute("SELECT bad_agg(v) FROM nums").fetchall()
except sqlite3.OperationalError:
    print("agg_err: OperationalError")

# create_collation
def rev_ci(a, b):
    al, bl = a.lower(), b.lower()
    return -1 if al > bl else (1 if al < bl else 0)

con.create_collation("REV_CI", rev_ci)
con.execute("CREATE TABLE words (w TEXT)")
con.executemany("INSERT INTO words VALUES (?)", [("apple",), ("Banana",), ("cherry",), ("Apricot",)])
print("collation:", [r[0] for r in con.execute("SELECT w FROM words ORDER BY w COLLATE REV_CI")])

# Non-callable collation raises TypeError; arbitrary collation names work in 3.12+
con.create_collation("my-coll", rev_ci)
print("custom_coll_name:", [r[0] for r in con.execute('SELECT w FROM words ORDER BY w COLLATE "my-coll"')])
try:
    con.create_collation("bad_cb", 123)
except TypeError:
    print("bad_coll_cb: TypeError")

def bad_coll(a, b):
    raise RuntimeError("coll boom")

con.create_collation("BAD_COLL", bad_coll)
try:
    con.execute("SELECT 'a' = 'b' COLLATE BAD_COLL").fetchall()
except RuntimeError as e:
    print("coll_err:", type(e).__name__, str(e))

con.create_collation("REV_CI", None)
try:
    con.execute("SELECT w FROM words ORDER BY w COLLATE REV_CI").fetchall()
except sqlite3.OperationalError:
    print("coll_unreg: OperationalError")
con.close()


# ── 7. Adapters & converters (PARSE_DECLTYPES / PARSE_COLNAMES) ──────────────
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __eq__(self, other):
        return isinstance(other, Point) and (self.x, self.y) == (other.x, other.y)

    def __repr__(self):
        return f"Point({self.x}, {self.y})"


class ConformPoint:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __conform__(self, protocol):
        if protocol is sqlite3.PrepareProtocol:
            return f"{self.x};{self.y}"


sqlite3.register_adapter(Point, lambda p: f"{p.x};{p.y}".encode("ascii"))
sqlite3.register_converter("point", lambda b: Point(*map(float, b.split(b";"))))

print("adapt_fn:", sqlite3.adapt(Point(1.0, 2.0)), sqlite3.adapt(ConformPoint(3.0, 4.0)))
print("adapt_alt:", sqlite3.adapt(object(), sqlite3.PrepareProtocol, "fallback"))

con = sqlite3.connect(":memory:", detect_types=sqlite3.PARSE_DECLTYPES | sqlite3.PARSE_COLNAMES)
con.execute('CREATE TABLE geom (p POINT, p2 "POINT(10)", raw TEXT)')
con.execute("INSERT INTO geom VALUES (?, ?, ?)", (Point(1.5, 2.5), ConformPoint(3.5, 4.5), "5.0;6.0"))
con.execute("INSERT INTO geom VALUES (NULL, NULL, NULL)")
cur = con.execute('SELECT p, p2, raw AS "r [point]" FROM geom')
print("conv_desc:", [d[0] for d in cur.description])
print("conv_rows:", cur.fetchall())
con.close()


# ── 8. iterdump, backup, Blob, complete_statement ────────────────────────────
src = sqlite3.connect(":memory:")
src.executescript(
    """
    CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT);
    INSERT INTO users (name) VALUES ('alice'), ('bob');
    CREATE INDEX idx_users_name ON users (name);
    CREATE VIEW v_users AS SELECT name FROM users;
    """
)
dump_sql = list(src.iterdump())
print("dump_bounds:", dump_sql[0], dump_sql[-1])

restored = sqlite3.connect(":memory:")
restored.executescript("\n".join(dump_sql))
print("restored:", restored.execute("SELECT * FROM users ORDER BY id").fetchall())
print("restored_view:", restored.execute("SELECT * FROM v_users ORDER BY name").fetchall())
restored.close()

# backup with progress callback
dst = sqlite3.connect(":memory:")
prog_log = []
src.backup(dst, pages=1, progress=lambda status, remaining, total: prog_log.append((status, remaining, total)))
print(
    "backup:",
    dst.execute("SELECT * FROM users ORDER BY id").fetchall(),
    len(prog_log) >= 1,
    prog_log[-1][0] == sqlite3.SQLITE_DONE,
    prog_log[-1][1] == 0,
)
dst.close()
src.close()

# Blob incremental I/O
con = sqlite3.connect(":memory:")
con.execute("CREATE TABLE blobs (data BLOB)")
con.execute("INSERT INTO blobs VALUES (?)", (b"0123456789",))
with con.blobopen("blobs", "data", 1) as blob:
    print("blob_init:", len(blob), blob.tell(), blob.read(4), blob.tell())
    blob.seek(2)
    blob.write(b"AB")
    print("blob_slice:", blob[0:6], blob[2])
    blob[0] = ord("X")
    blob[6:8] = b"YZ"
    blob.seek(0)
    print("blob_after:", blob.read())
    blob.seek(-2, io.SEEK_END)
    print("blob_end:", blob.read())
print("blob_db:", con.execute("SELECT data FROM blobs").fetchone())

with con.blobopen("blobs", "data", 1, readonly=True) as ro_blob:
    try:
        ro_blob.write(b"Z")
    except sqlite3.OperationalError:
        print("blob_ro: OperationalError")
con.close()

# complete_statement
print(
    "complete:",
    sqlite3.complete_statement("SELECT 1"),
    sqlite3.complete_statement("SELECT 1;"),
    sqlite3.complete_statement("SELECT 1;   "),
    sqlite3.complete_statement('SELECT "a;b"'),
)


# ── 9. File-backed DB, pathlib.Path, URI, custom factories, hooks, closed ────
with tempfile.TemporaryDirectory() as tmpdir:
    db_path = pathlib.Path(tmpdir) / "sample.db"
    c1 = sqlite3.connect(db_path)
    with c1:
        c1.execute("CREATE TABLE kv (k TEXT PRIMARY KEY, v INT)")
        c1.execute("INSERT INTO kv VALUES ('one', 1), ('two', 2)")
    c1.close()

    # Reopen from disk via str path
    c2 = sqlite3.connect(str(db_path))
    print("file_db:", c2.execute("SELECT k, v FROM kv ORDER BY v").fetchall())
    c2.close()

    # Reopen read-only via URI (portable across POSIX and Windows)
    posix_path = str(db_path).replace("\\", "/")
    if not posix_path.startswith("/"):
        posix_path = "/" + posix_path
    uri_str = "file://" + posix_path + "?mode=ro"
    c3 = sqlite3.connect(uri_str, uri=True)
    print("uri_ro_read:", c3.execute("SELECT count(*) FROM kv").fetchone())
    try:
        c3.execute("INSERT INTO kv VALUES ('three', 3)")
    except sqlite3.OperationalError:
        print("uri_ro_write: OperationalError")
    c3.close()

# Shared in-memory database via URI
u1 = sqlite3.connect("file:pyrust_mem_shared?mode=memory&cache=shared", uri=True)
u2 = sqlite3.connect("file:pyrust_mem_shared?mode=memory&cache=shared", uri=True)
u1.execute("CREATE TABLE sh (v INT)")
u1.execute("INSERT INTO sh VALUES (77)")
u1.commit()
print("uri_shared_mem:", u2.execute("SELECT v FROM sh").fetchall())
u2.close()
u1.close()

# Custom Connection & Cursor factories
class CustomCursor(sqlite3.Cursor):
    tag = "custom_cur"


class CustomConn(sqlite3.Connection):
    tag = "custom_con"


cc = sqlite3.connect(":memory:", factory=CustomConn)
ccur = cc.cursor(factory=CustomCursor)
print("factories:", type(cc) is CustomConn, cc.tag, type(ccur) is CustomCursor, ccur.tag)
cc.close()

# Authorizer, trace_callback, progress_handler, serialize/deserialize, limits
con = sqlite3.connect(":memory:")
traced = []
con.set_trace_callback(traced.append)
con.execute("CREATE TABLE s (x INT)")
con.execute("INSERT INTO s VALUES (42)")
con.set_trace_callback(None)
print("trace_count:", len(traced) >= 2)

# Authorizer denies DELETE
def auth(action, arg1, arg2, dbname, source):
    if action == sqlite3.SQLITE_DELETE:
        return sqlite3.SQLITE_DENY
    return sqlite3.SQLITE_OK

con.set_authorizer(auth)
try:
    con.execute("DELETE FROM s")
except sqlite3.DatabaseError:
    print("auth_deny: DatabaseError")
con.set_authorizer(None)

# Progress handler interrupts a query
prog_ticks = [0]
def prog_cb():
    prog_ticks[0] += 1
    return 1 if prog_ticks[0] >= 2 else 0

con.set_progress_handler(prog_cb, 1)
try:
    con.execute("WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM cnt LIMIT 10000) SELECT sum(x) FROM cnt").fetchall()
except sqlite3.OperationalError as e:
    print("progress_interrupt:", type(e).__name__, e.sqlite_errorcode, e.sqlite_errorname)
con.set_progress_handler(None, 0)

# getconfig / setconfig
orig_fkey = con.getconfig(sqlite3.SQLITE_DBCONFIG_ENABLE_FKEY)
con.setconfig(sqlite3.SQLITE_DBCONFIG_ENABLE_FKEY, not orig_fkey)
toggled_fkey = con.getconfig(sqlite3.SQLITE_DBCONFIG_ENABLE_FKEY)
con.setconfig(sqlite3.SQLITE_DBCONFIG_ENABLE_FKEY, orig_fkey)
print("dbconfig:", toggled_fkey == (not orig_fkey), con.getconfig(sqlite3.SQLITE_DBCONFIG_ENABLE_FKEY) == orig_fkey)

# serialize / deserialize
raw_db = con.serialize()
c_des = sqlite3.connect(":memory:")
c_des.deserialize(raw_db)
print("deserialized:", c_des.execute("SELECT * FROM s").fetchall())
c_des.close()

# getlimit / setlimit
orig_lim = con.getlimit(sqlite3.SQLITE_LIMIT_COLUMN)
prev_lim = con.setlimit(sqlite3.SQLITE_LIMIT_COLUMN, 50)
print("limits:", orig_lim > 0, prev_lim == orig_lim, con.getlimit(sqlite3.SQLITE_LIMIT_COLUMN) == 50)

# Closed cursor / closed connection errors
cur = con.cursor()
cur.close()
try:
    cur.execute("SELECT 1")
except sqlite3.ProgrammingError:
    print("closed_cur: ProgrammingError")

con.close()
con.close()  # idempotent
try:
    con.execute("SELECT 1")
except sqlite3.ProgrammingError:
    print("closed_con: ProgrammingError")
try:
    with con:
        pass
except sqlite3.ProgrammingError:
    print("closed_with: ProgrammingError")
