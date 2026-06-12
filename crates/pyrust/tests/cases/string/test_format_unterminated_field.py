# Parity fixture for unterminated str.format / format_map fields vs CPython 3.12
# (#2402).  pyrust used to emit a single generic "Single '{' encountered in
# format string" for every unterminated replacement field; CPython 3.12
# distinguishes the field-name, conversion, and format-spec phases.  Each branch
# prints the message CPython raises; the harness diffs it byte-for-byte.
#
# Templates are run TWICE so the template cache (#2374) is exercised: the cached
# render must produce the identical error to the fresh parse.


def show(thunk):
    try:
        thunk()
    except Exception as e:
        print(type(e).__name__, str(e))


cases = [
    # Bare trailing '{' (no field started) keeps "Single '{'".
    lambda: "{".format(),
    lambda: "x{".format(),
    # Unterminated field name -> "expected '}' before end of string".
    lambda: "{x".format(x=1),
    lambda: "{ ".format(),
    lambda: "{a.b".format(a=1),
    lambda: "{a[".format(a=1),
    lambda: "{0[".format(1),
    lambda: "{0[1".format([10, 20]),
    lambda: "{0[1]".format([10, 20]),
    lambda: "{0[1]x".format([1, 2]),
    lambda: "{0.".format(1),
    # ':' / '!' *inside* an accessor subscript stay part of the field name.
    lambda: "{a[b:c".format(a={"b:c": 9}),
    lambda: "{a[b!c".format(a={"b!c": 9}),
    lambda: "{a[b][c:".format(),
    # Unterminated format spec (after a top-level ':') -> "unmatched '{'".
    lambda: "{x:".format(x=1),
    lambda: "{:".format(1),
    lambda: "{:>".format(1),
    lambda: "{ :".format(),
    lambda: "{x :".format(x=1),
    lambda: "{a]:".format(),
    lambda: "{0[a:".format({"a": 1}),
    lambda: "{a[b]:c".format(a={"b": 9}),
    # Nested replacement inside an unterminated spec -> still "unmatched '{'".
    lambda: "{x:{".format(x=1, y=2),
    lambda: "{x:{y".format(x=1, y=2),
    lambda: "{x:{y}".format(x=1, y=2),
    lambda: "{x:{y:".format(x=1, y=2),
    lambda: "{0:{1".format(8, 3),
    # Conversion phase.
    lambda: "{x!".format(x=1),
    lambda: "{!".format(1),
    lambda: "{0!".format(1),
    lambda: "{0!r".format(1),
    lambda: "{!r".format(1),
    lambda: "{!s".format(1),
    lambda: "{x!r".format(x=1),
    lambda: "{x!r:".format(x=1),
    lambda: "{0!r:>".format(1),
    lambda: "{0!rx".format(1),
    lambda: "{0!rr".format(1),
    lambda: "{a[b]!".format(a={"b": 9}),
    # Error ordering: an earlier complete field renders before the structural
    # error of a later unterminated field.
    lambda: "{x} and {y".format(x="A"),
    lambda: "{x}{y".format(x=1),
    lambda: "{x}{y:".format(x=1),
    lambda: "{x}{".format(x=1),
    # format_map shares the same parser.
    lambda: "{a".format_map({"a": 1}),
    lambda: "{a:".format_map({"a": 1}),
    lambda: "{a:{".format_map({"a": 1}),
    lambda: "{a:{b".format_map({"a": 1, "b": 2}),
    lambda: "{a!".format_map({"a": 1}),
]

# Run each case twice so the cached path is covered and must match the fresh one.
for _ in range(2):
    for c in cases:
        show(c)
