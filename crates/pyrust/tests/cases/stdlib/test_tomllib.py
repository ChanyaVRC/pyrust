# CPython 3.12 parity tests for `tomllib` (PEP 680, issue #3184).

import datetime
import decimal
import io
import math
import tomllib

# --- Module surface ---
print("all:", tomllib.__all__)
print("err_sub:", issubclass(tomllib.TOMLDecodeError, ValueError))
print("err_mod:", tomllib.TOMLDecodeError.__module__)

# --- Empty / whitespace / comments ---
print("empty:", tomllib.loads(""))
print("ws:", tomllib.loads("   \t  \n\r\n  # full line comment\n\t# tab comment"))
print("crlf:", tomllib.loads("a = 1\r\nb = 2\r\n"))

# --- Keys: bare, quoted, empty, dotted, unicode ---
keys_toml = """
bare_key = 1
bare-key = 2
1234 = 3
"127.0.0.1" = "value"
"character encoding" = "value"
"ʎǝʞ" = "value"
'key2' = "value"
'quoted "value"' = "value"
"" = "blank"
'' = "error_not_used"
"""
print("keys:", tomllib.loads("""
bare_key = 1
bare-key = 2
1234 = 3
"127.0.0.1" = "value"
"character encoding" = "value"
"ʎǝʞ" = "value"
'key2' = "value"
'quoted "value"' = "value"
"" = "blank"
"""))

print("dotted:", tomllib.loads("""
name = "Orange"
physical.color = "orange"
physical.shape = "round"
site."google.com" = true
fruit.name = "banana"
fruit. color = "yellow"
fruit . flavor = "banana"
3.14159 = "pi"
"""))

# --- Strings: basic, multiline basic, literal, multiline literal ---
print("basic_esc:", tomllib.loads(r'''
b = "\b"
t = "\t"
n = "\n"
f = "\f"
r = "\r"
q = "\""
bs = "\\"
u4 = "\u0041\u03B1\u2603"
u8 = "\U00000041\U0001F600"
'''))

print("ml_basic:", tomllib.loads(r'''
s1 = """
Roses are red
Violets are blue"""
s2 = """\
  The quick brown \


  fox jumps over \
    the lazy dog.\
"""
s3 = """\
The quick brown \
fox jumps over \
the lazy dog."""
s4 = """Here are two quotation marks: "". Simple enough."""
s5 = """Here are three quotation marks: ""\"."""
s6 = """"This," she said, "is just a pointless statement.""""
s7 = """""five quotes at end"""""
'''))

print("literal:", tomllib.loads(r"""
winpath  = 'C:\Users\nodejs\templates'
winpath2 = '\\ServerX\admin$\system32\'
quoted   = 'Tom "Dubs" Preston-Werner'
regex    = '<\i\c*\s*>'
"""))

print("ml_literal:", tomllib.loads("""
regex2 = '''I [dw]on't need \\d{2} apples'''
lines  = '''
The first newline is
trimmed in raw strings.
   All other whitespace
   is preserved.
'''
quot15 = '''Here are fifteen quotation marks: \"\"\"\"\"\"\"\"\"\"\"\"\"\"\"'''
apos15 = ''''That,' she said, 'is still pointless.''''
five = '''''two apos'''''
"""))

# --- Integers ---
print("ints:", tomllib.loads("""
int1 = +99
int2 = 42
int3 = 0
int4 = -17
int5 = 1_000
int6 = 5_349_221
int7 = 53_49_221
int8 = 1_2_3_4_5
pos0 = +0
neg0 = -0
hex1 = 0xDEADBEEF
hex2 = 0xdeadbeef
hex3 = 0xdead_beef
oct1 = 0o01234567
oct2 = 0o755
bin1 = 0b11010110
bin2 = 0b1101_0110
big = 9223372036854775807
negbig = -9223372036854775808
"""))

