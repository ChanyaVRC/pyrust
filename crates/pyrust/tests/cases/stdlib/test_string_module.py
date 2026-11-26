# Parity fixture for the `string` standard-library module (issue #2515):
# ASCII character-class constants, capwords, Template ($-substitution),
# and Formatter (PEP 3101).  All output is diffed against CPython 3.12.
import string
from string import Template, Formatter


def show(label, fn):
    try:
        print(label, "=>", repr(fn()))
    except Exception as e:
        print(label, "!!", type(e).__name__, e)


# --- constants ---
print(string.ascii_lowercase)
print(string.ascii_uppercase)
print(string.ascii_letters)
print(string.digits)
print(string.hexdigits)
print(string.octdigits)
print(string.punctuation)
print(repr(string.whitespace))
print(string.printable)
print(string.ascii_letters == string.ascii_lowercase + string.ascii_uppercase)
print(
    string.printable
    == string.digits + string.ascii_letters + string.punctuation + string.whitespace
)

# --- capwords ---
print(repr(string.capwords("hello world")))
print(repr(string.capwords("  hello   world  ")))
print(repr(string.capwords("a-b-c", "-")))
print(repr(string.capwords("ABC dEF")))

# --- Template.substitute / safe_substitute ---
show("braced", lambda: Template("${foo}bar").substitute(foo="X"))
show("escape", lambda: Template("$$5").substitute())
show("missing", lambda: Template("$x").substitute())
show("trail", lambda: Template("a$").substitute())
show("space", lambda: Template("a$ b").substitute())
show("digit", lambda: Template("$1abc").substitute())
show("mapping", lambda: Template("$x").substitute({"x": 1}))
show("chainmap", lambda: Template("$a$b").substitute({"a": "1", "b": "2"}, b="Z"))
show("upper", lambda: Template("$Foo").substitute(Foo="Z"))
show("boundary", lambda: Template("$fooXbar").substitute(foo="Y"))
show("safe_all", lambda: Template("$x $$ ${y} $z").safe_substitute(x="1"))
show("safe_badbrace", lambda: Template("${ foo }").safe_substitute())
show("safe_trail", lambda: Template("$ bad").safe_substitute())
show("get_ids", lambda: Template("$a ${b} $a $$ $c").get_identifiers())
show("is_valid_ok", lambda: Template("$a ${b}").is_valid())
show("is_valid_bad", lambda: Template("$ x").is_valid())
show("multiline", lambda: Template("line1\n$ bad").substitute())

# --- Formatter ---
f = Formatter()
show("fmt_pos_kw", lambda: f.format("{0} {name}", "pos", name="kw"))
show("fmt_align", lambda: f.format("{0:>5}", "ab"))
show("fmt_vformat", lambda: f.vformat("{a}", (), {"a": 7}))
show("fmt_auto", lambda: f.format("{} {} {}", "a", "b", "c"))
show("fmt_nested", lambda: f.format("{0:>{1}}", "x", 5))
show("fmt_conv_r", lambda: f.format("{0!r}", "hi"))
show("fmt_attr", lambda: f.format("{0.real}", 3))
show("fmt_index", lambda: f.format("{0[1]}", [10, 20]))
show("fmt_key", lambda: f.format("{d[k]}", d={"k": "v"}))
show("fmt_escape", lambda: f.format("{{literal}} {0}", 5))
show("fmt_switch1", lambda: f.format("{} {0}", "a", "b"))
show("fmt_switch2", lambda: f.format("{0} {}", "a", "b"))
show("fmt_bad_conv", lambda: f.format("{0!x}", "a"))
show("fmt_single_brace", lambda: f.format("a } b"))
show("fmt_unmatched", lambda: f.format("a {0"))

# --- invalid-placeholder line/col + malformed-field wording (regressions) ---
# CPython measures the column from the position after the delimiter and
# splits on universal newlines (\r, \x0b, \x0c), not just \n.
show("inv_midline", lambda: Template("abc$!def").substitute())
show("inv_cr", lambda: Template("a\rb$!").substitute())
show("inv_formfeed", lambda: Template("\x0c$!").substitute())
show("inv_nl_midline", lambda: Template("x\nyy$!").substitute())
# A '{' that is the last char is "Single '{'"; a format spec running to
# end-of-string is "unmatched '{' in format spec".
show("fmt_lone_open", lambda: f.format("end{"))
show("fmt_spec_eos", lambda: f.format("{0:", "a"))
show("fmt_nested_spec_eos", lambda: f.format("{0:{1}", "a", 3))

