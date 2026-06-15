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
