# Issue #3413: `json` is implemented natively in Rust.  This fixture pins the
# CPython 3.12.3 behaviour of the package surface with its `_json`
# accelerator: `dumps` / `dump` / `loads` / `load` with every option, the
# exact chunking of `JSONEncoder.iterencode` (and so of `json.dump`'s writes),
# `JSONEncoder` / `JSONDecoder` subclassing, `JSONDecodeError` positions and
# pickling, the native `_json.Scanner` vs `py_make_scanner`, `scanstring` /
# `py_scanstring` / `JSONObject` / `JSONArray`, bytes input with the
# `surrogatepass` decoding `loads` uses, argument binding errors, nesting
# limits, and the module namespaces.
#
# Not pinned: the dunder slots built-in modules do not have (`__file__`,
# `__path__`, `__cached__`, `__builtins__`) and class `__dict__` /
# `__weakref__` slots (runtime-wide); the pure-Python encoder's `ESCAPE` /
# `ESCAPE_ASCII` / `HAS_UTF8` / `STRINGCHUNK` regexes, `_decode_uXXXX`,
# `_make_iterencode` and `_json.make_encoder` / `c_make_encoder` (not built:
# the native encoder replaces them, and skipping the regexes keeps
# `import json` cheap); the exact nesting depth at which `RecursionError` is
# raised (platform-dependent in CPython); and `repr()` of generators.

import collections
import decimal
import enum
import io
import json
import json.decoder
import json.encoder
import json.scanner
import pickle

import _json


def show(label, thunk):
    try:
        r = thunk()
        print(label, "->", ascii(r))
    except Exception as e:
        print(label, "!!", type(e).__name__, ascii(str(e)))


class IE(enum.IntEnum):
    A = 1
    B = 2


class FS(float):
    def __repr__(self):
        return "FS!"


class SS(str):
    pass


class LS(list):
    pass


class TS(tuple):
    pass


class DS(dict):
    def items(self):
        return sorted(super().items(), reverse=True)


Point = collections.namedtuple("Point", "x y")

# ── 1. Namespaces ────────────────────────────────────────────────────────────
print("=== 1. namespaces ===")
print([k for k in vars(json) if not k.startswith("__")])
print(json.__all__, json.__version__, json.__author__, json.__package__)
print(json.__doc__.splitlines()[0])
# CPython-only helpers the native package does not build: the pure-Python
# encoder's regexes and closure factory, and the `_json.make_encoder` type.
PY_ONLY = {"STRINGCHUNK", "_decode_uXXXX", "c_make_encoder", "ESCAPE", "ESCAPE_ASCII", "HAS_UTF8", "_make_iterencode"}
for mod in (json.decoder, json.encoder, json.scanner):
    names = [k for k in vars(mod) if not k.startswith("__") and k not in PY_ONLY]
    print(mod.__name__, names, getattr(mod, "__all__", None), mod.__doc__)
for cls in (json.JSONEncoder, json.JSONDecoder, json.JSONDecodeError):
    print(cls.__module__, cls.__qualname__, [k for k in vars(cls) if k not in ("__dict__", "__weakref__")])
    print(cls.__doc__.splitlines()[0])
print(json.JSONDecodeError.__mro__)
print(
    json.JSONDecoder is json.decoder.JSONDecoder,
    json.JSONEncoder is json.encoder.JSONEncoder,
    json.decoder.scanstring is json.decoder.c_scanstring,
    json.encoder.encode_basestring is json.encoder.c_encode_basestring,
    json.encoder.encode_basestring_ascii is json.encoder.c_encode_basestring_ascii,
    json.scanner.make_scanner is json.scanner.c_make_scanner,
    _json.make_scanner is json.scanner.make_scanner,
    _json.scanstring is json.decoder.scanstring,
    _json.encode_basestring_ascii is json.encoder.encode_basestring_ascii,
)
print(json.scanner.make_scanner.__name__, json.scanner.make_scanner.__module__, json.decoder.scanstring.__name__)
print(type(json._default_encoder).__name__, type(json._default_decoder).__name__)
print(list(vars(json._default_encoder)), json._default_encoder.item_separator, json._default_encoder.key_separator)
print(list(vars(json._default_decoder)))
print(type(json._default_decoder.scan_once).__name__, json._default_decoder.parse_float, json._default_decoder.parse_int)
# `int()`: pyrust's `re` flags are plain ints rather than `re.RegexFlag`.
print(json.decoder.NaN is json.decoder._CONSTANTS["NaN"], int(json.decoder.FLAGS), repr(json.decoder.WHITESPACE_STR))
print(json.decoder.WHITESPACE.match("  \t\nx").end(), json.scanner.NUMBER_RE.match("-12.5e3x").groups())

