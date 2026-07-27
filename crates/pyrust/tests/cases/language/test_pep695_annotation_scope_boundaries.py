# PEP 695 annotation scopes have three distinct timing/ownership rules:
# definition defaults and generic annotations are eager, bounds and type-alias
# values are lazy, and lazy thunks retain enclosing function cells.

events = []


def mark(label, value):
    events.append(label)
    return value


type Alias = mark("alias", int)


def generic[T: mark("bound", int)](
    value=mark("default", 3),
) -> mark("annotation", int):
    return value


print("hidden-lambda-binding", "<lambda>" in globals())
print("after-definition", events)
print("alias-value", Alias.__value__.__name__, events)
print("alias-cached", Alias.__value__.__name__, events)

type_param = generic.__type_params__[0]
print("bound-value", type_param.__bound__.__name__, events)
print("bound-cached", type_param.__bound__.__name__, events)


def make_captured():
    captured = list
    type CapturedAlias = captured

    def captured_generic[T: captured]():
        pass

    return CapturedAlias, captured_generic


captured_alias, captured_generic = make_captured()
print("captured-alias", captured_alias.__value__.__name__)
print(
    "captured-bound",
    captured_generic.__type_params__[0].__bound__.__name__,
)

# Reads owned by a PEP 695 annotation scope are not prior reads in the
# enclosing function's symbol table. These declarations are valid in CPython;
# defaults remain enclosing-scope reads and are covered by declaration tests.
scope_name = dict


def declaration_boundary():
    type ScopedAlias = scope_name

    def scoped_generic[T: scope_name](value: scope_name):
        return value

    global scope_name
    return ScopedAlias, scoped_generic


scoped_alias, scoped_generic = declaration_boundary()
print("global-alias", scoped_alias.__value__.__name__)
print(
    "global-bound",
    scoped_generic.__type_params__[0].__bound__.__name__,
)