# --- Floats & special floats ---
fl = tomllib.loads("""
flt1 = +1.0
flt2 = 3.1415
flt3 = -0.01
flt4 = 5e+22
flt5 = 1e06
flt6 = -2E-2
flt7 = 6.626e-34
flt8 = 224_617.445_991_228
flt9 = 0.0
flt10 = -0.0
flt11 = +0.0
sf1 = inf
sf2 = +inf
sf3 = -inf
sf4 = nan
sf5 = +nan
sf6 = -nan
""")
for k in ("flt1", "flt2", "flt3", "flt4", "flt5", "flt6", "flt7", "flt8", "flt9"):
    print(k, fl[k], type(fl[k]).__name__)
print("neg_zero:", math.copysign(1.0, fl["flt10"]))
print("pos_zero:", math.copysign(1.0, fl["flt11"]))
print("infs:", math.isinf(fl["sf1"]), fl["sf1"] > 0, fl["sf2"] > 0, fl["sf3"] < 0)
print("nans:", math.isnan(fl["sf4"]), math.isnan(fl["sf5"]), math.isnan(fl["sf6"]))

# --- parse_float callback ---
pf_dec = tomllib.loads(
    "a = 1.5\nb = 1_000.25\nc = inf\nd = -inf\ne = nan\ni = 42\n",
    parse_float=decimal.Decimal,
)
print("pf_dec:", pf_dec["a"], type(pf_dec["a"]).__name__, pf_dec["b"], pf_dec["c"], pf_dec["d"], pf_dec["e"].is_nan(), pf_dec["i"], type(pf_dec["i"]).__name__)

pf_str = tomllib.loads("x = 1_2.3_4e+5\ny = +nan\n", parse_float=str)
print("pf_str:", pf_str)

for bad_pf in (list, dict):
    try:
        tomllib.loads("x = 1.5", parse_float=bad_pf)
        print("bad_pf_fail", bad_pf.__name__)
    except ValueError as e:
        print("bad_pf_ok", bad_pf.__name__, type(e).__name__)

# --- Booleans ---
print("bools:", tomllib.loads("t = true\nf = false\n"))

# --- Dates and Times (RFC 3339) ---
dt = tomllib.loads("""
odt1 = 1979-05-27T07:32:00Z
odt2 = 1979-05-27T00:32:00-07:00
odt3 = 1979-05-27T00:32:00.999999-07:00
odt4 = 1979-05-27 07:32:00Z
odt5 = 1979-05-27t07:32:00z
odt6 = 1979-05-27T07:32:00+05:30
odt7 = 1979-05-27T07:32:00.123456789+00:00
ldt1 = 1979-05-27T07:32:00
ldt2 = 1979-05-27T00:32:00.999999
ldt3 = 1979-05-27 07:32:00.5
ld1 = 1979-05-27
ld2 = 2024-02-29 # leap day with comment
lt1 = 07:32:00
lt2 = 00:32:00.999999
lt3 = 23:59:59.1
lt4 = 12:34:56.123456999
""")
for k, v in dt.items():
    print(k, repr(v), type(v).__name__)
print("odt1_utc:", dt["odt1"].tzinfo is datetime.timezone.utc)
print("odt5_utc:", dt["odt5"].tzinfo is datetime.timezone.utc)
print("odt2_off:", dt["odt2"].utcoffset())
print("odt6_off:", dt["odt6"].utcoffset())
print("ldt1_naive:", dt["ldt1"].tzinfo is None)

# --- Arrays ---
print("arrays:", tomllib.loads("""
empty = []
ints = [ 1, 2, 3 ]
colors = [ "red", "yellow", "green" ]
nested = [ [ 1, 2 ], [3, 4, 5] ]
mixed = [ 0.1, 0.2, 0.5, 1, 2, 5 ]
trailing = [
  1, # first
  2, # second
  3, # trailing comma
]
inline_tables = [ { x = 1 }, { x = 2 } ]
"""))

# --- Inline tables ---
print("inline_tables:", tomllib.loads("""
empty = {}
name = { first = "Tom", last = "Preston-Werner" }
point = { x = 1, y = 2 }
animal = { type.name = "pug", type.age = 3 }
nested = { a = { b = { c = [1, 2] } } }
"""))

# --- Standard tables & super-tables ---
print("tables:", tomllib.loads("""
[table-1]
key1 = "some string"
key2 = 123

[table-2]
key1 = "another string"
key2 = 456

[dog."tater.man"]
type.name = "pug"

[a.b.c]
d = 1

[a]
e = 2

[x.y.z.w]
[x]
v = 3
"""))