# =============================================================================
# Issue #3401: native `string` keeps CPython 3.12.3's class protocol.
# =============================================================================
import re
import sys
import _string

# --- module surface (namespace order after the dunder slots) ---
print([n for n in vars(string) if not n.startswith("__") or n == "__all__"])
print(string.__all__)
print(string.__doc__.splitlines()[0], len(string.__doc__))
print(string._string is sys.modules["_string"], string._re is re)
print(string._ChainMap is __import__("collections").ChainMap)
print(type(string._sentinel_dict).__name__, len(string._sentinel_dict))
print("re" in sys.modules, "collections" in sys.modules)
print(Template.__module__, Formatter.__module__, Template.__qualname__)
print(Template.__doc__, Formatter.__doc__)
print([n for n in vars(Template) if n not in ("__dict__", "__weakref__")])
print([n for n in vars(Formatter) if n not in ("__dict__", "__weakref__")])

# --- Template class attributes and the compiled default pattern ---
print(repr(Template.delimiter), repr(Template.idpattern), Template.braceidpattern)
print(Template.flags == re.IGNORECASE, Template.pattern.flags & re.VERBOSE != 0)
print(repr(Template.pattern.pattern))
print(sorted(Template.pattern.groupindex))
print("pattern" in vars(Template), Template.__init_subclass__.__name__)


# --- Template subclasses: __init_subclass__ recompiles `pattern` ---
class Pct(Template):
    delimiter = "%"


show("pct", lambda: Pct("%who likes %{what} 100%%").substitute(who="tim", what="ham"))
show("pct_pattern", lambda: Pct.pattern.pattern)
show("pct_own", lambda: ("pattern" in vars(Pct), Pct.pattern is not Template.pattern))


class Lower(Template):
    flags = 0


show("lower_ok", lambda: Lower("$abc").substitute(abc=1))
show("lower_upper", lambda: Lower("$Abc").safe_substitute(Abc=1))
show("lower_mixed", lambda: Lower("$aBc").safe_substitute(a=1))


class Dotted(Template):
    idpattern = r"[a-z][_a-z0-9.]*"


show("dotted", lambda: Dotted("$a.b ${c.d}").substitute({"a.b": 1, "c.d": 2}))
show("dotted_ids", lambda: Dotted("$a.b ${c.d} $a.b").get_identifiers())


class Brace(Template):
    braceidpattern = r"[^}]+"


show("brace", lambda: Brace("$x ${a b} ${ }").substitute({"x": 1, "a b": 2, " ": 3}))
show("brace_valid", lambda: Brace("${a b} $").is_valid())


class Custom(Template):
    pattern = r"""
    \#(?:
      (?P<escaped>\#) |
      (?P<named>[a-z]+) |
      \[(?P<braced>[a-z]+)\] |
      (?P<invalid>)
    )
    """


show("custom_safe", lambda: Custom("#a #[b] ## #").safe_substitute(a=1, b=2))
show("custom_sub", lambda: Custom("#a #[b] ##").substitute(a=1, b=2))
show("custom_bad", lambda: Custom("x\n #1").substitute())
show("custom_ids", lambda: Custom("#a #[b] #a ##").get_identifiers())
show("custom_valid", lambda: (Custom("#a").is_valid(), Custom("# ").is_valid()))
show("custom_pattern_type", lambda: type(Custom.pattern).__name__)


class Unrecognized(Template):
    pattern = r"\$(?P<other>x)|\$(?P<named>[a-z]+)"


show("unrec_sub", lambda: Unrecognized("$x").substitute())
show("unrec_valid", lambda: Unrecognized("$x").is_valid())
show("unrec_ids", lambda: Unrecognized("$x").get_identifiers())


class Multi(Template):
    delimiter = "@@"


show("multi", lambda: Multi("@@a @@@@ @@{b} @@").safe_substitute(a=1, b=2))
show("multi_err", lambda: Multi("x @@ y").substitute())


class Inherit(Pct):
    idpattern = r"[a-z]+"


show("inherit", lambda: Inherit("%abc1 %x").substitute(abc=1, x=2))
show("inherit_delim", lambda: Inherit.delimiter)


