# A LoadGlobal name that matches a pure builtin is not proof that the runtime
# value is that builtin. Shared module mappings, explicit exec namespaces, and
# the canonical builtins module can all rebind the name after compilation.
# Every replacement call below has a dead result but an observable body.


def replacement(value):
    print("replacement called:", value)


def call_through_shared_globals():
    abs(-5)


shared_globals = call_through_shared_globals.__globals__
shared_globals["abs"] = replacement
call_through_shared_globals()
del shared_globals["abs"]


explicit_code = compile("abs(-6)", "", "exec")
exec(explicit_code, {"abs": replacement})
exec(explicit_code, {"__builtins__": {"abs": replacement}})


import builtins

original_abs = builtins.abs


def call_through_canonical_builtins():
    abs(-7)


builtins.abs = replacement
try:
    call_through_canonical_builtins()
finally:
    builtins.abs = original_abs
