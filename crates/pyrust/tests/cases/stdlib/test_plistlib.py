import collections
import datetime
import enum
import io
import os
import plistlib
import tempfile

# 1. Constants, enum, exception hierarchy, UID
assert issubclass(plistlib.InvalidFileException, ValueError)
assert isinstance(plistlib.FMT_XML, plistlib.PlistFormat)
assert isinstance(plistlib.FMT_BINARY, plistlib.PlistFormat)
assert isinstance(plistlib.FMT_XML, enum.Enum)
assert plistlib.FMT_XML.name == "FMT_XML"
assert plistlib.FMT_XML.value == 1
assert plistlib.FMT_BINARY.name == "FMT_BINARY"
assert plistlib.FMT_BINARY.value == 2
assert plistlib.FMT_XML is plistlib.PlistFormat.FMT_XML
assert plistlib.FMT_BINARY is plistlib.PlistFormat.FMT_BINARY

u0 = plistlib.UID(0)
u1 = plistlib.UID(42)
u2 = plistlib.UID(42)
u_big = plistlib.UID((1 << 64) - 1)
assert u1.data == 42
assert int(u1) == 42
assert u1.__index__() == 42
assert u1 == u2
assert u1 != u0
assert u1 != 42
assert hash(u1) == hash(42)
assert repr(u1) == "UID(42)"
assert repr(u_big) == f"UID({(1 << 64) - 1})"
assert {u1: "yes"}[u2] == "yes"

for bad_uid, exc_ty in (
    ("42", TypeError),
    (1.5, TypeError),
    (-1, ValueError),
    (1 << 64, ValueError),
):
    try:
        plistlib.UID(bad_uid)
        assert False, f"expected {exc_ty} for {bad_uid!r}"
    except exc_ty:
        pass
print("uid_and_enum_ok")

# 2. Rich payload for XML & Binary byte-for-byte parity
payload = {
    "aString": "Doodah",
    "aSpecial": "<hello & 'hi' \"there\"!>",
    "aUnicode": "Caf\xe9 \u65e5\u672c\u8a9e \U0001f680",
    "aList": ["A", "B", 12, -456, 32.5, [1, 2, 3], []],
    "aFloat": 0.25,
    "anInt": 728,
    "aNegInt": -9876543210,
    "aBigUint": (1 << 63) + 12345,
    "aDict": {
        "aTrueValue": True,
        "aFalseValue": False,
        "emptyDict": {},
        "emptyStr": "",
    },
    "someData": b"<binary gunk>\x00\xff",
    "longData": bytes(range(128)),
    "emptyData": b"",
    "aDate": datetime.datetime(2024, 6, 15, 12, 34, 56),
    "oldDate": datetime.datetime(1999, 12, 31, 23, 59, 59),
}

# Exact XML output comparison with CPython
xml_bytes = plistlib.dumps(payload, fmt=plistlib.FMT_XML)
print("xml_exact:")
print(xml_bytes.decode("utf-8"))
parsed_xml = plistlib.loads(xml_bytes)
assert parsed_xml == payload
assert parsed_xml["aDict"]["aTrueValue"] is True
assert parsed_xml["aDict"]["aFalseValue"] is False
assert isinstance(parsed_xml["aBigUint"], int)
assert parsed_xml["aBigUint"] == (1 << 63) + 12345
assert parsed_xml["aDate"] == datetime.datetime(2024, 6, 15, 12, 34, 56)
assert parsed_xml["oldDate"] == datetime.datetime(1999, 12, 31, 23, 59, 59)
print("xml_roundtrip_ok")

# Exact Binary (bplist00) output comparison with CPython
bin_bytes = plistlib.dumps(payload, fmt=plistlib.FMT_BINARY)
assert bin_bytes.startswith(b"bplist00")
print("bin_hex:", bin_bytes.hex())
parsed_bin = plistlib.loads(bin_bytes)
assert parsed_bin == payload
assert parsed_bin["aDict"]["aTrueValue"] is True
assert parsed_bin["aDict"]["aFalseValue"] is False
assert parsed_bin["aBigUint"] == (1 << 63) + 12345
print("bin_roundtrip_ok")

