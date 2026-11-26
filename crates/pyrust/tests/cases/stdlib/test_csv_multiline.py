import csv
import io


def show(label, fn):
    try:
        print(label, repr(fn()))
    except Exception as e:
        print(label, type(e).__name__, e)


def rows(lines, **kw):
    return list(csv.reader(lines, **kw))


# --- multi-line quoted fields (issue #3340) ---------------------------------
show("lf", lambda: rows(io.StringIO('a,"b\nc",d\n')))
show("crlf-blank", lambda: rows(io.StringIO('a,"b\r\nc\n\nd",e\n')))
show("cr", lambda: rows(io.StringIO('a,"b\rc",d\r')))
show("crlf-terminated", lambda: rows(io.StringIO('a,"b\r\nc",d\r\n', newline="")))
show("list-lines", lambda: rows(['a,"b\n', 'c",d\n', 'e,f\n']))
show("multi-continuation", lambda: rows(['"a\n', 'b\n', 'c\n', 'd"\n']))
show("empty-quoted-lines", lambda: rows(['"\n', '\n', '"\n']))
show("trailing-nl-field", lambda: rows(['x,"y\n', '"\n']))
show("two-records", lambda: rows(['1,"a\n', 'b",2\n', '3,"c\n', 'd",4\n']))
show("no-trailing-newline", lambda: rows(['a,"b\n', 'c",d']))

# --- line_num across multi-line records --------------------------------------
r = csv.reader(['a,"b\n', 'c",d\n', 'e\n'])
nums = []
for row in r:
    nums.append((row, r.line_num))
print("line_num", nums, r.line_num)

r = csv.reader(io.StringIO('a,"b\nc\nd",e\nf\n'))
print("line_num-sio", [(row, r.line_num) for row in r], r.line_num)

r = csv.reader(['a\n', '\n', 'b\n'])
print("line_num-blank", [(row, r.line_num) for row in r])

# --- unterminated quoted fields and strict ------------------------------------
show("eof-nonstrict", lambda: rows(['a,"b']))
show("eof-nonstrict-empty", lambda: rows(['a,"']))
show("eof-nonstrict-nl", lambda: rows(['a,"b\n']))
show("eof-nonstrict-2lines", lambda: rows(['a,"b\n', 'c']))
show("eof-strict", lambda: rows(['a,"b'], strict=True))
show("eof-strict-empty", lambda: rows(['a,"'], strict=True))
show("eof-strict-2lines", lambda: rows(['a,"b\n', 'c\n'], strict=True))
show("eof-strict-ok", lambda: rows(['a,"b\n', 'c"\n'], strict=True))
show("eof-strict-unquoted", lambda: rows(['a,b'], strict=True))
show("eof-strict-trailing-delim", lambda: rows(['a,'], strict=True))
show("strict-after-quote", lambda: rows(['"a"b,c'], strict=True))
show("nonstrict-after-quote", lambda: rows(['"a"b,c']))
show("strict-quote-then-space", lambda: rows(['"a" ,c'], strict=True))

r = csv.reader(['a,"b'], strict=True)
try:
    next(r)
except csv.Error as e:
    print("strict-err", e, r.line_num)

# --- escapechar before newline -----------------------------------------------
show("esc-nl", lambda: rows(io.StringIO('a\\\nb\n'), escapechar="\\"))
show("esc-crlf", lambda: rows(['a\\\r\n', 'b\r\n'], escapechar="\\"))
show("esc-cr", lambda: rows(['a\\\r', 'b\r'], escapechar="\\"))
show("esc-nl-then-blank", lambda: rows(['a\\\n', '\n', 'b\n'], escapechar="\\"))
show("esc-eol-eof", lambda: rows(['a\\'], escapechar="\\"))
show("esc-eol-eof-strict", lambda: rows(['a\\'], escapechar="\\", strict=True))
show("esc-in-quoted", lambda: rows(['"a\\"b",c\n'], escapechar="\\"))
show("esc-in-quoted-nl", lambda: rows(['"a\\\n', 'b",c\n'], escapechar="\\"))
show("esc-in-quoted-eol", lambda: rows(['"a\\'], escapechar="\\"))
show("esc-in-quoted-eol-strict", lambda: rows(['"a\\'], escapechar="\\", strict=True))
show("esc-start-field", lambda: rows(['\\,a,b\n'], escapechar="\\"))
show("esc-delim", lambda: rows(['a\\,b,c\n'], escapechar="\\"))
show("esc-quote-none", lambda: rows(['a\\"b,c\n'], escapechar="\\", quoting=csv.QUOTE_NONE))
show("esc-nl-quote-none", lambda: rows(['a\\\n', 'b\n'], escapechar="\\", quoting=csv.QUOTE_NONE))

# --- doublequote ---------------------------------------------------------------
show("dq-true", lambda: rows(['"a""b\n', 'c""d",e\n']))
show("dq-true-end", lambda: rows(['"a""\n', '",e\n']))
show("dq-false", lambda: rows(['"a""b\n', 'c",e\n'], doublequote=False))
show("dq-false-escape", lambda: rows(['"a\\"b\n', 'c",e\n'], doublequote=False, escapechar="\\"))
show("dq-false-strict", lambda: rows(['"a"b\n'], doublequote=False, strict=True))
show("dq-false-then-quote", lambda: rows(['"a"b"c\n', 'd",e\n'], doublequote=False))