# ── 2. dumps: values and builtin subclasses ──────────────────────────────────
print("=== 2. dumps values ===")
values = [
    None, True, False, 0, -1, 2**70, -(2**64), 1.0, -0.0, 1e16, 1.5e-7, 3.14159, 1e300 * 10,
    float("nan"), float("-inf"), "", "a\"b\\c", "\x00\x1f\x7f\x80 ￿", "\U0001f600",
    "\ud800", "x\udfffy", [], (), {}, [1, [2, [3, []]]], {"a": {"b": {}}},
    IE.A, FS(2.5), SS("sub"), LS([1, 2]), TS((3, 4)), DS(a=1, b=2), Point(1, 2),
    collections.OrderedDict([("z", 1), ("a", 2)]), collections.Counter("aab"),
    collections.defaultdict(int, {"k": 1}),
    {1: "i", 2.5: "f", True: "t", None: "n", IE.B: "e", FS(1.0): "fs"},
]
for v in values:
    show("dumps", lambda v=v: json.dumps(v))
    show("dumps/na", lambda v=v: json.dumps(v, ensure_ascii=False))
    show("dumps/ind", lambda v=v: json.dumps(v, indent=2, sort_keys=True))

# ── 3. dumps: options ────────────────────────────────────────────────────────
print("=== 3. dumps options ===")
show("allow_nan", lambda: json.dumps([1.0, float("nan")], allow_nan=False))
show("allow_nan fs", lambda: json.dumps(FS("inf"), allow_nan=False))
show("allow_nan ind", lambda: json.dumps([float("inf")], allow_nan=False, indent=1))
show("allow_nan key", lambda: json.dumps({float("nan"): 1}, allow_nan=False))
show("sortkeys", lambda: json.dumps({"b": 1, "a": {"d": 1, "c": 2}}, sort_keys=True))
show("sortkeys mixed", lambda: json.dumps({1: 1, "a": 2}, sort_keys=True))
show("sortkeys mixed ind", lambda: json.dumps({1: 1, "a": 2}, sort_keys=True, indent=1))
show("badkey", lambda: json.dumps({(1, 2): 1}))
show("badkey ind", lambda: json.dumps({(1, 2): 1}, indent=1))
show("skipkeys", lambda: json.dumps({(1, 2): 1, "a": 2, b"x": 3}, skipkeys=True))
show("skipkeys ind", lambda: json.dumps({(1, 2): 1, "a": 2}, skipkeys=True, indent=1))
show("skipkeys all", lambda: json.dumps({(1, 2): 1}, skipkeys=True))
show("skipkeys all ind", lambda: json.dumps({(1, 2): 1}, skipkeys=True, indent=1))
show("nocirc", lambda: json.dumps([[1], [2]], check_circular=False))
show("default", lambda: json.dumps({"s": {1, 2}}, default=sorted))
show("default raise", lambda: json.dumps([1, 2j]))
show("default bytes", lambda: json.dumps(b"x"))
show("separators", lambda: json.dumps({"a": [1, 2]}, separators=(";", "=")))
show("separators bad", lambda: json.dumps({"a": [1, 2]}, separators=(1, 2)))
show("separators str", lambda: json.dumps({"a": 1}, separators=",:"))
show("separators 3", lambda: json.dumps({"a": 1}, separators=[",", ":", "x"]))
show("separators int", lambda: json.dumps({"a": 1}, separators=5))
show("indent str", lambda: json.dumps([1, {"a": []}], indent="--"))
show("indent 0", lambda: json.dumps([1, {"a": [2]}], indent=0))
show("indent neg", lambda: json.dumps([1], indent=-3))
show("indent bool", lambda: json.dumps([1], indent=True))
show("indent float", lambda: json.dumps([1], indent=2.5))
show("indent sep", lambda: json.dumps([1, 2], indent=1, separators=(" ,", " :")))
show("bigint limit", lambda: json.dumps(10**5000))
show("unknown kw", lambda: json.dumps(1, cls=json.JSONEncoder, foo=1))
show("args dumps()", lambda: json.dumps())
show("args dumps(1, 2, indent)", lambda: json.dumps(1, 2, indent=1))
show("args dumps(1, 2, 3)", lambda: json.dumps(1, 2, 3))
show("args dump(1)", lambda: json.dump(1))
show("args loads(1, 2)", lambda: json.loads("1", 2))
show("args JSONEncoder", lambda: json.JSONEncoder(indent=1, foo=2))
show("args JSONEncoder pos", lambda: json.JSONEncoder(1))
show("args encode()", lambda: json.JSONEncoder().encode())
show("args iterencode", lambda: json.JSONEncoder().iterencode(1, 2, 3))
show("args decode()", lambda: json.JSONDecoder().decode())
show("args raw_decode", lambda: json.JSONDecoder().raw_decode("1", 0, 1))
show("args JSONDecoder", lambda: json.JSONDecoder(foo=1))
show("args JSONDecodeError", lambda: json.JSONDecodeError("m", "d"))
show("args detect_encoding", lambda: json.detect_encoding())
show("args loads(s=)", lambda: json.loads(s="1"))
show("args dumps(obj=)", lambda: json.dumps(obj=[1]))

