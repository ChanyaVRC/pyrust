# Issue #3031: omitted exec namespaces come from the active function frame.

module_value = "module"


def default_string_exec():
    local_value = "local"
    sink = []
    exec("sink.append((local_value, module_value))")
    print("default string:", sink)


def default_code_exec():
    local_value = "local-code"
    sink = []
    code = compile("sink.append((local_value, module_value))", "<exec-test>", "exec")
    exec(code)
    print("default code:", sink)


def default_assignment_exec():
    local_value = "local-assignment"
    exec("seen = local_value")
    print("default assignment:", locals().get("seen"), globals().get("seen"))


def prior_locals_identity():
    local_value = 7
    before = locals()
    exec("added = local_value")
    after = locals()
    print(
        "prior locals:",
        before is after,
        before.get("added"),
        after.get("added"),
    )


def explicit_namespaces():
    local_value = "caller-local"
    globals_ns = {"module_value": "explicit-global"}
    locals_ns = {"local_value": "explicit-local", "sink": []}
    exec(
        "sink.append((local_value, module_value)); assigned = 'locals-only'",
        globals_ns,
        locals_ns,
    )
    print("explicit:", locals_ns["sink"], locals_ns["assigned"])
    print("explicit split:", "assigned" in globals_ns, local_value)


def none_globals_explicit_locals():
    locals_ns = {"local_value": "none-local", "sink": []}
    exec(
        "sink.append((local_value, module_value)); assigned = 'locals-only'",
        None,
        locals_ns,
    )
    print("none globals:", locals_ns["sink"], locals_ns["assigned"])
    print("none globals split:", "assigned" in globals())


def function_from_explicit_globals():
    namespace = {}
    exec(
        """
module_value = "explicit-root"
def nested():
    local_value = "nested-local"
    sink = []
    exec("sink.append((local_value, module_value))")
    return sink
""",
        namespace,
    )
    print("nested explicit root:", namespace["nested"]())


def generator_assignment_exec():
    local_value = "generator-local"
    exec("seen = local_value")
    yield "paused"
    yield locals().get("seen")


default_string_exec()
default_code_exec()
default_assignment_exec()
prior_locals_identity()
explicit_namespaces()
none_globals_explicit_locals()
function_from_explicit_globals()
generator = generator_assignment_exec()
print("generator assignment:", next(generator), next(generator))
