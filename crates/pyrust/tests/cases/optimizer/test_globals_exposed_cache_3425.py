# Root global inline caches stay valid after `globals()` materialization
# (#3425).
#
# Exposing a module's globals dictionary used to disable its `LoadGlobal`
# caches for good. They now stay enabled: a namespace that is only read through
# the dictionary keeps every cached resolution, and each mutation through any
# alias of the dictionary invalidates the caches at the dictionary-storage
# boundary. Every section below first warms the caches of its reader functions
# and then changes a binding through one particular alias operation, so a stale
# cached resolution would print the old value.
import operator
import sys

import _globals_exposed_cache_owner_3425 as owner


def warm(reader, *args):
    result = None
    for _ in range(4):
        result = reader(*args)
    return result


def outcome(reader, *args):
    try:
        return reader(*args)
    except NameError as error:
        return "NameError: " + str(error)


# --- read-only exposure -----------------------------------------------------
value = 10
flag = "initial"


def helper(a, b):
    return a * 100 + b


def read_value():
    return value


def read_flag():
    return flag


def call_helper(a, b):
    return helper(a, b)


def use_len(items):
    return len(items)


def use_builtins():
    return (len("abc"), max(1, 5), abs(-3), isinstance(1, int))


print("before exposure:", warm(read_value), warm(read_flag), warm(call_helper, 1, 2))
snapshot = list(globals().items())
print("snapshot has names:", all(name in dict(snapshot) for name in ("value", "helper", "warm")))
print("read-only exposure:", warm(read_value), warm(read_flag), warm(call_helper, 3, 4))
for _ in range(3):
    size_before = len(globals())
    keys = sorted(key for key in globals() if key.startswith("read_"))
print("repeated read-only globals():", keys, len(globals()) == size_before)
print("builtins stay resolvable:", warm(use_builtins), warm(use_len, [1, 2, 3]))
print("globals identity:", globals() is globals(), locals() is globals())
print("module dict identity:", sys.modules[__name__].__dict__ is globals())
print("function globals identity:", read_value.__globals__ is globals())

# --- item assignment and deletion through the alias --------------------------
d = globals()
d["value"] = 11
print("setitem existing:", read_value(), value)
d["created"] = "made by alias"


def read_created():
    return created


print("setitem new:", warm(read_created), created)
d["created"] = "rebound by alias"
print("setitem rebind:", read_created(), created)
del d["created"]
print("delitem:", outcome(read_created))
try:
    created
except NameError as error:
    print("delitem module read:", error)
d["created"] = "restored"
print("setitem after delitem:", warm(read_created))

# --- rebinding a function through the alias ---------------------------------
def replacement(a, b):
    return ("replacement", a, b)


warm(call_helper, 5, 6)
d["helper"] = replacement
print("function rebound:", call_helper(5, 6), helper is replacement)
d["helper"] = lambda a, b: a - b
print("function rebound again:", warm(call_helper, 9, 4))

# --- dict methods ------------------------------------------------------------
d.update(value=20, flag="updated")
print("update kwargs:", read_value(), read_flag())
warm(read_value)
d.update({"value": 21})
print("update mapping:", read_value())
warm(read_value)
d.update([("value", 22), ("flag", "pairs")])
print("update pairs:", read_value(), read_flag())
warm(read_value)
print("pop:", d.pop("value"), outcome(read_value))
print("pop default:", d.pop("value", "gone"))
print("setdefault new:", d.setdefault("value", 30), warm(read_value))
print("setdefault existing:", d.setdefault("value", 31), read_value())
d |= {"value": 40, "flag": "ior"}
print("ior:", read_value(), read_flag())
warm(read_value)
d.__setitem__("value", 41)
print("__setitem__:", read_value())
warm(read_value)
dict.__setitem__(d, "value", 42)
print("dict.__setitem__:", read_value())
warm(read_value)
operator.setitem(d, "value", 43)
print("operator.setitem:", read_value())
warm(read_value)
dict.update(d, value=44)
print("dict.update:", read_value())
warm(read_value)
d.__delitem__("value")
print("__delitem__:", outcome(read_value))
d["value"] = 45
warm(read_value)
operator.delitem(d, "value")
print("operator.delitem:", outcome(read_value))
d["value"] = 46
warm(read_value)
d.__ior__({"value": 47})
print("__ior__:", read_value())


def read_tail():
    return zz_last_key


d["zz_last_key"] = "tail"
warm(read_tail)
popped_key, popped_value = d.popitem()
print("popitem:", popped_key, popped_value, outcome(read_tail))

