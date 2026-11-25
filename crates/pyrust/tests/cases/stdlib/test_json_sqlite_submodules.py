"""CPython 3.12 parity tests for `json.{decoder,encoder,scanner,tool}` and `sqlite3.dump` (issue #3251)."""

import json
import json.decoder
import json.encoder
import json.scanner
import json.tool
import math
import os
import sqlite3
import sqlite3.dump
import subprocess
import sys
import tempfile


# ===========================================================================
# 1. json package submodule bindings & class identity
# ===========================================================================
print("=== 1. json bindings & identity ===")
print(hasattr(json, "decoder"), hasattr(json, "encoder"), hasattr(json, "scanner"), hasattr(json, "tool"))
print(
    json.JSONDecoder is json.decoder.JSONDecoder,
    json.JSONDecodeError is json.decoder.JSONDecodeError,
    json.JSONEncoder is json.encoder.JSONEncoder,
)


# ===========================================================================
# 2. json.decoder
# ===========================================================================
print("=== 2. json.decoder ===")
print("WHITESPACE_STR:", repr(json.decoder.WHITESPACE_STR))
print("WHITESPACE match:", json.decoder.WHITESPACE.match("  \t\nhello").end())
print("scanstring basic:", json.decoder.scanstring('"hello\\nworld"', 1))
print("scanstring unicode:", json.decoder.scanstring('"\\u00e9\\u2603"', 1))
print("scanstring surrogate:", json.decoder.scanstring('"\\ud83d\\ude00"', 1))
print("scanstring non-strict:", json.decoder.scanstring('"a\tb"', 1, False))

try:
    json.decoder.scanstring('"a\tb"', 1, True)
    print("strict ctrl: NO-ERR")
except json.decoder.JSONDecodeError as e:
    print("strict ctrl: JSONDecodeError", e.pos)

try:
    json.decoder.scanstring('"unterminated', 1)
    print("unterminated: NO-ERR")
except json.decoder.JSONDecodeError as e:
    print("unterminated: JSONDecodeError", e.pos)


# ===========================================================================
# 3. json.encoder
# ===========================================================================
print("=== 3. json.encoder ===")
print("INFINITY:", math.isinf(json.encoder.INFINITY), json.encoder.INFINITY > 0)
print("encode_basestring:", json.encoder.encode_basestring('a"b\\c\n\x01\u00e9'))
print("encode_basestring_ascii:", json.encoder.encode_basestring_ascii('a"b\\c\n\x01\u00e9'))
print("encode_basestring_ascii emoji:", json.encoder.encode_basestring_ascii("\U0001f600"))


# ===========================================================================
# 4. json.scanner
# ===========================================================================
print("=== 4. json.scanner ===")
dec = json.JSONDecoder()
sc = json.scanner.make_scanner(dec)
py_sc = json.scanner.py_make_scanner(dec)
for fn_name, fn in (("make_scanner", sc), ("py_make_scanner", py_sc)):
    print(fn_name, "obj:", fn('{"a": [1, 2.5, true, false, null]}', 0))
    print(fn_name, "str:", fn('"abc" rest', 0))
    try:
        fn("]", 0)
        print(fn_name, "stop: NO-ERR")
    except StopIteration as e:
        print(fn_name, "stop:", e.value)

custom_dec = json.JSONDecoder(parse_int=lambda s: int(s) * 10, parse_constant=lambda c: f"CONST:{c}")
custom_sc = json.scanner.py_make_scanner(custom_dec)
print("custom scanner:", custom_sc("[3, NaN, Infinity]", 0))


# ===========================================================================
# 5. json.tool (direct main() + `-m json.tool` CLI)
# ===========================================================================
print("=== 5. json.tool ===")
with tempfile.TemporaryDirectory() as tmpdir:
    in_file = os.path.join(tmpdir, "in.json")
    out_file = os.path.join(tmpdir, "out.json")
    with open(in_file, "w", encoding="utf-8") as f:
        f.write('{"b": 1, "a": [2, "\u00e9"]}')

    # Direct json.tool.main() with --indent 2 --sort-keys --no-ensure-ascii
    saved_argv = sys.argv[:]
    try:
        sys.argv = [
            "json.tool",
            "--indent",
            "2",
            "--sort-keys",
            "--no-ensure-ascii",
            in_file,
            out_file,
        ]
        json.tool.main()
    finally:
        sys.argv = saved_argv

    with open(out_file, "r", encoding="utf-8") as f:
        print("direct main:\n" + f.read().rstrip("\n"))

    # CLI `-m json.tool` via subprocess (compact + sort-keys + stdin)
    res_compact = subprocess.run(
        [sys.executable, "-m", "json.tool", "--compact", "--sort-keys"],
        input='{"z": 1, "a": 2}',
        text=True,
        capture_output=True,
    )
    print("cli compact:", res_compact.returncode, repr(res_compact.stdout))

    # CLI `-m json.tool` with --json-lines
    res_lines = subprocess.run(
        [sys.executable, "-m", "json.tool", "--json-lines", "--compact", "--sort-keys"],
        input='{"b": 1, "a": 2}\n{"d": 4, "c": 3}\n',
        text=True,
        capture_output=True,
    )
    print("cli json-lines:", res_lines.returncode, res_lines.stdout.strip().splitlines())

    # CLI `-m json.tool` invalid JSON exits non-zero
    res_bad = subprocess.run(
        [sys.executable, "-m", "json.tool"],
        input="{bad json",
        text=True,
        capture_output=True,
    )
    print("cli invalid nonzero:", res_bad.returncode != 0)


# ===========================================================================
# 6. sqlite3.dump
# ===========================================================================
print("=== 6. sqlite3.dump ===")
print("sqlite3.dump bound:", hasattr(sqlite3, "dump"), hasattr(sqlite3.dump, "_iterdump"))
conn = sqlite3.connect(":memory:")
conn.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, price REAL)")
conn.execute("INSERT INTO items (name, price) VALUES ('apple', 1.25)")
conn.execute("INSERT INTO items (name, price) VALUES ('O''Brien', 2.5)")
conn.commit()

dump_lines = list(sqlite3.dump._iterdump(conn))
conn_lines = list(conn.iterdump())
print("dump matches iterdump:", dump_lines == conn_lines)
print("has BEGIN/COMMIT:", dump_lines[0] == "BEGIN TRANSACTION;", dump_lines[-1] == "COMMIT;")

# Replay dump into a new database and verify exact row parity
conn2 = sqlite3.connect(":memory:")
conn2.executescript("\n".join(dump_lines))
rows = conn2.execute("SELECT id, name, price FROM items ORDER BY id").fetchall()
print("replayed rows:", rows)
conn.close()
conn2.close()

print("DONE")