# ── 4. JSONEncoder subclasses and iterencode chunking ────────────────────────
print("=== 4. encoder classes ===")


class Enc(json.JSONEncoder):
    def default(self, o):
        if isinstance(o, complex):
            return [o.real, o.imag]
        return super().default(o)


class Enc2(json.JSONEncoder):
    def iterencode(self, o, _one_shot=False):
        for chunk in super().iterencode(o, _one_shot):
            yield chunk.upper()


class Enc3(json.JSONEncoder):
    def encode(self, o):
        return "<" + super().encode(o) + ">"


show("Enc", lambda: json.dumps([1j, {"x": 2 + 3j}], cls=Enc))
show("Enc bad", lambda: json.dumps([object()], cls=Enc))
show("Enc2", lambda: json.dumps({"a": "b"}, cls=Enc2))
show("Enc3", lambda: json.dumps({"a": "b"}, cls=Enc3))
show("Enc3 str", lambda: Enc3().encode("x"))
show("enc vars", lambda: list(vars(json.JSONEncoder(separators=(",", ":"), default=str)).items())[:-1])
show("enc seps", lambda: (json.JSONEncoder(indent=2).item_separator, json.JSONEncoder(indent=2).key_separator))
show("one_shot", lambda: json.JSONEncoder().iterencode([1, {"a": 2}], True))
show("one_shot ind", lambda: list(json.JSONEncoder(indent=1).iterencode([1, {"a": 2}], True)))
for kwargs in [{}, {"indent": 2}, {"sort_keys": True, "indent": "\t"}, {"separators": (",", ":")}]:
    enc = json.JSONEncoder(**kwargs)
    obj = {
        "b": [1, 2.5, None, True, "s", [], {}, [[]], {"x": {"y": [3]}}],
        "a": IE.A,
        "c": Point(1, 2),
        "d": DS(q=1, r=2),
    }
    show("iterencode %r" % (kwargs,), lambda enc=enc, obj=obj: list(enc.iterencode(obj)))