class Hook(Template):
    def _invalid(self, mo):
        raise KeyError("hook at %d" % mo.start("invalid"))


show("hook", lambda: Hook("ab $ c").substitute())


class HookReturn(Template):
    def _invalid(self, mo):
        return None


def hook_return():
    try:
        HookReturn("ab $ c").substitute()
    except ValueError as e:
        return e.args[0], e.args[1] is HookReturn.pattern


show("hook_return", hook_return)

t_inst = Template("$$ $a")
t_inst.delimiter = "#"
show("inst_delim", lambda: t_inst.substitute(a=1))
t_re = Template("$a")
t_re.template = "${b}"
show("reassign", lambda: t_re.substitute(b=5))


class Mixin:
    def __init_subclass__(cls, **kw):
        print("mixin init", cls.__name__)
        super().__init_subclass__(**kw)


class Mixed(Template, Mixin):
    delimiter = "&"


show("mixed", lambda: Mixed("&a").substitute(a=1))


class PlainFirst:
    pass


class SecondBase(PlainFirst, Template):
    delimiter = "%"


show("second_base", lambda: ("pattern" in vars(SecondBase), SecondBase("%a $b").safe_substitute(a=1)))


class Kw(Template):
    def __init_subclass__(cls, extra=None, **kw):
        print("kw init", cls.__name__, extra)
        super().__init_subclass__(**kw)


class KwChild(Kw, extra=5):
    delimiter = "!"


show("kwchild", lambda: KwChild("!a").substitute(a=1))


def bad_kw():
    class Bad(Template, nope=1):
        pass


show("bad_kw", bad_kw)


def bad_re():
    class BadRe(Template):
        idpattern = "("


def bad_re_type():
    try:
        bad_re()
    except re.error:
        return "re.error"


show("bad_re", bad_re_type)
Dyn = type("Dyn", (Template,), {"delimiter": "~"})
show("dyn", lambda: Dyn("~x ~~").substitute(x=3))

# --- substitute mapping semantics ---
show("sentinel", lambda: Template("$a").substitute(string._sentinel_dict, a=1))
show("chainmap", lambda: Template("$a $b").substitute({"a": 1, "b": 2}, b=3))
show("mapping_kw", lambda: Template("$mapping $self").substitute(mapping=1, self=2))


class Loud(dict):
    def __getitem__(self, k):
        print("get", k)
        return super().__getitem__(k)


show("loud", lambda: Template("$a $b $a").safe_substitute(Loud(a=1)))
show("loud_chain", lambda: Template("$a $b").safe_substitute(Loud(a=1), b=2))


class StrKey:
    def __str__(self):
        raise KeyError("in str")


show("str_keyerror_safe", lambda: Template("$a").safe_substitute(a=StrKey()))
show("str_keyerror", lambda: Template("$a").substitute(a=StrKey()))


class KeySub(KeyError):
    pass


class Raiser(dict):
    def __getitem__(self, k):
        raise KeySub(k)


show("keyerror_subclass", lambda: Template("$a").safe_substitute(Raiser()))
show("nonmapping", lambda: Template("$a").substitute(5))
show("lineno_crlf", lambda: Template("a\r\nb\n\x85c$ ").substitute())
show("lineno_ls", lambda: Template("  $").substitute())
show("nonascii", lambda: Template("héllo $wörld").safe_substitute(w="W"))
show("nonascii2", lambda: Template("é$é").safe_substitute())
show("nonascii_err", lambda: Template("éé\né$é").substitute())
show("strsub_tmpl", lambda: Template(type("S", (str,), {})("$a")).substitute(a=1))
show("tmpl_non_str", lambda: Template(5).substitute())
show("tmpl_bytes", lambda: Template(b"$a").substitute(a=1))
show("is_valid_non_str", lambda: Template(5).is_valid())
show("invalid_direct", lambda: Template("ab\ncd")._invalid(Template.pattern.search("ab\nc$")))


class D(Template):
    pass


d_none = D("a$$b$$")
d_none.delimiter = None
show("none_delim", lambda: d_none.substitute())
d_int = D("x$$ $nope")
d_int.delimiter = 5
show("int_delim_keyerror", lambda: d_int.substitute())
show("int_delim_safe", lambda: d_int.safe_substitute())

# --- Formatter subclasses: every hook dispatches through self ---


