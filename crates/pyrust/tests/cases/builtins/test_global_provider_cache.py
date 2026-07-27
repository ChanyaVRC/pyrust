# A compiled code object owns inline caches but not one fixed globals provider.
# Reusing it with distinct eval/exec dictionaries must never leak values.
expression = compile("source", "", "eval")
print("eval providers:", eval(expression, {"source": 11}), eval(expression, {"source": 22}))

function_code = compile(
    "def read_source():\n"
    "    return source\n"
    "def write_source(value):\n"
    "    global source\n"
    "    source = value\n",
    "",
    "exec",
)
left = {"source": 33}
right = {"source": 44}
exec(function_code, left)
exec(function_code, right)
print("exec providers:", left["read_source"](), right["read_source"]())
left["source"] = 55
print("exec live globals:", left["read_source"]())
left["write_source"](66)
print("exec global write:", left["source"])
print("function globals:", left["read_source"].__globals__ is left)

# Dict methods mutate the live globals mapping without going through a
# SetItem/DeleteItem opcode. Module fast-local reads must still observe the
# mapping immediately, including deletion and in-place union.
module_alias_value = 1
module_alias_globals = globals()
module_alias_globals.update(module_alias_value=2)
print("globals update fastlocal:", module_alias_value)
module_alias_globals.setdefault("module_alias_default", 3)
print("globals setdefault fastlocal:", module_alias_default)
module_alias_globals |= {"module_alias_value": 4}
print("globals ior fastlocal:", module_alias_value)
module_alias_globals.pop("module_alias_value")
try:
    print(module_alias_value)
except NameError:
    print("globals pop fastlocal: NameError")

# A helper imported from another module must mutate the caller's provider, not
# its own globals. The caller module's already-compiled name reads observe both
# update and pop immediately.
from _global_provider_alias_helper import pop_provider, update_provider

imported_alias_value = 10
caller_globals = globals()
update_provider(caller_globals, "imported_alias_value", 11)
print("imported globals update:", imported_alias_value)
print("imported globals pop value:", pop_provider(caller_globals, "imported_alias_value"))
try:
    print(imported_alias_value)
except NameError:
    print("imported globals pop: NameError")

# A normal instance may use the module globals mapping as its __dict__. Its
# attribute writes/deletes are therefore aliases of module-provider writes and
# must invalidate the same compiled-name state.
class GlobalAliasCarrier:
    pass


instance_alias_value = 20
global_alias_carrier = GlobalAliasCarrier()
global_alias_carrier.__dict__ = globals()
global_alias_carrier.instance_alias_value = 21
print(
    "instance dict set:",
    instance_alias_value,
    global_alias_carrier.instance_alias_value,
)
del global_alias_carrier.instance_alias_value
try:
    print(instance_alias_value)
except NameError:
    print("instance dict del: NameError")

# Separate exec locals remain authoritative even when a name also exists in
# globals. globals() and locals() expose their distinct live providers, and a
# normal deletion targets locals rather than globals.
separate_globals = {"overlap": 1}
separate_locals = {"doomed": 3}
exec(
    "overlap = 2\n"
    "seen_globals = globals()\n"
    "seen_locals = locals()\n"
    "del doomed\n",
    separate_globals,
    separate_locals,
)
print(
    "separate exec namespaces:",
    separate_globals["overlap"],
    separate_locals["overlap"],
    separate_locals["seen_globals"] is separate_globals,
    separate_locals["seen_locals"] is separate_locals,
    "doomed" in separate_locals,
)

# Reuse one explicit-exec code object while mutating its separate locals
# provider through direct item operations and dict methods.
separate_alias_globals = {}
separate_alias_locals = {}
separate_alias_code = compile(
    "separate_alias_seen = separate_alias_value",
    "",
    "exec",
)
separate_alias_locals["separate_alias_value"] = 31
exec(separate_alias_code, separate_alias_globals, separate_alias_locals)
print("separate locals direct set:", separate_alias_locals["separate_alias_seen"])
del separate_alias_locals["separate_alias_value"]
try:
    exec(separate_alias_code, separate_alias_globals, separate_alias_locals)
except NameError:
    print("separate locals direct del: NameError")
separate_alias_locals.update(separate_alias_value=32)
exec(separate_alias_code, separate_alias_globals, separate_alias_locals)
print("separate locals update:", separate_alias_locals["separate_alias_seen"])
separate_alias_locals.pop("separate_alias_value")
try:
    exec(separate_alias_code, separate_alias_globals, separate_alias_locals)