gen = json.JSONEncoder().iterencode([1, [object()]])
print("lazy", next(gen), next(gen))
show("lazy error", lambda: next(gen))
show("lazy after error", lambda: next(gen))


class W:
    def __init__(self):
        self.parts = []

    def write(self, s):
        self.parts.append(s)


w = W()
show("dump partial", lambda: json.dump([1, "a", {"b": [2, object()]}], w))
print("dump parts", w.parts)
w = W()
show("dump ok", lambda: json.dump({"a": [1, 2], "b": {}}, w, indent=1))
print("dump parts", w.parts)
w = W()
calls = []


def dflt(o):
    calls.append(("default", type(o).__name__, len(w.parts)))
    return type(o).__name__


show("dump default order", lambda: json.dump([1, object(), [object()]], w, default=dflt))
print("dump parts", w.parts, calls)
buf = io.StringIO()
json.dump({"k": ["é", 1.5]}, buf, ensure_ascii=False, sort_keys=True)
print("dump StringIO", ascii(buf.getvalue()))

# ── 5. Circular references ───────────────────────────────────────────────────
print("=== 5. circular ===")
a = []
a.append(a)
show("circ", lambda: json.dumps(a))
show("circ ind", lambda: json.dumps(a, indent=1))
d = {}
d["d"] = [d]
show("circ d", lambda: json.dumps(d))
show("circ d ind", lambda: json.dumps(d, indent=1))
show("circ default", lambda: json.dumps(object(), default=lambda o: [o]))
x = []
show("shared", lambda: json.dumps([x, x, {"k": x}]))

# ── 6. loads ─────────────────────────────────────────────────────────────────
print("=== 6. loads ===")
docs = [
    '{"a": 1, "a": 2}', '[1.5, -0, 1e5, 1E-5, 0.0, 12345678901234567890123]',
    '"\\u00e9\\ud83d\\ude00\\n"', '  [ 1 , 2 ] ', '{"é": "ü", "k中": [1]}',
    '"中文"', 'NaN', '[Infinity, -Infinity]', '{"a": [1, {"b": null}]}', '"\\/"',
    '[true, false, null]', '"\\ud800\\udc00"', '"\\udc00\\ud800"', '"\\ud800"', '"\\ud800x"',
    '1' * 30, '-' + '9' * 20, '1.0' + '0' * 30, '[' * 50 + ']' * 50,
]
bad = [
    '', ' ', '[', ']', '{', '}', '[1,]', '{"a":}', '{"a" 1}', '{1: 2}', '[1 2]', '"abc', '"a\tb"',
    '"\\x"', '"\\u12"', '"\\u12zz"', '"\\u1234', 'tru', 'nul', 'NaN1', '-', '-a', '1.', '.5', '01',
    '1e', '1e+', '[1] [2]', 'é', '["é", x]', '{"éé": 1, x}', '"é\\q"',
    '[\n1,\n2,\n]', '"\\', '{"a":1,}', '[,]', '"\x7f\x80"', '{"a"', '{"a":', '{"a":1', '[1', '[1,',
    '﻿[]', '"\\ud800\\udc00', '1' * 5000,
]
for doc in docs:
    show("loads", lambda doc=doc: json.loads(doc))


def jde(fn):
    def run():
        try:
            return fn()
        except json.JSONDecodeError as e:
            return ("JDE", e.msg, e.pos, e.lineno, e.colno, str(e), e.args)
    return run


for doc in bad:
    show("bad", jde(lambda doc=doc: json.loads(doc)))
    show("bad nonstrict", jde(lambda doc=doc: json.loads(doc, strict=False)))