class Upper(Formatter):
    def format_field(self, value, spec):
        return super().format_field(value, spec).upper()


show("upper", lambda: Upper().format("{0} {x!r:>6}", "ab", x="cd"))


class Default(Formatter):
    def get_value(self, key, args, kwargs):
        if isinstance(key, str):
            return kwargs.get(key, "<%s>" % key)
        return super().get_value(key, args, kwargs)


show("default", lambda: Default().format("{a} {b} {0}", 1, a="A"))


class Strict(Formatter):
    def check_unused_args(self, used_args, args, kwargs):
        print("used", sorted(map(str, used_args)), args, sorted(kwargs))
        unused = set(range(len(args))) | set(kwargs)
        unused -= used_args
        if unused:
            raise ValueError("unused %s" % sorted(map(str, unused)))


show("strict_ok", lambda: Strict().format("{0}{a}", 1, a=2))
show("strict_bad", lambda: Strict().format("{0}", 1, 2, a=3))


class Conv(Formatter):
    def convert_field(self, value, conversion):
        if conversion == "u":
            return str(value).upper()
        return super().convert_field(value, conversion)


show("conv", lambda: Conv().format("{0!u} {0!r} {0!s} {0!a}", "héy"))


class Tracer(Formatter):
    def parse(self, s):
        print("parse", repr(s))
        return super().parse(s)

    def get_field(self, name, args, kwargs):
        print("get_field", repr(name))
        return super().get_field(name, args, kwargs)

    def _vformat(self, fs, args, kwargs, used, depth, auto_arg_index=0):
        print("_vformat", repr(fs), depth, auto_arg_index)
        return super()._vformat(fs, args, kwargs, used, depth, auto_arg_index)

    def vformat(self, fs, args, kwargs):
        print("vformat", repr(fs), args, kwargs)
        return super().vformat(fs, args, kwargs)


show("tracer", lambda: Tracer().format("x{}{:{}}y{k.real}", 1, 2, 3, k=4))


class Lazy(Formatter):
    def format_field(self, v, spec):
        print("format_field", v)
        return super().format_field(v, spec)


show("lazy_parse_error", lambda: Lazy().format("{0}{1}}", 1, 2))
show("lazy_field_error", lambda: Lazy().format("{0}{1[0]x}", 1, [2]))


class ListParse(Formatter):
    def parse(self, s):
        if s == "":
            return []
        return [("lit", "0", "", None), ("end", None, None, None)]


show("list_parse", lambda: ListParse().format("ignored", 7))


class SelfRecursiveParse(Formatter):
    def parse(self, s):
        return [("a", "0", "", None)]


show("parse_recursion", lambda: SelfRecursiveParse().format("x", 1))


class BadParse(Formatter):
    def parse(self, s):
        return [5]


show("bad_parse", lambda: BadParse().format("x", 1))


class ShortParse(Formatter):
    def parse(self, s):
        return [("a", "b")]


show("short_parse", lambda: ShortParse().format("x", 1))


class IntField(Formatter):
    def format_field(self, v, spec):
        return 7


show("int_field", lambda: IntField().format("{0}", 1))


class OneVformat(Formatter):
    def _vformat(self, *a, **k):
        return "x"


