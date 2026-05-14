# Regression test for issue #429: repr/str of GeneratorExit must not drop args.
#
# Before the fix, the runtime predicate (`is_exception_class`, used by
# `raise`/`except`) accepted `GeneratorExit` as an exception, but the
# repr/str path (`class_chain_contains_exception` in pyrust-core) did not —
# so `repr(GeneratorExit("boom"))` fell back to `<GeneratorExit object>`
# instead of formatting the args.

# Direct construction
print(repr(GeneratorExit("bye")))
print(str(GeneratorExit("bye")))

# Caught instance
try:
    raise GeneratorExit("boom")
except GeneratorExit as e:
    print(repr(e))
    print(str(e))

# Zero-arg form
print(repr(GeneratorExit()))
print(str(GeneratorExit()))

# Sibling-root semantics: GeneratorExit is NOT an Exception subclass.
print(isinstance(GeneratorExit(), Exception))

# `except Exception:` must NOT catch GeneratorExit.
try:
    raise GeneratorExit("g")
except Exception:
    print("caught-by-Exception")
except GeneratorExit:
    print("caught-by-GeneratorExit")

# Other built-in exceptions still format their args (no regression).
print(repr(ValueError("v")))
print(repr(Exception("e")))
print(repr(KeyError("k")))
print(repr(RuntimeError("r")))