show("hooks", lambda: json.loads('{"a": {"b": 1}, "c": [{"d": 2}]}', object_hook=lambda d: sorted(d)))
show("pairs", lambda: json.loads('{"a": {"b": 1}, "a": 3}', object_pairs_hook=lambda p: p))
show("pairs od", lambda: json.loads('{"b": 1, "a": 2}', object_pairs_hook=collections.OrderedDict))
show("pairs wins", lambda: json.loads('{"k": 1}', object_hook=lambda d: 0, object_pairs_hook=lambda p: 1))
show("decimal", lambda: [str(v) + type(v).__name__ for v in json.loads('[1.10, 2, 3e2]', parse_float=decimal.Decimal, parse_int=str)])
show("parse_int float", lambda: json.loads('[1, 2]', parse_int=float))
show("parse_constant", lambda: json.loads('[NaN, -Infinity]', parse_constant=lambda c: "C" + c))
show("parse_constant raise", lambda: json.loads('[NaN]', parse_constant=lambda c: 1 / 0))
show("hook stopiter", lambda: json.loads('[{}]', object_hook=lambda d: next(iter([]))))
show("hook raise", lambda: json.loads('[{}]', object_hook=lambda d: {}[1]))
show("str subclass", lambda: json.loads(SS('{"k": [1]}')))
show("int", lambda: json.loads(5))
show("none", lambda: json.loads(None))
show("cls kw", lambda: json.loads("[1]", cls=json.JSONDecoder, foo=1))
show("strict kw", lambda: json.loads('"\t"', strict=False))
show("load", lambda: json.load(io.StringIO('{"x": [1, 2]}'), object_pairs_hook=list))
a1, a2 = json.loads('[{"key_long_enough_to_be_heap": 1}, {"key_long_enough_to_be_heap": 2}]')
print("memo", list(a1)[0] is list(a2)[0])
print("constants", json.loads("NaN") is json.decoder.NaN, json.loads("[-Infinity]")[0] is json.decoder.NegInf)

# ── 7. bytes input, detect_encoding, surrogatepass ───────────────────────────
print("=== 7. bytes ===")
for enc in ["utf-8", "utf-8-sig", "utf-16", "utf-16-le", "utf-16-be", "utf-32", "utf-32-le", "utf-32-be"]:
    raw = '["é\U0001f600", 1]'.encode(enc)
    show(enc, lambda raw=raw: (json.detect_encoding(raw), json.loads(raw), json.loads(bytearray(raw))))
    lone = '"\ud800"'.encode(enc, "surrogatepass")
    show(enc + " lone", lambda lone=lone: json.loads(lone))
show("bytes bad", lambda: json.loads(b'["\xff"]'))
show("bytes truncated", lambda: json.loads(b'["\xc3"]'))
show("bytes utf-16 odd", lambda: json.loads(b'\x00["\x00'))
show("bytes BOM str", jde(lambda: json.loads("﻿[]")))
show("bytes BOM utf-8-sig", lambda: json.loads(b"\xef\xbb\xbf[1]"))
show("memoryview", lambda: json.loads(memoryview(b"[1]")))
for b in [b"", b"a", b"ab", b"\x00a", b"a\x00", b"abc", b"\x00\x00\x00a", b"a\x00\x00\x00", b"a\x00b\x00", b"\x00a\x00b"]:
    show("detect", lambda b=b: json.detect_encoding(b))
show("detect mv", lambda: json.detect_encoding(memoryview(b"ab")))
show("detect str", lambda: json.detect_encoding("ab"))
show("decode surrogatepass", lambda: [b'"\xed\xa0\x80"'.decode("utf-8", "surrogatepass"), b"\x00\xd8".decode("utf-16-le", "surrogatepass")])

# ── 8. JSONDecoder, raw_decode, scanners ─────────────────────────────────────
print("=== 8. decoder ===")
dec = json.JSONDecoder()
show("raw", lambda: dec.raw_decode('xx [1, "é"] yy', 3))
show("raw2", lambda: dec.raw_decode('éé {"a": 1} ', 3))
show("raw neg", lambda: dec.raw_decode("[1]", -1))
show("raw big", jde(lambda: dec.raw_decode("[1]", 10)))
show("raw bytes", lambda: dec.raw_decode(b"[1]"))
show("raw float idx", lambda: dec.raw_decode("[1]", 1.0))
show("decode bytes", lambda: dec.decode(b"[1]"))
show("decode int", lambda: dec.decode(1))
show("decode extra", jde(lambda: dec.decode("[1] é")))
stream = 'é{"a": 1} [2, 3] "endé" éé'
pos, items = 1, []
while True:
    while stream[pos] == " ":
        pos += 1
    if stream[pos] == "é":
        break
    val, pos = dec.raw_decode(stream, pos)
    items.append(val)
