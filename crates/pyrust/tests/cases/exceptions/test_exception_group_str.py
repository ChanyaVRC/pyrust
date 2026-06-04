# PEP 654: BaseExceptionGroup.__str__ — "message (N sub-exception[s])" (issue #2176)

# Plural form (2+ sub-exceptions)
print(str(ExceptionGroup("group", [ValueError("a"), TypeError("b")])))

# Singular form (exactly 1 sub-exception)
print(str(ExceptionGroup("m", [ValueError()])))

# str counts only direct sub-exceptions, not recursive leaves
inner = ExceptionGroup("inner", [ValueError(1), TypeError(2)])
outer = ExceptionGroup("outer", [inner, KeyError(3)])
print(str(outer))   # outer (2 sub-exceptions)

# BaseExceptionGroup (non-Exception leaf) uses the same format
beg = BaseExceptionGroup("base", [KeyboardInterrupt(), GeneratorExit()])
print(str(beg))     # base (2 sub-exceptions)

# repr is unaffected — still renders the args
print(repr(ExceptionGroup("group", [ValueError("a"), TypeError("b")])))

# .message and .exceptions are unchanged by the __str__ fix
eg = ExceptionGroup("g", [ValueError(1)])
print(eg.message)
print(eg.exceptions)

# str() inside an f-string / via format()
print(f"{ExceptionGroup('fmt', [ValueError(1), ValueError(2)])}")