show("one_vformat", lambda: OneVformat().format("{0}", 1))
show("parse_type", lambda: type(Formatter().parse("a{0}")).__name__)
show("parse_list", lambda: list(Formatter().parse("a{0!r:^{1}}b{{")))
show("get_field_attr", lambda: Formatter().get_field("0.real", (3,), {}))
show("get_field_item", lambda: Formatter().get_field("a[k][0]", (), {"a": {"k": "xy"}}))
show("get_field_lazy", lambda: Formatter().get_field("0[1]x", [[1, 2]], {}))
show("get_value", lambda: Formatter().get_value(0, ("z",), {}))
show("get_value_bool", lambda: Formatter().get_value(True, (1, 2), {}))
show("get_value_float", lambda: Formatter().get_value(1.0, (1,), {1.0: "f"}))
show("vformat_list", lambda: Formatter().vformat("{0}{x}", [1], {"x": 2}))
show("_vformat", lambda: Formatter()._vformat("{}{}", (1, 2), {}, set(), 2))
show("_vformat_auto", lambda: Formatter()._vformat("{}", (1, 2), {}, set(), 2, 1))
show("_vformat_neg", lambda: Formatter()._vformat("x", (), {}, set(), -1))
show("nested_spec", lambda: Formatter().format("{0:{1}{2}}", 3.14159, ".", "2f"))
show("deep_spec", lambda: Formatter().format("{0:{1:{2}}}", 1, 2, 3))
show("manual_then_auto", lambda: Formatter().format("{1}{}", 1, 2))
show("format_kw_names", lambda: Formatter().format("{self}{format_string}", self=1, format_string=2))
show("format_field", lambda: Formatter().format_field(3, "03d"))
show("convert_bad", lambda: Formatter().convert_field(1, 5))
show("unicode_digit", lambda: Formatter().format("{١}", "a", "b"))
show("superscript_digit", lambda: Formatter().format("{²}", 1, 2, 3))
show("big_index", lambda: Formatter().format("{99999999999999999999}", 1))
show("empty_attr", lambda: Formatter().format("{0.}", 1))
show("missing_kw", lambda: Formatter().format("{x}", 1))
show("used_args", lambda: Formatter().vformat("{0}{a}{0.real}", (1,), {"a": 2}))
show("strsub_format", lambda: Formatter().format(type("S", (str,), {})("{0}"), 5))
show("non_str_format", lambda: Formatter().format(5))
show("format_no_args", lambda: Formatter().format())

# --- _string scanners shared with Formatter.parse / get_field ---
for s in ["a{{b", "{0!r", "{!}", "{0!}", "{0!rr}", "{0[}", "{0[1]x}",
          "{0:{1}}", "{}", "}", "{", "a{0", "{0:", "{a.}", "{a[]}",
          "x}}y{{z", "{0!r:>{1}}", "{٣}", "{0.1}", "{a[b].c[1]}",
          "{0:{{}}}", "{[}", "{0!é}", "{a{b}"]:
    show("parser " + ascii(s), lambda: list(_string.formatter_parser(s)))
for s in ["0", "a.b[c]", "a[1]x", "a.", "a[]", "a[", "٣",
          "99999999999999999999", "", ".x", "[0]", "a..b", "a[0][1].x",
          "a[99999999999999999999]", "１２"]:
    def split(s=s):
        first, rest = _string.formatter_field_name_split(s)
        return first, list(rest)

    show("split " + ascii(s), split)
show("parser_types", lambda: (
    type(_string.formatter_parser("")).__name__,
    type(_string.formatter_field_name_split("")[1]).__name__,
))
print(repr(_string.__doc__), [n for n in vars(_string) if not n.startswith("__")])
show("parser_non_str", lambda: _string.formatter_parser(5))
show("split_non_str", lambda: _string.formatter_field_name_split(5))


def parser_resumes():
    it = _string.formatter_parser("}ab{0}")
    out = []
    try:
        next(it)
    except ValueError as e:
        out.append(str(e))
    out.extend(it)
    return out


show("parser_resumes", parser_resumes)

# --- capwords ---
show("cap_ws", lambda: string.capwords(" aBc  dEf "))
show("cap_sep", lambda: string.capwords("a--b--c", "--"))
show("cap_unicode", lambda: string.capwords("ǆemal ßtraße"))
show("cap_empty", lambda: string.capwords("", None))
show("cap_kw", lambda: string.capwords("hello world", sep=" "))
show("cap_bytes", lambda: string.capwords(b"ab cd", b" "))
show("cap_none", lambda: string.capwords(None))
show("cap_int_sep", lambda: string.capwords("a b", 5))
show("cap_empty_sep", lambda: string.capwords("a b", ""))


class CapStr(str):
    def capitalize(self):
        return "X"


show("cap_str_subclass", lambda: string.capwords(CapStr("ab cd")))

# --- a fresh import generation gets fresh classes; old ones keep working ---
old_template, old_formatter = string.Template, string.Formatter
del sys.modules["string"]
import string as string2

print(string2.Template is old_template, string2.Template.pattern is old_template.pattern)


class OldGen(old_template):
    delimiter = "%"


class NewGen(string2.Template):
    delimiter = "%"


print(OldGen("%a $b").substitute(a=1), NewGen("%a").substitute(a=1))
print(old_template("$a").substitute(a=1), string2.Template("$a").substitute(a=1))
print(old_formatter().format("{}", 1), string2.Formatter().format("{0}", 2))
