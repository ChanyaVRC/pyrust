# str.format / str.format_map template-parse cache invisibility (issue #2353).
#
# The renderer caches a template's parsed field structure keyed by content.
# These cases exercise the same template repeatedly with different argument
# types and via different methods, plus every error path, to prove the cache
# is behaviourally invisible (identical output and errors to a fresh parse).


def show(thunk):
    try:
        print(repr(thunk()))
    except Exception as e:
        print(type(e).__name__, str(e))


# --- same template reused with different positional arg types ---
tmpl = "[{}|{}]"
for a, b in [(1, 2), ("x", "y"), (3.5, None), (True, [1, 2]), (b"by", {"k": 1})]:
    print(tmpl.format(a, b))

# --- same template, named field, varying types ---
named = "{key}: {val}"
for v in [0, "s", 3.14, None, (1, 2), {"a": 1}]:
    print(named.format(key="k", val=v))

# --- indexed/repeated fields reused ---
idx = "{0} {1} {0}"
print(idx.format("a", "b"))
print(idx.format(10, 20))

# --- attribute + subscript accessors reused across calls ---
class P:
    def __init__(self, r):
        self.real = r


acc = "{0.real} {1[0]} {1[2]}"
print(acc.format(P(1), [7, 8, 9]))
print(acc.format(P("z"), ("p", "q", "r")))

# --- nested spec ({:{width}}) reused with different widths ---
nest = "{0:>{1}}"
print(nest.format("hi", 5))
print(nest.format("longer", 10))
print(nest.format(42, 8))

# --- conversions !r !s !a reused ---
conv = "{0!r} {0!s} {0!a}"
print(conv.format("café"))
print(conv.format(123))
print(conv.format([1, "two"]))

# --- __format__ dunder reused ---
class Custom:
    def __init__(self, n):
        self.n = n

    def __format__(self, spec):
        return "C[" + spec + "]=" + str(self.n)


cust = "{:abc} {:>5}"
print(cust.format(Custom(1), Custom(2)))
print(cust.format(Custom("x"), Custom("y")))

# --- escaped braces + unicode literals reused ---
esc = "{{{}}} café ☕ 日本 {x}"
print(esc.format("v", x="résumé"))
print(esc.format(99, x="naïve"))

# --- format spec mini-language reused ---
spec = "{:08.3f} {:+d} {:#x} {:,}"
print(spec.format(3.14159, 42, 255, 1234567))
print(spec.format(-2.5, -7, 4096, 9999999))

# --- format_map reused with different mappings ---
fm = "{a}-{b}-{a}"
print(fm.format_map({"a": 1, "b": 2}))
print(fm.format_map({"a": "x", "b": "y"}))

# --- format_map with nested mapping accessor ---
fmn = "{d[x]}/{d[y]}"
print(fmn.format_map({"d": {"x": 1, "y": 2}}))

# --- same template content via BOTH format and format_map ---
both = "{a} {a}"
print(both.format(a="f"))
print(both.format_map({"a": "m"}))
print(both.format(a="f2"))

# --- format_map __missing__ ---
class Default(dict):
    def __missing__(self, k):
        return "<" + k + ">"


print("{present} {absent}".format_map(Default(present="here")))

# --- empty / no-field templates ---
print(repr("".format()))
print(repr("plain text".format(1, 2, 3)))

# === error paths (each reused after an error to confirm cache stays valid) ===

# index out of range
show(lambda: "{5}".format(1))
show(lambda: "{}".format())
print("{}".format("recovered"))

# auto/manual switch errors (both directions)
show(lambda: "{} {0}".format(1, 2))
show(lambda: "{0} {}".format(1, 2))

# unknown conversion
show(lambda: "{!q}".format(1))

# missing keyword
show(lambda: "{missing}".format())

# lone braces
show(lambda: "{".format())
show(lambda: "}".format())
show(lambda: "{} {".format(1))     # trailing single '{' after a valid field

# accessor errors
show(lambda: "{0[9]}".format([1, 2]))
show(lambda: "{0.nope}".format(P(1)))

# format_map rejects positional fields
show(lambda: "{0}".format_map({"0": 1}))
show(lambda: "{}".format_map({}))
show(lambda: "{a}".format_map({}))

# nested-spec positional in format_map rejected
show(lambda: "{x:{0}}".format_map({"x": 1}))

# confirm recovery after errors
print("{a}".format(a="final"))
