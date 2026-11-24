# CPython 3.12 parity tests for `configparser` (issue #3185).

import io
import os
import sys
import tempfile
import configparser
from configparser import (
    DEFAULTSECT,
    MAX_INTERPOLATION_DEPTH,
    Error,
    NoSectionError,
    DuplicateSectionError,
    NoOptionError,
    DuplicateOptionError,
    InterpolationError,
    InterpolationMissingOptionError,
    InterpolationSyntaxError,
    InterpolationDepthError,
    ParsingError,
    MissingSectionHeaderError,
    Interpolation,
    BasicInterpolation,
    ExtendedInterpolation,
    RawConfigParser,
    ConfigParser,
    SectionProxy,
    ConverterMapping,
)

# 1. Module metadata & constants & exception hierarchy
print("mod_id:", configparser is sys.modules["configparser"])
print("all:", configparser.__all__)
print("consts:", DEFAULTSECT, MAX_INTERPOLATION_DEPTH)
print(
    "exc_hier:",
    issubclass(NoSectionError, Error),
    issubclass(DuplicateSectionError, Error),
    issubclass(NoOptionError, Error),
    issubclass(DuplicateOptionError, Error),
    issubclass(InterpolationError, Error),
    issubclass(InterpolationMissingOptionError, InterpolationError),
    issubclass(InterpolationSyntaxError, InterpolationError),
    issubclass(InterpolationDepthError, InterpolationError),
    issubclass(ParsingError, Error),
    issubclass(MissingSectionHeaderError, ParsingError),
)

# 2. Basic parsing, sections, defaults, getters, mapping interface
ini = """
# Top comment
; Semicolon comment
[DEFAULT]
ServerAliveInterval = 45
Compression = yes
CompressionLevel = 9
ForwardX11 = no
base_dir = /opt/app

[forge.example]
User = hg
Port = 50022
Ratio = 3.14
data_dir = %(base_dir)s/data
escaped = 100%%

[topsecret.server.example]
Port = 50022
ForwardX11 = yes
Multiline = line1
    line2

    line4
"""

cp = ConfigParser()
cp.read_string(ini)
print("sections:", cp.sections())
print("has_sec:", cp.has_section("forge.example"), cp.has_section("DEFAULT"), cp.has_section("missing"))
print("in_cp:", "DEFAULT" in cp, "forge.example" in cp, "missing" in cp)
print("len_iter:", len(cp), list(cp))
print("options:", cp.options("forge.example"))
print("has_opt:", cp.has_option("forge.example", "User"), cp.has_option("forge.example", "compression"), cp.has_option("missing", "x"))
print("get:", cp.get("forge.example", "user"), cp.get("forge.example", "data_dir"), cp.get("forge.example", "data_dir", raw=True))
print("escaped:", cp.get("forge.example", "escaped"))
print("get_vars:", cp.get("forge.example", "data_dir", vars={"base_dir": "/tmp"}))
print("get_fallback:", cp.get("forge.example", "nope", fallback="fb"), cp.get("no_sec", "nope", fallback=None))
print("getint:", cp.getint("forge.example", "port"), cp.getint("forge.example", "missing", fallback=80))
print("getfloat:", cp.getfloat("forge.example", "ratio"), cp.getfloat("forge.example", "missing", fallback=1.5))
print(
    "getbool:",
    cp.getboolean("forge.example", "compression"),
    cp.getboolean("forge.example", "forwardx11"),
    cp.getboolean("topsecret.server.example", "forwardx11"),
    cp.getboolean("forge.example", "missing", fallback=False),
)
print("multiline:", repr(cp.get("topsecret.server.example", "multiline")))
print("items_sec:", cp.items("forge.example"))
print("items_sec_raw:", cp.items("forge.example", raw=True))
print("items_top:", [(k, repr(v)) for k, v in cp.items()])

# 3. SectionProxy & MutableMapping protocol
sec = cp["forge.example"]
print("proxy:", isinstance(sec, SectionProxy), repr(sec), sec.name, sec.parser is cp)
print("proxy_get:", sec["user"], sec.get("missing", "def"), sec.getint("port"), sec.getfloat("ratio"), sec.getboolean("compression"))
print("proxy_len_keys:", len(sec), list(sec))
sec["extra"] = "hello"
print("proxy_set:", cp.get("forge.example", "extra"), "extra" in sec)
del sec["extra"]
print("proxy_del:", "extra" in sec)

cp["new_sec"] = {"KeyA": "valA", "KeyB": "123"}
print("cp_setitem:", cp.has_section("new_sec"), cp["new_sec"]["keya"], cp["new_sec"].getint("keyb"))
popped_name, popped_proxy = cp.popitem()
print("popitem:", popped_name, repr(popped_proxy))
del cp["new_sec"]
print("after_del:", cp.has_section("new_sec"))
try:
    del cp["DEFAULT"]
except ValueError as e:
    print("del_default_err:", str(e))

# 4. ExtendedInterpolation
ext = ConfigParser(interpolation=ExtendedInterpolation())
ext.read_string("""
[Common]
home_dir = /Users
library_dir = ${home_dir}/Library
system_dir = /System${library_dir}

[Frameworks]
Python = 3.12
path = ${Common:system_dir}/Frameworks/Python/${Python}
price = $$100
""")
print("ext_common:", ext["Common"]["system_dir"])
print("ext_cross:", ext["Frameworks"]["path"], ext["Frameworks"]["price"])
print("ext_raw:", ext.get("Frameworks", "path", raw=True))

