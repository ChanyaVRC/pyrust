# Parity fixture for issue #2009: deeply nested expressions/literals must raise
# a catchable SyntaxError instead of crashing the interpreter with a native
# stack overflow (SIGABRT).  CPython rejects the same input with a SyntaxError
# ("too many nested parentheses"); the key invariant is that the program keeps
# running (the error is catchable) rather than aborting the process.
#
# Covers parens, list / dict / set literals, subscripts, and nested calls.


def caught(label, src):
    try:
        eval(src)
        print(label, "OK")
    except (SyntaxError, RecursionError):
        print(label, "caught")


# Moderate nesting that CPython accepts (well under the limit): must parse.
caught("paren-100", "(" * 100 + "1" + ")" * 100)
caught("list-100", "[" * 100 + "1" + "]" * 100)

# Deep nesting that CPython rejects: must raise a catchable exception, not crash.
caught("paren-2000", "(" * 2000 + "1" + ")" * 2000)
caught("list-5000", "[" * 5000 + "1" + "]" * 5000)
caught("dict-1000", "{1:" * 1000 + "1" + "}" * 1000)
caught("set-1000", "{" * 1000 + "1" + "}" * 1000)
caught("call-1000", "f(" * 1000 + "1" + ")" * 1000)
caught("subscript-1000", "a" + "[0]" * 1 + "[" * 1000 + "0" + "]" * 1000)

print("survived")
