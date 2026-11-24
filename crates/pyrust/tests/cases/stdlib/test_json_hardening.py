"""CPython 3.12 parity tests for json hardening (issue #3155)."""

import io
import json

# ── 1. bytes and bytearray input to loads() / load() / detect_encoding() ──────
sample = '{"a": 1, "b": "café", "c": [true, null]}'
for enc in [
    "utf-8",
    "utf-8-sig",
    "utf-16",
    "utf-16-le",
    "utf-16-be",
    "utf-32",
    "utf-32-le",
    "utf-32-be",
]:
    raw = sample.encode(enc)
    print(enc, json.detect_encoding(raw), json.loads(raw))

# bytearray input
print("bytearray:", json.loads(bytearray(b'[1, 2, {"k": "v"}]')))

# load() from binary stream
print("BytesIO:", json.load(io.BytesIO(sample.encode("utf-16"))))

# UTF-8 BOM in a str must raise JSONDecodeError
try:
    json.loads("\ufeff{}")
    print("str-bom NO ERROR")
except json.JSONDecodeError as e:
    print("str-bom:", e.pos, "BOM" in e.msg)


# ── 2. JSONEncoder & JSONDecoder classes and cls= ─────────────────────────────
enc = json.JSONEncoder(sort_keys=True, separators=(",", ":"))
print("encoder.encode:", enc.encode({"b": 2, "a": [1, 3]}))
print("encoder.iterencode:", "".join(enc.iterencode({"b": 2, "a": [1, 3]})))


class CustomEncoder(json.JSONEncoder):
    def default(self, o):
        if isinstance(o, set):
            return {"__set__": sorted(o)}
        return super().default(o)


print("cls dumps:", json.dumps({"s": {3, 1, 2}}, cls=CustomEncoder))
buf = io.StringIO()
json.dump({"s": {2, 1}}, buf, cls=CustomEncoder)
print("cls dump:", buf.getvalue())

dec = json.JSONDecoder()
print("decoder.decode:", dec.decode('  {"x": [1, 2]}  '))
print("decoder.raw_decode:", dec.raw_decode('  [1, 2] trailing', 2))

# Streaming multiple values with raw_decode
stream = '{"a": 1} [2, 3] "end"'
pos = 0
items = []
while pos < len(stream):
    while pos < len(stream) and stream[pos] in " \t\n\r":
        pos += 1
    if pos >= len(stream):
        break
    val, pos = dec.raw_decode(stream, pos)
    items.append(val)
print("raw_decode stream:", items)

# strict=False allows unescaped control chars in strings
print("strict=False:", repr(json.JSONDecoder(strict=False).decode('"a\nb\tc"')))
try:
    json.JSONDecoder(strict=True).decode('"a\nb"')
    print("strict=True NO ERROR")
except json.JSONDecodeError:
    print("strict=True JSONDecodeError")


class TagDecoder(json.JSONDecoder):
    def __init__(self, tag="T", **kw):
        self.tag = tag
        super().__init__(object_hook=self._hook, **kw)

    def _hook(self, d):
        return (self.tag, sorted(d.items()))


print("cls loads:", json.loads('{"b": 2, "a": 1}', cls=TagDecoder, tag="X"))
print("cls load:", json.load(io.StringIO('{"k": 9}'), cls=TagDecoder, tag="Y"))


# ── 3. Decoder hooks: object_hook, object_pairs_hook, parse_* ─────────────────
print(
    "object_hook:",
    json.loads('{"x": 1, "inner": {"y": 2}}', object_hook=lambda d: ("obj", sorted(d.items()))),
)
print(
    "object_pairs_hook:",
    json.loads('{"a": 1, "b": 2, "a": 3}', object_pairs_hook=lambda pairs: pairs),
)
# object_pairs_hook takes precedence over object_hook
print(
    "pairs wins:",
    json.loads(
        '{"k": 1}',
        object_hook=lambda d: "OBJECT_HOOK",
        object_pairs_hook=lambda p: ("PAIRS", p),
    ),
)
print(
    "parse_int/float/const:",
    json.loads(
        '[1, -20, 3.5, 1e2, NaN, Infinity, -Infinity]',
        parse_int=lambda s: ("I", s),
        parse_float=lambda s: ("F", s),
        parse_constant=lambda s: ("C", s),
    ),
)


# ── 4. allow_nan & check_circular & skipkeys ──────────────────────────────────
for bad in [float("nan"), float("inf"), float("-inf")]:
    try:
        json.dumps(bad, allow_nan=False)
        print("allow_nan NO ERROR")
    except ValueError as e:
        print("allow_nan:", "Out of range float" in str(e))

try:
    json.dumps({float("nan"): 1}, allow_nan=False)
    print("allow_nan key NO ERROR")
except ValueError:
    print("allow_nan key ValueError")

# Circular reference detection
circ_list = []
circ_list.append(circ_list)
try:
    json.dumps(circ_list)
    print("circ list NO ERROR")
except ValueError as e:
    print("circ list:", str(e))

circ_dict = {}
circ_dict["self"] = circ_dict
try:
    json.dumps(circ_dict)
    print("circ dict NO ERROR")
except ValueError as e:
    print("circ dict:", str(e))


class LoopObj:
    pass


try:
    json.dumps(LoopObj(), default=lambda o: o)
    print("circ default NO ERROR")
except ValueError as e:
    print("circ default:", str(e))

# Non-circular shared references are allowed
shared = [1, 2]
print("shared ok:", json.dumps([shared, shared]))

# skipkeys=True
print("skipkeys:", json.dumps({(1, 2): "skip", "keep": 42}, skipkeys=True))

print("json hardening ok")
