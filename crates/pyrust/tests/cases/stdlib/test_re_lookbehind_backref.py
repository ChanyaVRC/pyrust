import re


def show_search(label, pattern, text):
    try:
        match = re.search(pattern, text)
        if match is None:
            print(label, None)
        else:
            print(label, match.span(), match.groups())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


def show_rejected(label, pattern):
    try:
        re.compile(pattern)
        print(label, "compiled")
    except re.error:
        print(label, "error")


def show_compiled(label, pattern):
    try:
        print(label, "compiled", re.compile(pattern).groups)
    except re.error:
        print(label, "error")


# Issue #2803: the width of a lookbehind backreference comes from the
# completed capture it names.  Numeric and named forms follow the same rule.
show_search("numeric", r"(a)(?<=\1)b", "ab")
show_search("named", r"(?P<g>a)(?<=(?P=g))b", "ab")

# Negative assertions use the same resolved width without leaking or changing
# the capture.  The first subject differs from the capture; the second does
# not, so only the first one matches.
show_search("negative-numeric-pass", r"(a).(?<!\1)b", "acb")
show_search("negative-numeric-fail", r"(a).(?<!\1)b", "aab")
show_search("negative-named-pass", r"(?P<g>a).(?<!(?P=g))b", "acb")

# Widths are not restricted to one character.  Fixed repeats and equal-width
# alternatives remain fixed, and a completed capture may itself contain a
# fixed-width reference to an earlier capture.
show_search("multi", r"(ab)(?<=\1)c", "abc")
show_search("repeat", r"(a{2})(?<=\1)b", "aab")
show_search("alternation", r"((?:ab|cd))(?<=\1)e", "cde")
show_search("nested-reference", r"(a)(\1)(?<=\2)b", "aab")
show_search("nested-completed-numeric", r"((a)\2)(?<=\1)b", "aab")
show_search(
    "nested-completed-named",
    r"(?P<a>(?P<b>a)(?P=b))(?<=(?P=a))b",
    "aab",
)
show_search("nested-lookbehind-prior", r"(a)(?<=(?<=\1))b", "ab")
show_search("scoped-numeric", r"((?i:a))(?<=\1)b", "Ab")
show_search("scoped-named", r"(?P<g>(?i:a))(?<=(?P=g))b", "Ab")
show_search("scoped-direct", r"(?<=(?i:a))b", "Ab")

# Quantifying a zero-width assertion remains zero-width regardless of the
# repetition count.  An exact zero repeat is also fixed even when its body is
# otherwise variable-width.
show_compiled("zero-optional", r"((?=a)?)(?<=\1)a")
show_compiled("zero-star", r"((?=a)*)(?<=\1)a")
show_compiled("zero-plus", r"((?=a)+)(?<=\1)a")
show_compiled("zero-variable-repeat", r"((?=a){1,3})(?<=\1)a")
show_compiled("zero-exact-repeat", r"((?:a+){0})(?<=\1)b")
show_compiled(
    "zero-named-optional",
    r"(?P<g>(?=a)?)(?<=(?P=g))a",
)

# A referenced capture is still invalid as a lookbehind width when its own
# shape is variable.
show_rejected("variable-plus", r"(a+)(?<=\1)b")
show_rejected("variable-star", r"(a*)(?<=\1)b")
show_rejected("variable-optional", r"(a?)(?<=\1)b")
show_rejected("unequal-alternation", r"(a|bb)(?<=\1)c")

# Only completed captures may contribute a width.  Unknown, forward, and open
# group references must stay rejected rather than recursing or guessing.
show_rejected("unknown-numeric", r"(?<=\9)a")
show_rejected("unknown-named", r"(?<=(?P=missing))a")
show_rejected("forward-numeric", r"(?<=\1)(a)")
show_rejected("forward-named", r"(?<=(?P=g))(?P<g>a)")
show_rejected("captured-forward-numeric", r"(\2)(a)(?<=\1)b")
show_rejected(
    "captured-forward-named",
    r"(?P<a>(?P=b))(?P<b>a)(?<=(?P=a))b",
)
show_rejected("open-numeric", r"(a(?<=\1))")
show_rejected("open-named", r"(?P<g>a(?<=(?P=g)))")
show_rejected("same-lookbehind-numeric", r"(?<=(a)\1)b")
show_rejected("same-lookbehind-named", r"(?<=(?P<g>a)(?P=g))b")
show_rejected("zero-repeat-open", r"((?:\1){0})(?<=\1)a")
show_rejected("zero-repeat-forward", r"((?:\2){0})(a)(?<=\1)b")
show_rejected("zero-repeat-unknown", r"((?:\9){0})(?<=\1)a")
show_rejected("zero-repeat-hidden-unknown", r"((?:a+\9){0})(?<=\1)a")
show_rejected("nested-same-lookbehind-numeric", r"(?<=(a)(?<=\1))b")
show_rejected(
    "nested-same-lookbehind-named",
    r"(?<=(?P<g>a)(?<=(?P=g)))b",
)

# Width resolution must remain linear when completed captures share earlier
# captures.  Without memoization, each doubled backreference recursively
# expands the full preceding tree and this small pattern takes exponential
# time to compile.
deep_parts = ["(a)"]
for group_id in range(2, 27):
    previous = group_id - 1
    deep_parts.append("(\\" + str(previous) + "\\" + str(previous) + ")")
deep_pattern = "".join(deep_parts) + r"(?<=\26)x"
print("deep-recursive-width", re.compile(deep_pattern).groups)

# Controls: ordinary backreferences and literal fixed-width lookbehind are
# independent of the new width-resolution path.
show_search("ordinary-backref", r"(ab)\1", "abab")
show_search("ordinary-lookbehind", r"(?<=ab)c", "abc")