# 3. Binary-only types: None, UID (1/2/4/8 byte widths), deduplication, bytearray, tuples
bin_special = {
    "none": None,
    "uids": [
        plistlib.UID(0),
        plistlib.UID(255),
        plistlib.UID(256),
        plistlib.UID(65535),
        plistlib.UID(65536),
        plistlib.UID(1 << 32),
        plistlib.UID((1 << 64) - 1),
    ],
    "dedup": ["same", "same", 123456, 123456, True, 1, False, 0, b"dup", b"dup"],
    "ba": bytearray(b"mutable"),
    "tup": (1, "two", 3.0),
    "ints": [0, 1, 255, 256, 65535, 65536, (1 << 32) - 1, 1 << 32, -1, -(1 << 63)],
}
bin_special_bytes = plistlib.dumps(bin_special, fmt=plistlib.FMT_BINARY)
print("bin_special_hex:", bin_special_bytes.hex())
rt_special = plistlib.loads(bin_special_bytes, fmt=plistlib.FMT_BINARY)
assert rt_special["none"] is None
assert rt_special["uids"] == bin_special["uids"]
assert rt_special["dedup"] == bin_special["dedup"]
assert rt_special["dedup"][4] is True and type(rt_special["dedup"][5]) is int
assert rt_special["dedup"][6] is False and type(rt_special["dedup"][7]) is int
assert rt_special["ba"] == b"mutable"
assert rt_special["tup"] == [1, "two", 3.0]
assert rt_special["ints"] == bin_special["ints"]
print("bin_special_ok")

# 4. sort_keys, skipkeys, dict_type, load/dump on BytesIO & files
unsorted = {"z": 1, "a": 2, "m": 3}
xml_unsorted = plistlib.dumps(unsorted, sort_keys=False)
assert xml_unsorted.index(b"<key>z</key>") < xml_unsorted.index(b"<key>a</key>")
od = plistlib.loads(xml_unsorted, dict_type=collections.OrderedDict)
assert isinstance(od, collections.OrderedDict)
assert list(od.keys()) == ["z", "a", "m"]

bin_unsorted = plistlib.dumps(unsorted, fmt=plistlib.FMT_BINARY, sort_keys=False)
od_bin = plistlib.loads(bin_unsorted, dict_type=collections.OrderedDict)
assert isinstance(od_bin, collections.OrderedDict)
assert list(od_bin.keys()) == ["z", "a", "m"]

mixed_keys = {"ok": 1, 99: "skipme"}
for fmt in (plistlib.FMT_XML, plistlib.FMT_BINARY):
    try:
        plistlib.dumps(mixed_keys, fmt=fmt, sort_keys=False)
        assert False
    except TypeError:
        pass
    dumped = plistlib.dumps(mixed_keys, fmt=fmt, sort_keys=False, skipkeys=True)
    assert plistlib.loads(dumped) == {"ok": 1}

buf = io.BytesIO()
plistlib.dump({"hello": "stream"}, buf, fmt=plistlib.FMT_BINARY)
buf.seek(0)
assert plistlib.load(buf) == {"hello": "stream"}

with tempfile.TemporaryDirectory() as tmpdir:
    p = os.path.join(tmpdir, "test.plist")
    with open(p, "wb") as f:
        plistlib.dump({"disk": [1, 2, 3]}, f, fmt=plistlib.FMT_XML)
    with open(p, "rb") as f:
        assert plistlib.load(f) == {"disk": [1, 2, 3]}
print("options_and_io_ok")

# 5. XML quirks: <plist> without <?xml, hex integer, UTF-8 BOM, UTF-16 BOM, CDATA, comments
xml_no_decl = b'<plist version="1.0"><dict><key>hex</key><integer>0x1a</integer><!-- comment --></dict></plist>'
assert plistlib.loads(xml_no_decl) == {"hex": 26}
assert plistlib.loads(b"\xef\xbb\xbf" + xml_no_decl) == {"hex": 26}
utf16_plist = '<?xml version="1.0" encoding="UTF-16"?><plist version="1.0"><string>hi</string></plist>'.encode("utf-16")
assert plistlib.loads(utf16_plist) == "hi"
print("xml_quirks_ok")

# 6. Error handling: invalid files, entity declarations, control chars, overflow, bad fmt
for bad_data in (b"", b"not a plist", b"bplist00" + b"\x00" * 10):
    try:
        plistlib.loads(bad_data)
        assert False, f"expected InvalidFileException for {bad_data!r}"
    except plistlib.InvalidFileException:
        pass

xxe = b'<?xml version="1.0"?><!DOCTYPE plist [<!ENTITY xxe "evil">]><plist version="1.0"><string>&xxe;</string></plist>'
try:
    plistlib.loads(xxe)
    assert False, "expected InvalidFileException for entity decl"
except plistlib.InvalidFileException:
    pass

try:
    plistlib.dumps({"bad": "a\x00b"}, fmt=plistlib.FMT_XML)
    assert False
except ValueError:
    pass

for fmt in (plistlib.FMT_XML, plistlib.FMT_BINARY):
    try:
        plistlib.dumps({"huge": 1 << 64}, fmt=fmt)
        assert False
    except OverflowError:
        pass
    try:
        plistlib.dumps({"neg_huge": -(1 << 64)}, fmt=fmt)
        assert False
    except OverflowError:
        pass

try:
    plistlib.dumps({"a": 1}, fmt=999)
    assert False
except ValueError:
    pass

try:
    plistlib.dumps({"none_in_xml": None}, fmt=plistlib.FMT_XML)
    assert False
except TypeError:
    pass
print("errors_ok")