print("stream", ascii(items), pos)
show("scan_once", lambda: dec.scan_once("  [1, 2] ", 2))
show("scan_once stop", lambda: dec.scan_once("[1, ]", 0))
show("scan_once neg", lambda: dec.scan_once("[1]", -1))
show("scan_once bytes", lambda: dec.scan_once(b"[1]", 0))
show("scan_once kw", lambda: dec.scan_once(string='"x"', idx=0))
show("scan_once end", lambda: dec.scan_once("[1]", 3))
show("make_scanner bad", lambda: json.scanner.make_scanner(object()))
show("make_scanner none", lambda: json.scanner.make_scanner())


class Dec(json.JSONDecoder):
    def raw_decode(self, s, idx=0):
        obj, end = super().raw_decode(s, idx)
        return ("RAW", obj), end


class Dec2(json.JSONDecoder):
    def decode(self, s):
        return ("DEC", super().decode(s))


class Dec3(json.JSONDecoder):
    def __init__(self, tag="T", **kw):
        self.tag = tag
        super().__init__(object_hook=self.hook, **kw)

    def hook(self, d):
        return (self.tag, sorted(d.items()))


show("Dec", lambda: json.loads('{"a": 1}', cls=Dec))
show("Dec2", lambda: json.loads('{"a": 1}', cls=Dec2))
show("Dec2 bytes", lambda: json.loads(b"[2]", cls=Dec2))
show("Dec3", lambda: json.loads('{"b": {"c": 1}}', cls=Dec3, tag="X", strict=False))

pydec = json.JSONDecoder(parse_int=lambda s: int(s) * 10, parse_constant=lambda c: "C:" + c)
pydec.scan_once = json.scanner.py_make_scanner(pydec)
cdec = json.JSONDecoder(parse_int=lambda s: int(s) * 10, parse_constant=lambda c: "C:" + c)
for doc in docs + bad:
    show("py", jde(lambda doc=doc: pydec.decode(doc)))
    show("c", jde(lambda doc=doc: cdec.decode(doc)))
