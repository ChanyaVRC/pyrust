# Parity fixture: \N{name} error paths at compile time.
#
# All three cases raise SyntaxError under CPython 3.12 at parse time.
# The fixture uses compile() to probe each error path and checks that
# the expected fragment appears in the exception message.
for src, expected_fragment in [
    (r'"\N{COMPLETELY UNKNOWN THING}"', "unknown Unicode character name"),
    (r'"\N SNOWMAN"', "malformed \\N character escape"),
    (r'"\N{}"', "malformed \\N character escape"),
]:
    try:
        compile(src, "<test>", "eval")
        print(f"FAIL: no error for {src!r}")
    except SyntaxError as e:
        ok = expected_fragment in str(e)
        print(ok)
