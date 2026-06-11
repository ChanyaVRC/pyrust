# str-subclass receivers for `.format()` / `.format_map()` and the
# receiver-vs-argument distinction for `__format__` overrides (#2376).
#
# Before the fix, `S("{}").format(1)` on a `str` subclass raised
# `TypeError: descriptor 'format' of 'str' object needs an argument`
# because the subclass backing-dispatch path routed through
# `call_str_method`, which had no `format` arm and dropped keyword
# arguments.  `.format_map` and every other str method already worked on
# subclass receivers; this pins the whole sweep so the seam can't drift.
#
# All output must be byte-identical to CPython 3.12.


class S(str):
    pass


class Fmt(str):
    def __format__(self, spec):
        return "FMT:" + spec

    def __str__(self):
        return "STR"


def show(label, fn):
    try:
        print(label, repr(fn()))
    except Exception as e:  # noqa: BLE001 - pin the exception class + message
        print(label, type(e).__name__, e)


# --- .format() on a subclass template ---------------------------------------
show("positional", lambda: S("{}!").format(1))
show("auto-multi", lambda: S("{} {}").format("a", "b"))
show("manual-index", lambda: S("{0}{0}-{1}").format("x", "y"))
show("kwargs", lambda: S("{a}").format(a=5))
show("mixed", lambda: S("{0}-{x}").format("p", x="q"))
show("conv-repr", lambda: S("{!r}").format("z"))
show("spec", lambda: S("{:>5}").format("a"))
show("subscript", lambda: S("{0[1]}").format(["i", "j"]))
show("attr", lambda: S("{0.real}").format(3))

# return type is a plain str, not the subclass
print("ret-type", type(S("{}").format(1)).__name__)

# error paths preserve CPython's exception class + message
show("err-index", lambda: S("{} {}").format(1))
show("err-key", lambda: S("{x}").format())

# --- .format_map() on a subclass template -----------------------------------
show("fmap", lambda: S("{a}").format_map({"a": 2}))
show("fmap-err", lambda: S("{}").format_map({}))

# --- receiver vs argument for a subclass that overrides __format__ ----------
# When the subclass is the format TEMPLATE/receiver, `.format()` uses its
# str value (NOT its __format__).
show("override-receiver", lambda: Fmt("{}").format(1))
# When the subclass is an ARGUMENT, its __format__ override applies.
show("override-arg", lambda: "{}".format(Fmt("x")))
show("override-arg-spec", lambda: "{:>3}".format(Fmt("x")))
# format() / f-string on the subclass instance use its __format__ override.
print("builtin-format", repr(format(Fmt("y"))))
print("fstring", repr(f"{Fmt('y')}"))
# plain subclass (no override): format()/f-string use the str value.
print("plain-format", repr(format(S("v"))))
print("plain-fstring", repr(f"{S('w')}"))

# --- other str methods on subclass receivers (already worked; pin them) -----
print("upper", repr(S("ab").upper()), type(S("ab").upper()).__name__)
print("split", repr(S("a,b").split(",")))
print("join", repr(S("-").join(["a", "b"])))
print("startswith", repr(S("ab").startswith("a")))
print("encode", repr(S("ab").encode()))
print("percent", repr(S("%s") % "x"))
print("replace", repr(S("aXa").replace("X", "Y")))

# CPython returns the receiver itself (subclass identity preserved) for a
# markup-free, non-empty template — surplus arguments ignored.
s_plain = S("noformat")
r_plain = s_plain.format()
print(type(r_plain).__name__, r_plain is s_plain)
print(type(S("x").format(1, k=2)).__name__)
# Brace markup (even escaped) and the empty template build a new plain str.
print(type(S("{{}}").format()).__name__, S("{{}}").format())
print(type(S("").format()).__name__)
