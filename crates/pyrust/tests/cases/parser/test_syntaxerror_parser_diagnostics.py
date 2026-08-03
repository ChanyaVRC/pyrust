# Parser diagnostics exposed through compile() must match CPython 3.12
# (issue #2855).


def check(source):
    try:
        compile(source, "<test>", "exec")
        print("no error")
    except SyntaxError as error:
        # Location metadata is outside #2855; compare the exception class and
        # parser message without SyntaxError.__str__'s filename suffix.
        print(type(error).__name__ + ": " + str(error.args[0]))


check("f(a=1, a=2)")
check("def g(x, x): pass")
check("x = (1 +")
check("1 = 2")
check("x = *a")
