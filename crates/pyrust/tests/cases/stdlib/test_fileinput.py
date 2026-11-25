# Comprehensive CPython 3.12 parity tests for `fileinput` (issue #3230).

import bz2
import fileinput
import gzip
import io
import os
import pathlib
import sys
import tempfile


def expect_exc(fn, *args, **kwargs):
    try:
        res = fn(*args, **kwargs)
        return ("OK", res)
    except Exception as e:
        return (type(e).__name__, str(e))


# 1. Module metadata and GenericAlias
print("in sys.modules:", "fileinput" in sys.modules)
print("__all__:", fileinput.__all__)
print("FileInput.__module__:", fileinput.FileInput.__module__)
print("GenericAlias:", fileinput.FileInput[str])

# 2. No active input() errors
fileinput.close()
for fn_name in ("nextfile", "filename", "lineno", "filelineno", "fileno", "isfirstline", "isstdin"):
    fn = getattr(fileinput, fn_name)
    print(f"no active {fn_name}:", expect_exc(fn))

# 3. Validation errors
print("bad mode:", expect_exc(fileinput.FileInput, [], mode="w"))
print("hook+inplace:", expect_exc(fileinput.FileInput, [], inplace=True, openhook=fileinput.hook_encoded("utf-8")))
print("bad hook:", expect_exc(fileinput.FileInput, [], openhook=123))

with tempfile.TemporaryDirectory() as tmp:
    f1 = os.path.join(tmp, "a.txt")
    f2 = os.path.join(tmp, "empty.txt")
    f3 = os.path.join(tmp, "b.txt")
    with open(f1, "w", encoding="utf-8") as fp:
        fp.write("alpha\nbeta\n")
    with open(f2, "w", encoding="utf-8") as fp:
        fp.write("")
    with open(f3, "w", encoding="utf-8") as fp:
        fp.write("gamma\ndelta")

    def base(p):
        return None if p is None else (p if p.startswith("<") else os.path.basename(p))

    # 4. FileInput context manager + iterator across multiple files (including empty file)
    rows = []
    with fileinput.FileInput([f1, f2, f3], encoding="utf-8") as fi:
        print(
            "before read:",
            fi.filename(),
            fi.lineno(),
            fi.filelineno(),
            fi.fileno(),
            fi.isfirstline(),
            fi.isstdin(),
        )
        for line in fi:
            rows.append((
                base(fi.filename()),
                fi.lineno(),
                fi.filelineno(),
                fi.isfirstline(),
                fi.isstdin(),
                fi.fileno() >= 0,
                line,
            ))
        print(
            "after read:",
            base(fi.filename()),
            fi.lineno(),
            fi.filelineno(),
            fi.fileno(),
            fi.isfirstline(),
            fi.isstdin(),
        )
    print("multi-file rows:", rows)

    # 5. Single string path and PathLike input
    with fileinput.FileInput(f1, encoding="utf-8") as fi:
        print("single str:", list(fi), base(fi.filename()))
    with fileinput.FileInput(pathlib.Path(f1), encoding="utf-8") as fi:
        print("single Path:", list(fi), base(fi.filename()))

    # 6. Binary mode + readline()
    with fileinput.FileInput([f1, f3], mode="rb") as fi:
        b_lines = []
        while True:
            chunk = fi.readline()
            if not chunk:
                break
            b_lines.append((base(fi.filename()), fi.lineno(), fi.filelineno(), chunk))
        print("binary readline:", b_lines, repr(fi.readline()))

    # 7. nextfile() skips remainder of current file
    skipped = []
    with fileinput.FileInput([f1, f3], encoding="utf-8") as fi:
        for line in fi:
            skipped.append((base(fi.filename()), fi.lineno(), fi.filelineno(), line))
            if fi.isfirstline():
                fi.nextfile()
    print("nextfile skip:", skipped)

    # 8. Module-level stateful helpers + re-entry error
    mod_rows = []
    with fileinput.input([f1, f3], encoding="utf-8") as fi:
        for line in fi:
            if fileinput.lineno() == 1:
                print("re-entry:", expect_exc(fileinput.input, [f1]))
            mod_rows.append((
                base(fileinput.filename()),
                fileinput.lineno(),
                fileinput.filelineno(),
                fileinput.isfirstline(),
                fileinput.isstdin(),
                fileinput.fileno() >= 0,
                line,
            ))
    print("module input rows:", mod_rows)
    fileinput.close()

    # 9. '-' reads from sys.stdin
    old_stdin = sys.stdin
    try:
        sys.stdin = io.StringIO("s1\ns2\n")
        stdin_rows = []
        with fileinput.FileInput(["-", f1], encoding="utf-8") as fi:
            for line in fi:
                stdin_rows.append((base(fi.filename()), fi.lineno(), fi.filelineno(), fi.isstdin(), line))
        print("stdin rows:", stdin_rows)
    finally:
        sys.stdin = old_stdin

    # 10. inplace=True without backup (temp .bak deleted) and with backup=".orig"
    in1 = os.path.join(tmp, "inplace1.txt")
    in2 = os.path.join(tmp, "inplace2.txt")
    with open(in1, "w", encoding="utf-8") as fp:
        fp.write("one\ntwo\n")
    with open(in2, "w", encoding="utf-8") as fp:
        fp.write("three\nfour\n")

    with fileinput.FileInput([in1], inplace=True, encoding="utf-8") as fi:
        for line in fi:
            print(line.strip().upper())
    with open(in1, "r", encoding="utf-8") as fp:
        print("inplace1 content:", repr(fp.read()))
    print("inplace1 bak removed:", not os.path.exists(in1 + ".bak"))

    with fileinput.FileInput([in2], inplace=True, backup=".orig", encoding="utf-8") as fi:
        for line in fi:
            print("PREFIX:" + line.strip())
    with open(in2, "r", encoding="utf-8") as fp:
        print("inplace2 content:", repr(fp.read()))
    with open(in2 + ".orig", "r", encoding="utf-8") as fp:
        print("inplace2 orig:", repr(fp.read()))

    # 11. hook_encoded
    enc_file = os.path.join(tmp, "latin1.txt")
    with open(enc_file, "wb") as fp:
        fp.write("caf\xe9\nna\xefve\n".encode("latin-1"))
    with fileinput.FileInput([enc_file], openhook=fileinput.hook_encoded("latin-1")) as fi:
        print("hook_encoded:", list(fi))

    # 12. hook_compressed (.gz, .bz2, and plain file in both text and binary modes)
    gz_file = os.path.join(tmp, "data.gz")
    bz_file = os.path.join(tmp, "data.bz2")
    with gzip.open(gz_file, "wt", encoding="utf-8") as fp:
        fp.write("gz1\ngz2\n")
    with bz2.open(bz_file, "wt", encoding="utf-8") as fp:
        fp.write("bz1\nbz2\n")

    comp_text = []
    with fileinput.FileInput(
        [gz_file, bz_file, f1],
        openhook=fileinput.hook_compressed,
        encoding="utf-8",
    ) as fi:
        for line in fi:
            comp_text.append((base(fi.filename()), fi.lineno(), fi.filelineno(), fi.fileno() >= 0, line))
    print("hook_compressed text:", comp_text)

    comp_bin = []
    with fileinput.FileInput(
        [gz_file, bz_file],
        mode="rb",
        openhook=fileinput.hook_compressed,
    ) as fi:
        for line in fi:
            comp_bin.append((base(fi.filename()), line))
    print("hook_compressed bin:", comp_bin)