# --- Arrays of tables ---
print("aot:", tomllib.loads("""
[[products]]
name = "Hammer"
sku = 738594937

[[products]]  # empty table within the array

[[products]]
name = "Nail"
sku = 284758393
color = "gray"

[[fruits]]
name = "apple"

[fruits.physical]
color = "red"
shape = "round"

[[fruits.varieties]]
name = "red delicious"

[[fruits.varieties]]
name = "granny smith"

[[fruits]]
name = "banana"

[[fruits.varieties]]
name = "plantain"
"""))

# --- load() from binary file-like object ---
buf = io.BytesIO(b'title = "TOML"\n[owner]\nname = "Alice"\npi = 3.14\n')
print("load_bytesio:", tomllib.load(buf))
buf2 = io.BytesIO(b"val = 2.5\n")
print("load_parse_float:", tomllib.load(buf2, parse_float=decimal.Decimal))

# --- Type errors for load / loads ---
for bad in (b"a = 1", 123, None, ["a = 1"]):
    try:
        tomllib.loads(bad)
        print("loads_type_fail", type(bad).__name__)
    except TypeError as e:
        print("loads_type_ok", type(bad).__name__, str(e))

try:
    tomllib.load(io.StringIO("a = 1"))
    print("load_stringio_fail")
except TypeError as e:
    print("load_stringio_ok", str(e))

try:
    tomllib.load(io.BytesIO(b"\xff\xfe"))
    print("load_bad_utf8_fail")
except UnicodeDecodeError:
    print("load_bad_utf8_ok")

# Positional-only check
try:
    tomllib.loads(s="a = 1")
    print("posonly_loads_fail")
except TypeError:
    print("posonly_loads_ok")

try:
    tomllib.load(fp=io.BytesIO(b"a = 1"))
    print("posonly_load_fail")
except TypeError:
    print("posonly_load_ok")

try:
    tomllib.loads("a = 1.0", float)
    print("kwonly_loads_fail")
except TypeError:
    print("kwonly_loads_ok")