except NameError:
    print("separate locals pop: NameError")

# Mutations through locals() while the explicit Script frame is still active
# must replace/clear its fastlocal slot immediately. The final frame flush must
# not resurrect the pre-mutation register value.
in_frame_globals = {}
in_frame_locals = {}
exec(
    "in_frame_value = 41\n"
    "locals()['in_frame_value'] = 42\n"
    "in_frame_seen = in_frame_value\n",
    in_frame_globals,
    in_frame_locals,
)
print(
    "separate locals in-frame set:",
    in_frame_locals["in_frame_seen"],
    in_frame_locals["in_frame_value"],
)
exec(
    "in_frame_deleted = 51\n"
    "provider = locals()\n"
    "provider.pop('in_frame_deleted')\n"
    "try:\n"
    "    in_frame_delete_seen = in_frame_deleted\n"
    "except NameError:\n"
    "    in_frame_delete_seen = 'NameError'\n",
    in_frame_globals,
    in_frame_locals,
)
print(
    "separate locals in-frame del:",
    in_frame_locals["in_frame_delete_seen"],
    "in_frame_deleted" in in_frame_locals,
)

# Keep ordinary locals distinct from declared globals beyond the normal script
# runner's fast-local threshold, for both string and precompiled exec paths.
many_assignments = "\n".join(f"value_{index} = {index}" for index in range(201))
many_assignments += "\nglobal forced_global\nforced_global = 901\n"
for source in (many_assignments, compile(many_assignments, "", "exec")):
    many_globals = {}
    many_locals = {}
    exec(source, many_globals, many_locals)
    print(
        "large exec namespaces:",
        many_locals["value_200"],
        "value_200" in many_globals,
        many_globals["forced_global"],
        "forced_global" in many_locals,
    )

# The canonical builtins module is the authoritative namespace. Its attributes
# may change after a function's LoadGlobal cache has already been populated.
import builtins

original_len = builtins.len
original_int = builtins.int


def read_len():
    return len


print("canonical before:", read_len()([1, 2]))
builtins.len = lambda value: 91
print("canonical changed:", read_len()([]))
del builtins.len
try:
    read_len()
except NameError:
    print("canonical deleted: NameError")
finally:
    builtins.len = original_len

# Finalization is one-time state, not inferred from a mutable public attribute:
# deleting a type must not make a later lookup silently rebuild the namespace.
del builtins.int
try:
    print("canonical type deleted:", eval(compile("int", "", "eval"), {}))
except NameError:
    print("canonical type deleted: NameError")
finally:
    builtins.int = original_int

print(
    "canonical constants:",
    builtins.NotImplemented is NotImplemented,
    builtins.Ellipsis is Ellipsis,
    builtins.__debug__,
)

# Arbitrary attributes added to the canonical provider are builtins too. A
# later same-named module assignment must invalidate the already-populated
# builtin cache even though the name is absent from the static registry.
builtins._pyrust_dynamic_builtin_shadow_probe = "builtin"


def read_dynamic_builtin():
    return _pyrust_dynamic_builtin_shadow_probe


print("dynamic shadow before:", read_dynamic_builtin())
_pyrust_dynamic_builtin_shadow_probe = "module"
print("dynamic shadow after:", read_dynamic_builtin())
del builtins._pyrust_dynamic_builtin_shadow_probe

# A custom module is authoritative too, but remains uncacheable. Reuse the same
# expression with a canonical provider afterwards to catch provider bleed.
import math

math.len = lambda value: 73
builtin_expression = compile("len([])", "", "eval")
print(
    "module providers:",
    eval(builtin_expression, {"__builtins__": math}),
    eval(builtin_expression, {}),
)
del math.len
try:
    eval(builtin_expression, {"__builtins__": math})
except NameError:
    print("module deleted: NameError")

# A source-backed module exposes names through its live globals dictionary
# rather than the built-in module attrs fast path. It is still a valid
# __builtins__ provider.
import _global_provider_filesystem

print(
    "filesystem module provider:",
    eval(
        compile("provider_value", "", "eval"),
        {"__builtins__": _global_provider_filesystem},
    ),
)

# Dict providers are also live and uncacheable through aliases.
provider = {"len": lambda value: 64}
provider_globals = {"__builtins__": provider}
print("dict before:", eval(builtin_expression, provider_globals))
provider["len"] = lambda value: 65
print("dict changed:", eval(builtin_expression, provider_globals))
del provider["len"]
try:
    eval(builtin_expression, provider_globals)
except NameError:
    print("dict deleted: NameError")