show("py stop", lambda: json.scanner.py_make_scanner(dec)("[1,", 0))
show("py memo", lambda: (pydec.decode('{"a": {"a": 1}}'), pydec.memo))
show("JSONObject", lambda: json.decoder.JSONObject(('{"a": 1, "b": [2]}', 1), True, pydec.scan_once, None, None))
show("JSONObject pairs", lambda: json.decoder.JSONObject(('{ "a" : 1 }', 1), True, pydec.scan_once, None, list, {}))
show("JSONObject empty", lambda: json.decoder.JSONObject(("{ }", 1), True, pydec.scan_once, dict.copy, None))
show("JSONObject bad", jde(lambda: json.decoder.JSONObject(('{"a": 1,}', 1), True, pydec.scan_once, None, None)))
show("JSONObject bad2", jde(lambda: json.decoder.JSONObject(('{"a" 1}', 1), True, pydec.scan_once, None, None)))
show("JSONArray", lambda: json.decoder.JSONArray(("[ 1 , [2] ]", 1), pydec.scan_once))
show("JSONArray bad", jde(lambda: json.decoder.JSONArray(("[1 2]", 1), pydec.scan_once)))
show("JSONArray end", jde(lambda: json.decoder.JSONArray(("[", 1), pydec.scan_once)))
show("scanstring", lambda: json.decoder.scanstring('"a\\u00e9\\n"', 1))
show("scanstring strict", jde(lambda: json.decoder.scanstring('"a\tb"', 1, True)))
show("scanstring kw", lambda: json.decoder.scanstring('"a"', end=1))
show("scanstring oob", lambda: json.decoder.scanstring('"a"', 5))
show("scanstring neg", lambda: json.decoder.scanstring('"a"', -1))
show("scanstring 1", lambda: json.decoder.scanstring('"a"'))
show("scanstring bytes", lambda: json.decoder.scanstring(b'"a"', 1))
show("py_scanstring", lambda: json.decoder.py_scanstring('"a\\u00e9\\n"', 1))
show("py_scanstring ctrl", jde(lambda: json.decoder.py_scanstring('"a\x01"', 1)))
show("py_scanstring esc", jde(lambda: json.decoder.py_scanstring('"a\\é"', 1)))
show("py_scanstring sp", lambda: json.decoder.py_scanstring('"\\u 12f"', 1))
show("py_scanstring minus", lambda: json.decoder.py_scanstring('"\\u-12f"', 1))
show("py_scanstring 0x", jde(lambda: json.decoder.py_scanstring('"\\u0x12"', 1)))
show("py_scanstring pair", lambda: json.decoder.py_scanstring('"\\ud83d\\ude00"', 1))
show("py_scanstring lone", lambda: json.decoder.py_scanstring('"\\ud83d\\u0041"', 1))
show("py_scanstring oob", jde(lambda: json.decoder.py_scanstring('"abc"', 9)))
show("py_scanstring nonstrict", lambda: json.decoder.py_scanstring('"a\nb"', 1, False))
show("encode_basestring", lambda: json.encoder.encode_basestring('\x00"\x7fé\ud800'))
show("encode_basestring_ascii", lambda: json.encoder.encode_basestring_ascii('\x00"\x7fé\ud800\U0001f600'))
show("py_encode_basestring_ascii", lambda: json.encoder.py_encode_basestring_ascii('\x7fé'))

# ── 9. JSONDecodeError ───────────────────────────────────────────────────────
print("=== 9. JSONDecodeError ===")
e = json.JSONDecodeError("Bad", "ab\ncd\nef", 7)
print(e.msg, e.pos, e.lineno, e.colno, str(e), e.args, repr(e))
e2 = pickle.loads(pickle.dumps(e))
print(type(e2).__name__, e2.msg, e2.pos, e2.lineno, e2.colno, str(e2), e.__reduce__())
show("jde neg", lambda: str(json.JSONDecodeError("m", "a\nb", -1)))
show("jde big", lambda: str(json.JSONDecodeError("m", "a\nb", 10)))
show("jde bytes", lambda: str(json.JSONDecodeError("m", b"a\nb", 2)))
show("jde nonascii", lambda: str(json.JSONDecodeError("m", "é\néé", 4)))
try:
    json.loads("[1, 2")
except ValueError as err:
    print("caught as ValueError", type(err).__name__, err)


class MyErr(json.JSONDecodeError):
    pass


show("subclass", lambda: (str(MyErr("x", "abc", 1)), MyErr("x", "abc", 1).__reduce__()[0].__name__))

# ── 10. Nesting limits ───────────────────────────────────────────────────────
print("=== 10. nesting ===")
show("loads deep list", lambda: json.loads("[" * 20000 + "]" * 20000))
show("loads deep dict", lambda: json.loads('{"a":' * 20000 + "1" + "}" * 20000))
deep = []
for _ in range(20000):
    deep = [deep]
show("dumps deep", lambda: json.dumps(deep))
show("dumps deep indent", lambda: json.dumps(deep, indent=1))
show("iterencode deep", lambda: len(list(json.JSONEncoder().iterencode(deep))))
show("loads 500", lambda: len(json.dumps(json.loads("[" * 500 + "]" * 500))))
show("dumps 500 indent", lambda: len(json.dumps(json.loads("[" * 500 + "]" * 500), indent=1)))
print(json.dumps({"k": json.loads(json.dumps({"x": [1, 2.5, "é"]}))}, sort_keys=True, indent=4))