# --- Invalid TOML documents (must raise TOMLDecodeError) ---
invalid_cases = [
    ("dup_key", "a = 1\na = 2\n"),
    ("dup_dotted_key", "a.b = 1\na.b = 2\n"),
    ("dup_table", "[a]\nx = 1\n[a]\ny = 2\n"),
    ("redef_table_via_dotted", "[a]\nb = 1\n[c]\n[a.d]\nx = 1\n"),
    ("redef_dotted_as_table", "a.b = 1\n[a]\nc = 2\n"),
    ("redef_dotted_in_other_table", "a.b = 1\n[other]\n[a.b]\nx = 1\n"),
    ("overwrite_scalar_table", "a = 1\n[a]\nb = 2\n"),
    ("overwrite_scalar_dotted", "a = 1\na.b = 2\n"),
    ("mutate_inline_table_dotted", "a = { b = 1 }\na.c = 2\n"),
    ("mutate_inline_table_header", "a = { b = 1 }\n[a]\nc = 2\n"),
    ("mutate_inline_table_sub", "a = { b = 1 }\n[a.c]\nd = 2\n"),
    ("mutate_inline_array_aot", "a = []\n[[a]]\nb = 1\n"),
    ("inline_table_trailing_comma", "a = { b = 1, }\n"),
    ("inline_table_newline", "a = {\nb = 1\n}\n"),
    ("inline_table_dup_key", "a = { b = 1, b = 2 }\n"),
    ("inline_table_mutate_nested", "a = { b = { c = 1 }, b.d = 2 }\n"),
    ("unclosed_inline_table", "a = { b = 1\n"),
    ("unclosed_array", "a = [1, 2\n"),
    ("array_double_comma", "a = [1,, 2]\n"),
    ("leading_zero_int", "a = 01\n"),
    ("leading_zero_neg", "a = -01\n"),
    ("leading_zero_double", "a = 00\n"),
    ("leading_underscore_int", "a = _1\n"),
    ("trailing_underscore_int", "a = 1_\n"),
    ("double_underscore_int", "a = 1__2\n"),
    ("signed_hex", "a = +0x1\n"),
    ("signed_oct", "a = -0o7\n"),
    ("signed_bin", "a = +0b1\n"),
    ("hex_leading_underscore", "a = 0x_1\n"),
    ("hex_trailing_underscore", "a = 0x1_\n"),
    ("float_no_frac", "a = 1.\n"),
    ("float_no_int", "a = .5\n"),
    ("float_bad_underscore_before_dot", "a = 1_.5\n"),
    ("float_bad_underscore_after_dot", "a = 1._5\n"),
    ("float_bad_exp", "a = 1e\n"),
    ("float_bad_exp_underscore", "a = 1e_5\n"),
    ("capital_inf", "a = Inf\n"),
    ("capital_nan", "a = NaN\n"),
    ("invalid_date_feb29", "d = 2023-02-29\n"),
    ("invalid_date_month", "d = 2023-13-01\n"),
    ("invalid_date_day", "d = 2023-04-31\n"),
    ("invalid_time_hour", "t = 24:00:00\n"),
    ("invalid_time_min", "t = 12:60:00\n"),
    ("invalid_time_sec", "t = 12:00:60\n"),
    ("invalid_escape", r's = "\a"' + "\n"),
    ("invalid_unicode_short", r's = "\u123"' + "\n"),
    ("invalid_unicode_surrogate", r's = "\uD800"' + "\n"),
    ("invalid_unicode_range", r's = "\U00110000"' + "\n"),
    ("unterminated_basic_str", 's = "hello\n'),
    ("unterminated_literal_str", "s = 'hello\n"),
    ("unterminated_ml_basic", 's = """hello'),
    ("unterminated_ml_literal", "s = '''hello"),
    ("six_quotes_ml_basic", 's = """a""""""\n'),
    ("six_apos_ml_literal", "s = '''a''''''\n"),
    ("ctrl_char_in_basic", 's = "a\x00b"\n'),
    ("ctrl_char_in_literal", "s = 'a\x07b'\n"),
    ("ctrl_char_in_comment", "# bad\x08comment\n"),
    ("lone_cr_in_basic", 's = "a\rb"\n'),
    ("lone_cr_in_ml_basic", 's = """a\rb"""\n'),
    ("missing_eq", "a 1\n"),
    ("missing_value", "a =\n"),
    ("missing_key", "= 1\n"),
    ("empty_table_header", "[]\n"),
    ("empty_aot_header", "[[]]\n"),
    ("unclosed_table_header", "[a\n"),
    ("unclosed_aot_header", "[[a]\n"),
    ("two_kv_same_line", "a = 1 b = 2\n"),
    ("bad_bool_suffix", "a = truefalse\n"),
    ("ml_key", '"""a""" = 1\n'),
    ("bad_line_ending_backslash", 's = """\\\n"""\nx = """\\ a\n"""\n'),
    ("table_then_aot_same_key", "[a]\nb = 1\n[[a]]\nc = 2\n"),
    ("aot_then_table_same_key", "[[a]]\nb = 1\n[a]\nc = 2\n"),
]

# Note: "redef_table_via_dotted" above is actually VALID TOML (adding subtable [a.d] after [a] is allowed!).
# Let's check each invalid case except that one, and explicitly verify the valid subtable case too!
print("valid_subtable:", tomllib.loads("[a]\nb = 1\n[c]\n[a.d]\nx = 1\n"))

for name, bad_toml in invalid_cases:
    if name == "redef_table_via_dotted":
        continue
    try:
        res = tomllib.loads(bad_toml)
        print("SHOULD_FAIL:", name, res)
    except tomllib.TOMLDecodeError as e:
        print("ERR_OK:", name, isinstance(e, ValueError))

# Also test that redefining an explicit table via dotted keys inside another section fails:
try:
    tomllib.loads("[a.b]\nx = 1\n[a]\nb.y = 2\n")
    print("SHOULD_FAIL: dotted_redef_explicit_subtable")
except tomllib.TOMLDecodeError:
    print("ERR_OK: dotted_redef_explicit_subtable")

print("tomllib parity ok")