# --- builtin fallback in both directions --------------------------------------
print("builtin before shadow:", warm(use_len, "four"))
d["len"] = lambda items: "shadowed len"
print("alias shadows builtin:", use_len("four"), warm(use_len, "four"))
del d["len"]
print("alias unshadows builtin:", use_len("four"), warm(use_len, "four"))
len = lambda items: "module-level shadow"
print("module shadows builtin:", use_len("four"))
del len
print("module del restores builtin:", use_len("four"), warm(use_len, "four"))
d.setdefault("len", lambda items: "setdefault shadow")
print("setdefault shadows builtin:", use_len("four"))
d.pop("len")
print("pop restores builtin:", use_len("four"))

# --- controlled writes after exposure -----------------------------------------
value = 50
print("module assignment:", read_value(), d["value"])


def set_value(new_value):
    global value
    value = new_value


set_value(51)
print("global store:", read_value(), d["value"], value)
for step in range(5):
    set_value(60 + step)
print("repeated global store:", read_value(), d["value"], value)
d["value"] = 70
set_value(71)
print("alias then global store:", read_value(), d["value"])
set_value(72)
d["value"] = 73
print("global store then alias:", read_value(), value)
del d["value"]
set_value(74)
print("alias delete then global store:", read_value(), d["value"], list(d)[-1] == "value")

counter = 0


def count_up(limit):
    global counter
    for _ in range(limit):
        counter = counter + 1
    return counter


print("hot global store:", count_up(500), d["counter"], counter)
d["counter"] = 1000
print("alias between hot stores:", count_up(500), d["counter"], counter)
d["counter"] -= 400
print("alias augmented:", count_up(10), counter)


def observed_by_dict(limit):
    global counter
    seen = []
    for index in range(limit):
        counter = index
        seen.append(d["counter"])
    return seen


print("dict observes each store:", observed_by_dict(6))

total = 0
for step in range(6):
    total = total + helper(step, 1)
    d["flag"] = "loop %d" % step
print("module loop:", total, step, d["step"], d["total"], read_flag())

# --- exec / eval against the exposed dictionary --------------------------------
warm(read_value)
exec("value = 80", globals())
print("exec assignment:", read_value(), value)
exec("def helper(a, b):\n    return 'exec helper'\n", d)
print("exec def:", call_helper(1, 2))
exec("del value", globals())
print("exec del:", outcome(read_value))
exec("value = 81")
print("exec default namespace:", warm(read_value))
exec("global value\nvalue = 82", globals(), {})
print("exec global statement:", read_value())
print("eval reads:", eval("value + 1", globals()), eval("read_value()"))


def exec_inside_function():
    exec("value = 83", globals())
    return read_value()


print("exec inside function:", exec_inside_function(), value)
exec_function_namespace = globals()
exec(
    "def exec_reader():\n"
    "    return value\n"
    "def exec_writer(new_value):\n"
    "    global value\n"
    "    value = new_value\n",
    exec_function_namespace,
)
print("exec-defined reader:", warm(exec_reader))
exec_writer(84)
print("exec-defined writer:", read_value(), exec_reader(), value)
d["value"] = 85
print("alias seen by exec-defined reader:", exec_reader(), read_value())

# --- other aliases of the same dictionary --------------------------------------
warm(read_value)
sys.modules[__name__].__dict__["value"] = 90
print("module __dict__:", read_value())
warm(read_value)
vars(sys.modules[__name__])["value"] = 91
print("vars(module):", read_value())
warm(read_value)
read_value.__globals__["value"] = 92
print("function __globals__:", read_value())
warm(read_value)
sys._getframe().f_globals["value"] = 93
print("frame f_globals:", read_value())
warm(read_value)
sys._getframe().f_locals["value"] = 94
print("module frame f_locals:", read_value())
warm(read_value)
locals()["value"] = 95
print("module locals():", read_value())
warm(read_value)
setattr(sys.modules[__name__], "value", 96)
print("setattr(module):", read_value(), d["value"])
warm(read_value)
delattr(sys.modules[__name__], "value")
print("delattr(module):", outcome(read_value), "value" in d)
value = 97


class NamespaceCarrier:
    pass


carrier = NamespaceCarrier()
carrier.__dict__ = globals()
warm(read_value)
carrier.value = 98
print("instance __dict__ set:", read_value(), value)
del carrier.value
print("instance __dict__ del:", outcome(read_value))
value = 99
owner.mutate(globals(), "value", 100)
print("mutated by another module:", read_value(), value)
print("popped by another module:", owner.discard(globals(), "value"), outcome(read_value))
value = 101