# 5. RawConfigParser & interpolation=None
raw = RawConfigParser()
raw.read_string("[s]\na = %(not_interp)s\nb = ${also_raw}\n")
print("raw_vals:", raw.get("s", "a"), raw.get("s", "b"))
cp_none = ConfigParser(interpolation=None)
cp_none.read_string("[s]\na = %(raw)s\n")
print("interp_none:", cp_none.get("s", "a"))

# 6. Options: allow_no_value, delimiters, inline_comment_prefixes, empty_lines_in_values, strict, default_section, optionxform
cp_nv = ConfigParser(allow_no_value=True, delimiters=(":",), inline_comment_prefixes=("#",), empty_lines_in_values=False)
cp_nv.read_string("""
[flags]
verbose
debug: true # inline comment
multi: a
    b

    not_part
""")
print("nv_vals:", cp_nv.get("flags", "verbose"), cp_nv.getboolean("flags", "debug"), repr(cp_nv.get("flags", "multi")))
print("nv_has_not_part:", cp_nv.has_option("flags", "not_part"), cp_nv.get("flags", "not_part"))

cp_case = ConfigParser()
cp_case.optionxform = str
cp_case.read_string("[S]\nCamelCase = 1\ncamelcase = 2\n")
print("case_sensitive:", cp_case.options("S"), cp_case.get("S", "CamelCase"), cp_case.get("S", "camelcase"))

cp_def = ConfigParser(default_section="COMMON")
cp_def.read_string("[COMMON]\nx = 10\n[A]\ny = %(x)s\n")
print("custom_default:", cp_def.default_section, cp_def.get("A", "y"), list(cp_def))

# 7. Custom converters
cp_conv = ConfigParser(converters={"list": lambda s: [x.strip() for x in s.split(",")]})
cp_conv.read_string("[S]\nitems = a, b, c\n")
print("conv_parser:", cp_conv.getlist("S", "items"))
print("conv_proxy:", cp_conv["S"].getlist("items"))
print("conv_fallback:", cp_conv.getlist("S", "nope", fallback=[]))
print("conv_mapping:", isinstance(cp_conv.converters, ConverterMapping), "list" in cp_conv.converters)

# 8. read_dict, read_file, write, read (from disk)
cp_io = ConfigParser(allow_no_value=True)
cp_io.read_dict({"DEFAULT": {"d": 1}, "sec1": {"k1": "v1", "k2": None, "multi": "a\nb"}})
out = io.StringIO()
cp_io.write(out)
written = out.getvalue()
print("written:\n" + written)
cp_rt = ConfigParser(allow_no_value=True)
cp_rt.read_file(io.StringIO(written), source="<mem>")
print("roundtrip:", cp_rt.get("sec1", "k1"), cp_rt.get("sec1", "k2"), repr(cp_rt.get("sec1", "multi")), cp_rt.get("sec1", "d"))

with tempfile.TemporaryDirectory() as td:
    p1 = os.path.join(td, "a.ini")
    p2 = os.path.join(td, "missing.ini")
    with open(p1, "w", encoding="utf-8") as f:
        f.write("[disk]\nval = 42\n")
    cp_disk = ConfigParser()
    ok = cp_disk.read([p1, p2], encoding="utf-8")
    print("read_disk:", len(ok), ok[0] == p1, cp_disk.getint("disk", "val"))

# 9. Mutation & removal
cp_mut = ConfigParser()
cp_mut.add_section("s1")
cp_mut.set("s1", "a", "1")
print("rem_opt:", cp_mut.remove_option("s1", "a"), cp_mut.remove_option("s1", "a"))
print("rem_sec:", cp_mut.remove_section("s1"), cp_mut.remove_section("s1"))

# 10. Error cases
def check_err(label, fn):
    try:
        fn()
    except Exception as e:
        print(label, type(e).__name__, str(e))

check_err("no_sec:", lambda: cp.get("nope", "x"))
check_err("no_opt:", lambda: cp.get("forge.example", "nope"))
check_err("dup_sec_api:", lambda: cp.add_section("forge.example"))
check_err("add_default:", lambda: cp.add_section("DEFAULT"))
check_err("dup_sec_strict:", lambda: ConfigParser().read_string("[a]\nx=1\n[a]\ny=2\n"))
check_err("dup_opt_strict:", lambda: ConfigParser().read_string("[a]\nx=1\nx=2\n"))
check_err("missing_hdr:", lambda: ConfigParser().read_string("x = 1\n"))
check_err("parse_err:", lambda: ConfigParser().read_string("[a]\nnot_a_kv\n"))

cp_ie = ConfigParser()
cp_ie.read_string("[a]\nx = %(missing)s\nloop1 = %(loop2)s\nloop2 = %(loop1)s\nbad = %q\n")
check_err("interp_missing:", lambda: cp_ie.get("a", "x"))
check_err("interp_depth:", lambda: cp_ie.get("a", "loop1"))
check_err("interp_syntax:", lambda: cp_ie.get("a", "bad"))
check_err("interp_set_bad:", lambda: cp_ie.set("a", "z", "%q"))
check_err("bad_bool:", lambda: ConfigParser(defaults={"b": "maybe"}).getboolean("DEFAULT", "b"))
check_err("non_str_val:", lambda: cp_ie.set("a", "n", 123))
check_err("non_str_sec:", lambda: cp_ie.add_section(123))
check_err("bad_interp_arg:", lambda: ConfigParser(interpolation="bad"))