# --- QUOTE_NONE / QUOTE_ALL / QUOTE_NONNUMERIC / QUOTE_MINIMAL ------------------
show("none-embedded-quote", lambda: rows(['a,"b\n', 'c",d\n'], quoting=csv.QUOTE_NONE))
show("none-eof", lambda: rows(['a,"b'], quoting=csv.QUOTE_NONE, strict=True))
show("all", lambda: rows(['"a","b\n', 'c","d"\n'], quoting=csv.QUOTE_ALL))
show("minimal", lambda: rows(['"a","b\n', 'c","d"\n'], quoting=csv.QUOTE_MINIMAL))
show("nonnumeric", lambda: rows(['1,"b\n', 'c",2.5\n'], quoting=csv.QUOTE_NONNUMERIC))
show("nonnumeric-empty", lambda: rows(['1,,3\n'], quoting=csv.QUOTE_NONNUMERIC))
show("nonnumeric-quoted-num", lambda: rows(['"1",2\n'], quoting=csv.QUOTE_NONNUMERIC))
show("nonnumeric-bad", lambda: rows(['abc,2\n'], quoting=csv.QUOTE_NONNUMERIC))
show("nonnumeric-multiline-unquoted", lambda: rows(['1,"x\n', 'y"\n'], quoting=csv.QUOTE_NONNUMERIC))
show("nonnumeric-esc", lambda: rows(['1\\2,3\n'], quoting=csv.QUOTE_NONNUMERIC, escapechar="\\"))

# --- other dialect knobs --------------------------------------------------------
show("skipinitialspace", lambda: rows(['a, "b\n', 'c", d\n'], skipinitialspace=True))
show("tab", lambda: rows(['a\t"b\n', 'c"\td\n'], delimiter="\t"))
show("quotechar", lambda: rows(["a,'b\n", "c',d\n"], quotechar="'"))
show("excel-dialect", lambda: rows(['a,"b\n', 'c"\n'], dialect="excel"))
show("unix-dialect", lambda: rows(['"a","b\n', 'c"\n'], dialect="unix"))

# --- record boundaries / blank and newline-only handling ---------------------------
show("empty-input", lambda: rows([]))
show("empty-line", lambda: rows(['']))
show("blank-lines", lambda: rows(['\n', '\r\n', 'a\n']))
show("cr-lone", lambda: rows(['\r']))
show("crlf-line", lambda: rows(['a,b\r\n', 'c,d\r\n']))
show("unquoted-embedded-cr", lambda: rows(['a\rb\n']))
show("unquoted-embedded-cr-quoted-tail", lambda: rows(['"a"\rb\n']))
show("nul-char", lambda: rows(['a\x00b,c\n']))
show("non-str-line", lambda: rows([b'a,b']))
show("int-line", lambda: rows([1]))
show("trailing-delim", lambda: rows(['a,b,\n']))
show("leading-delim", lambda: rows([',a\n']))

# --- DictReader ------------------------------------------------------------------
dr = csv.DictReader(io.StringIO('name,note\nann,"line1\nline2"\nbob,"x\n\ny"\n'))
print("dict", list(dr), dr.line_num)

dr = csv.DictReader(['h1,h2\n', '1,"a\n', 'b"\n', '\n', '3,4\n'])
out = []
for d in dr:
    out.append((dict(d), dr.line_num))
print("dict-line_num", out)

dr = csv.DictReader(['"h\n', '1",h2\n', 'x,y\n'])
print("dict-multiline-header", list(dr), dr.fieldnames)

dr = csv.DictReader(['a,"b\n', 'c"\n'], fieldnames=["k1", "k2", "k3"], restval="R")
print("dict-restval", list(dr))

dr = csv.DictReader(['1,"2\n', '3",4,5\n'], fieldnames=["k"], restkey="rest")
print("dict-restkey", list(dr))

dr = csv.DictReader(['h1,h2\n', '1,"unterminated\n'], strict=True)
show("dict-strict-eof", lambda: list(dr))

dr = csv.DictReader(['h1,h2\n', '1,"unterminated\n'])
show("dict-nonstrict-eof", lambda: list(dr))

# --- round trip through the writer --------------------------------------------------
buf = io.StringIO()
w = csv.writer(buf)
data = [["a", "b\nc", 'd"e'], ["", "x\r\ny", "z"], ["multi\n\nblank", "q", ""]]
w.writerows(data)
buf.seek(0)
back = list(csv.reader(buf))
print("roundtrip", back == data, back)

buf = io.StringIO()
w = csv.writer(buf, quoting=csv.QUOTE_ALL)
w.writerows(data)
buf.seek(0)
print("roundtrip-all", list(csv.reader(buf)) == data)

buf = io.StringIO()
w = csv.writer(buf, escapechar="\\", quoting=csv.QUOTE_NONE)
w.writerows([["a\nb", "c,d"], ["e", "f"]])
print("roundtrip-esc-text", repr(buf.getvalue()))
buf.seek(0)
print("roundtrip-esc", list(csv.reader(buf, escapechar="\\", quoting=csv.QUOTE_NONE)))

# --- file object with newline='' -------------------------------------------------------
import os
import tempfile

path = os.path.join(tempfile.gettempdir(), "pyrust_csv_multiline_3340.csv")
with open(path, "w", newline="") as f:
    f.write('id,text\r\n1,"first\r\nsecond"\r\n2,"only"\r\n')
with open(path, newline="") as f:
    r = csv.reader(f)
    print("file", [(row, r.line_num) for row in r])
with open(path, newline="") as f:
    print("file-dict", list(csv.DictReader(f)))
os.remove(path)

# --- iteration protocol ------------------------------------------------------------------
r = csv.reader(['a,"b\n', 'c"\n'])
print("iter-self", iter(r) is r)
print("next", next(r))
show("next-stop", lambda: next(r))
show("next-stop-again", lambda: next(r))