# --- nested scopes read through the same caches ---------------------------------
class Body:
    captured = value

    def method(self):
        return value


def nested_readers():
    inner = lambda: value
    listed = [value for _ in range(2)]
    generated = list(value for _ in range(2))
    return inner(), listed, generated


print("nested readers:", warm(nested_readers), warm(Body().method), Body.captured)
d["value"] = 102
print("nested readers after alias:", nested_readers(), Body().method(), Body.captured)

# --- one code object, several namespaces ----------------------------------------
shared_code = compile("def shared_reader():\n    return value\n", "<shared>", "exec")
other_namespace = {"value": "other"}
exec(shared_code, other_namespace)
exec(shared_code, globals())
other_reader = other_namespace["shared_reader"]
print("shared code object:", warm(shared_reader), warm(other_reader), warm(shared_reader))
other_namespace["value"] = "other rebound"
d["value"] = 103
print("shared code object after writes:", shared_reader(), other_reader(), read_value())
print("shared code namespaces:", shared_reader.__globals__ is d, other_reader.__globals__ is d)

# --- an imported module's dictionary ---------------------------------------------
print("owner before exposure:", warm(owner.read_level), warm(owner.call_helper, 1))
owner_dict = vars(owner)
print("owner read-only exposure:", warm(owner.read_level), warm(owner.call_helper, 1))
print("owner identities:", owner.__dict__ is owner_dict, owner.own_globals() is owner_dict)
print("owner function globals:", owner.read_level.__globals__ is owner_dict)
owner_dict["level"] = 2
print("vars(owner) setitem:", owner.read_level(), owner.call_helper(1), owner.level)
warm(owner.read_level)
owner.__dict__["level"] = 3
print("owner.__dict__ setitem:", owner.read_level(), owner.level)
warm(owner.read_level)
owner.level = 4
print("owner attribute set:", owner.read_level(), owner_dict["level"])
warm(owner.read_level)
owner.read_level.__globals__["level"] = 5
print("owner function __globals__:", owner.read_level())
warm(owner.read_level)
owner.set_level(6)
print("owner global store:", owner.read_level(), owner_dict["level"], owner.level)
print("owner hot global store:", owner.bump_level(300), owner_dict["level"], owner.level)
owner_dict["level"] = 1000
print("owner alias between hot stores:", owner.bump_level(5), owner.level)
warm(owner.call_helper, 1)
owner_dict["helper"] = lambda item: ("patched", item)
print("owner function patched:", owner.call_helper(1))
owner.helper = lambda item: ("patched by attribute", item)
print("owner function patched by attribute:", owner.call_helper(1))
print("owner missing name:", warm(owner.read_missing))
owner_dict["late_binding"] = "bound late"
print("owner late binding:", owner.read_missing())
del owner.late_binding
print("owner attribute delete:", owner.read_missing())
print("owner builtin:", warm(owner.use_len, "abcd"))
owner_dict["len"] = lambda items: "owner shadow"
print("owner builtin shadowed:", owner.use_len("abcd"))
del owner_dict["len"]
print("owner builtin restored:", owner.use_len("abcd"))
exec("label = 'exec label'", owner_dict)
print("exec into owner dict:", owner.read_label(), owner.label)
warm(owner.read_label)
owner_dict.update(label="updated label", level=7)
print("owner update:", owner.read_label(), owner.read_level())
warm(owner.read_label)
owner_read_level = owner.read_level
owner_use_len = owner.use_len
owner_dict.clear()
print("owner clear:", owner_dict.get("level"), len(owner_dict))
print("owner readers after clear:", outcome(owner_read_level), owner_use_len("ab"))
owner_dict["level"] = "rebuilt"
print("owner rebuilt after clear:", owner_read_level())

# --- sustained alternation ---------------------------------------------------------
checks = []
for round_index in range(200):
    d["value"] = round_index
    first = read_value()
    set_value(round_index + 1)
    second = read_value()
    checks.append(first == round_index and second == round_index + 1 and d["value"] == second)
print("alternating writes:", all(checks), read_value())

# --- clearing the main dictionary is still observed ----------------------------------
def clear_and_report(namespace, report, reader, len_user):
    namespace.clear()
    try:
        seen = reader()
    except NameError as error:
        seen = "NameError: " + str(error)
    report("clear:", seen, len_user([1, 2]), len(namespace))


clear_and_report(d, print, read_value, use_len)
